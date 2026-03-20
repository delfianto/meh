# STEP 11 — ToolHandler Trait + Registry

## Objective
Define the `ToolHandler` trait, `ToolRegistry` for registration and lookup, and tool definitions for system prompt injection. After this step, tools can be registered and their schemas exported for both native tool calling and XML-based prompt injection.

## Prerequisites
- STEP 01–04 complete

## Detailed Instructions

### 11.1 Define ToolHandler trait (`src/tool/mod.rs`)

```rust
//! Tool system — handler trait, registry, and definitions.

pub mod executor;
pub mod definitions;
pub mod handlers;
pub mod mcp;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Category for permission grouping and filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolCategory {
    /// Tools that only read data (read_file, list_files, search_files).
    ReadOnly,
    /// Tools that write or modify files (write_file, apply_patch).
    FileWrite,
    /// Tools that execute shell commands (execute_command).
    Command,
    /// MCP server tools.
    Mcp,
    /// Informational/communication tools (ask_followup, attempt_completion, plan_mode_respond).
    Informational,
}

/// Context provided to tool handlers during execution.
pub struct ToolContext {
    /// Current working directory.
    pub cwd: String,
    /// Whether this tool invocation was auto-approved (no user confirmation needed).
    pub auto_approved: bool,
}

/// Response from a tool execution.
#[derive(Debug, Clone)]
pub struct ToolResponse {
    /// Text content to return to the LLM.
    pub content: String,
    /// Whether the tool execution resulted in an error.
    pub is_error: bool,
}

impl ToolResponse {
    /// Create a successful response.
    pub fn success(content: String) -> Self {
        Self {
            content,
            is_error: false,
        }
    }

    /// Create an error response.
    pub fn error(message: String) -> Self {
        Self {
            content: message,
            is_error: true,
        }
    }
}

/// The core tool handler trait. Each tool implements this to define its
/// schema, permissions, and execution logic.
#[async_trait]
pub trait ToolHandler: Send + Sync {
    /// Tool name — must match what the LLM calls (e.g., "read_file").
    fn name(&self) -> &str;

    /// Human-readable description shown in approval prompts and system prompt.
    fn description(&self) -> &str;

    /// Execute the tool with the given JSON parameters and context.
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolResponse>;

    /// Whether this tool requires explicit user approval by default.
    /// Read-only tools return false; write/command tools return true.
    fn requires_approval(&self) -> bool {
        true
    }

    /// Category for auto-approval grouping and plan-mode filtering.
    fn category(&self) -> ToolCategory;

    /// JSON Schema for the tool's input parameters.
    /// This is used both for native tool calling (sent to the API) and for
    /// XML-based system prompt injection.
    fn input_schema(&self) -> serde_json::Value;
}
```

### 11.2 Tool Registry (`src/tool/mod.rs` continued)

```rust
/// Registry for looking up tool handlers by name.
pub struct ToolRegistry {
    handlers: std::collections::HashMap<String, Box<dyn ToolHandler>>,
}

impl ToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            handlers: std::collections::HashMap::new(),
        }
    }

    /// Register a tool handler. Replaces any existing handler with the same name.
    pub fn register(&mut self, handler: Box<dyn ToolHandler>) {
        let name = handler.name().to_string();
        self.handlers.insert(name, handler);
    }

    /// Look up a handler by tool name.
    pub fn get(&self, name: &str) -> Option<&dyn ToolHandler> {
        self.handlers.get(name).map(|h| h.as_ref())
    }

    /// Return all registered tool names.
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.handlers.keys().map(|s| s.as_str()).collect();
        names.sort(); // Deterministic ordering
        names
    }

    /// Return the number of registered tools.
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    /// Export all tool definitions for the provider's tool parameter.
    pub fn tool_definitions(&self) -> Vec<crate::provider::ToolDefinition> {
        let mut defs: Vec<_> = self
            .handlers
            .values()
            .map(|h| crate::provider::ToolDefinition {
                name: h.name().to_string(),
                description: h.description().to_string(),
                input_schema: h.input_schema(),
            })
            .collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name)); // Deterministic ordering
        defs
    }

    /// Export definitions filtered by category.
    /// Excludes tools whose category is in the `exclude` list.
    /// Useful for plan mode where write/command tools should be hidden.
    pub fn tool_definitions_filtered(
        &self,
        exclude: &[ToolCategory],
    ) -> Vec<crate::provider::ToolDefinition> {
        let mut defs: Vec<_> = self
            .handlers
            .values()
            .filter(|h| !exclude.contains(&h.category()))
            .map(|h| crate::provider::ToolDefinition {
                name: h.name().to_string(),
                description: h.description().to_string(),
                input_schema: h.input_schema(),
            })
            .collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    /// Create a default registry with all built-in tool handlers.
    pub fn with_defaults() -> Self {
        let mut reg = Self::new();
        reg.register(Box::new(handlers::read_file::ReadFileHandler));
        reg.register(Box::new(handlers::list_files::ListFilesHandler));
        reg.register(Box::new(handlers::search_files::SearchFilesHandler));
        reg.register(Box::new(handlers::write_file::WriteFileHandler));
        reg.register(Box::new(handlers::apply_patch::ApplyPatchHandler));
        reg.register(Box::new(handlers::execute_command::ExecuteCommandHandler));
        reg.register(Box::new(handlers::ask_followup::AskFollowupHandler));
        reg.register(Box::new(
            handlers::attempt_completion::AttemptCompletionHandler,
        ));
        reg.register(Box::new(
            handlers::plan_mode_respond::PlanModeRespondHandler,
        ));
        reg
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
```

### 11.3 Tool Definitions (`src/tool/definitions.rs`)

Provides XML rendering for providers that do not support native tool calling. Also useful for debugging.

```rust
//! Tool schema definitions for system prompt injection.

/// Build the tools section of the system prompt as XML.
/// Used for providers that do not support native tool calling (e.g., some
/// local models via text completion).
pub fn tools_as_xml(tools: &[crate::provider::ToolDefinition]) -> String {
    let mut xml = String::from("<tools>\n");
    for tool in tools {
        xml.push_str(&format!(
            "<tool name=\"{}\">\n<description>{}</description>\n<parameters>{}</parameters>\n</tool>\n",
            tool.name,
            tool.description,
            serde_json::to_string_pretty(&tool.input_schema).unwrap_or_default(),
        ));
    }
    xml.push_str("</tools>");
    xml
}

/// Build a compact one-line-per-tool summary for logging/debugging.
pub fn tools_summary(tools: &[crate::provider::ToolDefinition]) -> String {
    tools
        .iter()
        .map(|t| format!("  - {}: {}", t.name, t.description))
        .collect::<Vec<_>>()
        .join("\n")
}
```

### 11.4 Tool Executor (`src/tool/executor.rs`)

The executor is a convenience wrapper that looks up a handler in the registry and runs it.

```rust
//! Tool executor — runs a tool handler by name.

use crate::tool::{ToolContext, ToolRegistry, ToolResponse};

/// Execute a tool by name with the given parameters.
/// Returns a ToolResponse. If the tool is not found, returns an error response.
pub async fn execute_tool(
    registry: &ToolRegistry,
    tool_name: &str,
    params: serde_json::Value,
    ctx: &ToolContext,
) -> ToolResponse {
    match registry.get(tool_name) {
        Some(handler) => match handler.execute(params, ctx).await {
            Ok(response) => response,
            Err(e) => ToolResponse::error(format!("Tool execution failed: {e}")),
        },
        None => ToolResponse::error(format!("Unknown tool: {tool_name}")),
    }
}
```

### 11.5 Handler stubs module (`src/tool/handlers/mod.rs`)

All handler modules declared here. STEP 12 implements the read-only ones; later steps implement the rest.

```rust
//! Tool handler implementations.

pub mod read_file;
pub mod list_files;
pub mod search_files;
pub mod write_file;
pub mod apply_patch;
pub mod execute_command;
pub mod ask_followup;
pub mod attempt_completion;
pub mod plan_mode_respond;
pub mod mcp_tool;
```

For handlers not yet implemented (write_file, apply_patch, execute_command, ask_followup, attempt_completion, plan_mode_respond, mcp_tool), create stub implementations that return a "not yet implemented" error:

```rust
// Example stub: src/tool/handlers/write_file.rs
use crate::tool::{ToolCategory, ToolContext, ToolHandler, ToolResponse};
use async_trait::async_trait;

pub struct WriteFileHandler;

#[async_trait]
impl ToolHandler for WriteFileHandler {
    fn name(&self) -> &str { "write_to_file" }
    fn description(&self) -> &str { "Write content to a file at the specified path." }
    fn category(&self) -> ToolCategory { ToolCategory::FileWrite }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["path", "content"],
            "properties": {
                "path": { "type": "string", "description": "File path to write to" },
                "content": { "type": "string", "description": "Content to write" }
            }
        })
    }

    async fn execute(
        &self,
        _params: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResponse> {
        Ok(ToolResponse::error("write_to_file not yet implemented".to_string()))
    }
}
```

Create similar stubs for: `apply_patch`, `execute_command`, `ask_followup`, `attempt_completion`, `plan_mode_respond`, `mcp_tool`.

### 11.6 MCP module stub (`src/tool/mcp/mod.rs`)

```rust
//! MCP (Model Context Protocol) tool support — stub for future implementation.
```

## Tests

```rust
#[cfg(test)]
mod registry_tests {
    use super::*;

    // Simple test handler for unit testing
    struct TestHandler {
        tool_name: &'static str,
        tool_category: ToolCategory,
    }

    impl TestHandler {
        fn new(name: &'static str, category: ToolCategory) -> Self {
            Self {
                tool_name: name,
                tool_category: category,
            }
        }
    }

    #[async_trait::async_trait]
    impl ToolHandler for TestHandler {
        fn name(&self) -> &str {
            self.tool_name
        }
        fn description(&self) -> &str {
            "A test tool"
        }
        async fn execute(
            &self,
            _params: serde_json::Value,
            _ctx: &ToolContext,
        ) -> anyhow::Result<ToolResponse> {
            Ok(ToolResponse::success("test result".to_string()))
        }
        fn category(&self) -> ToolCategory {
            self.tool_category
        }
        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {"input": {"type": "string"}}})
        }
    }

    #[test]
    fn test_empty_registry() {
        let reg = ToolRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(reg.names().is_empty());
    }

    #[test]
    fn test_register_and_get() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(TestHandler::new("test_tool", ToolCategory::ReadOnly)));
        assert!(reg.get("test_tool").is_some());
        assert!(reg.get("nonexistent").is_none());
        assert_eq!(reg.len(), 1);
        assert!(!reg.is_empty());
    }

    #[test]
    fn test_register_replaces_existing() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(TestHandler::new("test_tool", ToolCategory::ReadOnly)));
        reg.register(Box::new(TestHandler::new("test_tool", ToolCategory::FileWrite)));
        assert_eq!(reg.len(), 1);
        // Category should be updated
        assert_eq!(reg.get("test_tool").unwrap().category(), ToolCategory::FileWrite);
    }

    #[test]
    fn test_names_sorted() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(TestHandler::new("zebra", ToolCategory::ReadOnly)));
        reg.register(Box::new(TestHandler::new("alpha", ToolCategory::ReadOnly)));
        reg.register(Box::new(TestHandler::new("middle", ToolCategory::ReadOnly)));
        let names = reg.names();
        assert_eq!(names, vec!["alpha", "middle", "zebra"]);
    }

    #[test]
    fn test_tool_definitions() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(TestHandler::new("tool_b", ToolCategory::ReadOnly)));
        reg.register(Box::new(TestHandler::new("tool_a", ToolCategory::FileWrite)));
        let defs = reg.tool_definitions();
        assert_eq!(defs.len(), 2);
        // Should be sorted
        assert_eq!(defs[0].name, "tool_a");
        assert_eq!(defs[1].name, "tool_b");
    }

    #[test]
    fn test_filtered_definitions_exclude_readonly() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(TestHandler::new("reader", ToolCategory::ReadOnly)));
        reg.register(Box::new(TestHandler::new("writer", ToolCategory::FileWrite)));
        reg.register(Box::new(TestHandler::new("runner", ToolCategory::Command)));
        let defs = reg.tool_definitions_filtered(&[ToolCategory::ReadOnly]);
        assert_eq!(defs.len(), 2);
        assert!(defs.iter().all(|d| d.name != "reader"));
    }

    #[test]
    fn test_filtered_definitions_exclude_multiple() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(TestHandler::new("reader", ToolCategory::ReadOnly)));
        reg.register(Box::new(TestHandler::new("writer", ToolCategory::FileWrite)));
        reg.register(Box::new(TestHandler::new("runner", ToolCategory::Command)));
        reg.register(Box::new(TestHandler::new("info", ToolCategory::Informational)));
        let defs = reg.tool_definitions_filtered(&[ToolCategory::FileWrite, ToolCategory::Command]);
        assert_eq!(defs.len(), 2);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"reader"));
        assert!(names.contains(&"info"));
    }

    #[test]
    fn test_filtered_definitions_exclude_none() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(TestHandler::new("tool", ToolCategory::ReadOnly)));
        let defs = reg.tool_definitions_filtered(&[]);
        assert_eq!(defs.len(), 1);
    }

    #[test]
    fn test_tool_response_success() {
        let r = ToolResponse::success("ok".to_string());
        assert!(!r.is_error);
        assert_eq!(r.content, "ok");
    }

    #[test]
    fn test_tool_response_error() {
        let r = ToolResponse::error("fail".to_string());
        assert!(r.is_error);
        assert_eq!(r.content, "fail");
    }

    #[test]
    fn test_tool_response_clone() {
        let r = ToolResponse::success("data".to_string());
        let r2 = r.clone();
        assert_eq!(r.content, r2.content);
        assert_eq!(r.is_error, r2.is_error);
    }

    #[tokio::test]
    async fn test_handler_execute() {
        let handler = TestHandler::new("test", ToolCategory::ReadOnly);
        let ctx = ToolContext {
            cwd: "/tmp".to_string(),
            auto_approved: false,
        };
        let result = handler.execute(serde_json::json!({}), &ctx).await.unwrap();
        assert_eq!(result.content, "test result");
        assert!(!result.is_error);
    }

    #[test]
    fn test_handler_requires_approval_default() {
        let handler = TestHandler::new("test", ToolCategory::ReadOnly);
        // Default implementation returns true
        assert!(handler.requires_approval());
    }

    #[test]
    fn test_default_registry() {
        let reg = ToolRegistry::default();
        assert!(reg.is_empty());
    }

    #[test]
    fn test_with_defaults_has_core_tools() {
        let reg = ToolRegistry::with_defaults();
        // Must have the read-only tools
        assert!(reg.get("read_file").is_some());
        assert!(reg.get("list_files").is_some());
        assert!(reg.get("search_files").is_some());
        // Must have write tools
        assert!(reg.get("write_to_file").is_some());
        assert!(reg.get("apply_patch").is_some());
        // Must have command tool
        assert!(reg.get("execute_command").is_some());
        // Must have communication tools
        assert!(reg.get("ask_followup_question").is_some() || reg.get("ask_followup").is_some());
        assert!(reg.get("attempt_completion").is_some());
        // Should have multiple tools
        assert!(reg.len() >= 8);
    }

    #[test]
    fn test_with_defaults_categories() {
        let reg = ToolRegistry::with_defaults();
        // read_file should be ReadOnly
        assert_eq!(
            reg.get("read_file").unwrap().category(),
            ToolCategory::ReadOnly
        );
        // execute_command should be Command
        assert_eq!(
            reg.get("execute_command").unwrap().category(),
            ToolCategory::Command
        );
    }
}

#[cfg(test)]
mod definitions_tests {
    use super::definitions::*;
    use crate::provider::ToolDefinition;

    #[test]
    fn test_tools_as_xml_single() {
        let tools = vec![ToolDefinition {
            name: "test".to_string(),
            description: "A test tool".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
        }];
        let xml = tools_as_xml(&tools);
        assert!(xml.starts_with("<tools>"));
        assert!(xml.ends_with("</tools>"));
        assert!(xml.contains("<tool name=\"test\">"));
        assert!(xml.contains("<description>A test tool</description>"));
        assert!(xml.contains("<parameters>"));
    }

    #[test]
    fn test_tools_as_xml_multiple() {
        let tools = vec![
            ToolDefinition {
                name: "read".to_string(),
                description: "Read".to_string(),
                input_schema: serde_json::json!({}),
            },
            ToolDefinition {
                name: "write".to_string(),
                description: "Write".to_string(),
                input_schema: serde_json::json!({}),
            },
        ];
        let xml = tools_as_xml(&tools);
        assert!(xml.contains("<tool name=\"read\">"));
        assert!(xml.contains("<tool name=\"write\">"));
    }

    #[test]
    fn test_tools_as_xml_empty() {
        let xml = tools_as_xml(&[]);
        assert_eq!(xml, "<tools>\n</tools>");
    }

    #[test]
    fn test_tools_summary() {
        let tools = vec![
            ToolDefinition {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                input_schema: serde_json::json!({}),
            },
            ToolDefinition {
                name: "write_file".to_string(),
                description: "Write a file".to_string(),
                input_schema: serde_json::json!({}),
            },
        ];
        let summary = tools_summary(&tools);
        assert!(summary.contains("read_file: Read a file"));
        assert!(summary.contains("write_file: Write a file"));
    }

    #[test]
    fn test_tools_summary_empty() {
        let summary = tools_summary(&[]);
        assert!(summary.is_empty());
    }
}

#[cfg(test)]
mod executor_tests {
    use super::executor::*;
    use super::*;

    struct EchoHandler;

    #[async_trait::async_trait]
    impl ToolHandler for EchoHandler {
        fn name(&self) -> &str { "echo" }
        fn description(&self) -> &str { "Echo input" }
        fn category(&self) -> ToolCategory { ToolCategory::Informational }
        fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }

        async fn execute(
            &self,
            params: serde_json::Value,
            _ctx: &ToolContext,
        ) -> anyhow::Result<ToolResponse> {
            let text = params
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("no input");
            Ok(ToolResponse::success(text.to_string()))
        }
    }

    struct FailingHandler;

    #[async_trait::async_trait]
    impl ToolHandler for FailingHandler {
        fn name(&self) -> &str { "fail" }
        fn description(&self) -> &str { "Always fails" }
        fn category(&self) -> ToolCategory { ToolCategory::Informational }
        fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }

        async fn execute(
            &self,
            _params: serde_json::Value,
            _ctx: &ToolContext,
        ) -> anyhow::Result<ToolResponse> {
            anyhow::bail!("intentional failure")
        }
    }

    #[tokio::test]
    async fn test_execute_existing_tool() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoHandler));
        let ctx = ToolContext { cwd: "/tmp".to_string(), auto_approved: false };
        let result = execute_tool(&reg, "echo", serde_json::json!({"text": "hello"}), &ctx).await;
        assert!(!result.is_error);
        assert_eq!(result.content, "hello");
    }

    #[tokio::test]
    async fn test_execute_unknown_tool() {
        let reg = ToolRegistry::new();
        let ctx = ToolContext { cwd: "/tmp".to_string(), auto_approved: false };
        let result = execute_tool(&reg, "nonexistent", serde_json::json!({}), &ctx).await;
        assert!(result.is_error);
        assert!(result.content.contains("Unknown tool"));
    }

    #[tokio::test]
    async fn test_execute_failing_tool() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(FailingHandler));
        let ctx = ToolContext { cwd: "/tmp".to_string(), auto_approved: false };
        let result = execute_tool(&reg, "fail", serde_json::json!({}), &ctx).await;
        assert!(result.is_error);
        assert!(result.content.contains("execution failed"));
    }
}
```

## Acceptance Criteria
- [ ] `ToolHandler` trait defined with name, description, execute, requires_approval, category, input_schema
- [ ] `ToolRegistry` supports register, get, names, len, is_empty, tool_definitions, tool_definitions_filtered
- [ ] `ToolRegistry::with_defaults()` registers all built-in handler stubs
- [ ] Tool definitions exportable as XML (for non-native tool calling providers)
- [ ] Tool definitions exportable as `Vec<ToolDefinition>` (for native tool calling)
- [ ] `tools_summary()` produces readable one-line-per-tool output
- [ ] `execute_tool()` handles unknown tools gracefully with error response
- [ ] `execute_tool()` handles handler panics/errors gracefully
- [ ] Plan mode filtering excludes FileWrite and Command categories
- [ ] Tool names and definitions are deterministically sorted
- [ ] Stub handlers exist for all tools not yet implemented (return error response)
- [ ] `cargo clippy -- -D warnings` passes
- [ ] All tests pass (20+ test cases)
