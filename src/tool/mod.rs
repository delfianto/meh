//! Tool system — handler trait, registry, executor, and MCP integration.
//!
//! Tools are the actions the LLM can take on the user's behalf.
//! Each tool implements the `ToolHandler` trait and is registered
//! in a `ToolRegistry`. The `executor` module routes incoming tool
//! calls to the correct handler after permission checks.
//!
//! ```text
//!   LLM emits tool_use
//!         │
//!         ▼
//!   StreamProcessor parses ToolCallComplete
//!         │
//!         ▼
//!   Agent sends ToolCallRequest ──► Controller
//!         │
//!         ▼
//!   PermissionController::check()
//!         │
//!         ├── auto-approved ──► Executor::run()
//!         └── needs approval ──► TUI prompt ──► ApprovalResponse
//!                                                    │
//!                                              ┌─────┴─────┐
//!                                           approved     denied
//!                                              │            │
//!                                        Executor::run()  error result
//!         │
//!         ▼
//!   ToolRegistry::get(name) ──► dyn ToolHandler::execute()
//!         │
//!         ▼
//!   ToolCallResult ──► Agent ──► next API call
//! ```
//!
//! Tool categories determine default permission behavior:
//! - `ReadOnly` — `read_file`, `list_files`, `search_files`
//! - `FileWrite` — `write_file`, `apply_patch`
//! - `Command` — `execute_command`
//! - `Mcp` — dynamically registered MCP server tools
//! - `Informational` — `ask_followup`, `attempt_completion`, `plan_mode_respond`

pub mod definitions;
pub mod executor;
pub mod handlers;
pub mod mcp;

use async_trait::async_trait;

/// Category for permission grouping and filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolCategory {
    /// Tools that only read data (`read_file`, `list_files`, `search_files`).
    ReadOnly,
    /// Tools that write or modify files (`write_file`, `apply_patch`).
    FileWrite,
    /// Tools that execute shell commands (`execute_command`).
    Command,
    /// MCP server tools.
    Mcp,
    /// Informational/communication tools (`ask_followup`, `attempt_completion`, `plan_mode_respond`).
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
    pub const fn success(content: String) -> Self {
        Self {
            content,
            is_error: false,
        }
    }

    /// Create an error response.
    pub const fn error(message: String) -> Self {
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
    /// Tool name — must match what the LLM calls (e.g., `read_file`).
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
        self.handlers.get(name).map(AsRef::as_ref)
    }

    /// Return all registered tool names.
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.handlers.keys().map(String::as_str).collect();
        names.sort_unstable();
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
        defs.sort_by(|a, b| a.name.cmp(&b.name));
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

#[cfg(test)]
mod registry_tests {
    use super::*;

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

    #[async_trait]
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
        reg.register(Box::new(TestHandler::new(
            "test_tool",
            ToolCategory::ReadOnly,
        )));
        assert!(reg.get("test_tool").is_some());
        assert!(reg.get("nonexistent").is_none());
        assert_eq!(reg.len(), 1);
        assert!(!reg.is_empty());
    }

    #[test]
    fn test_register_replaces_existing() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(TestHandler::new(
            "test_tool",
            ToolCategory::ReadOnly,
        )));
        reg.register(Box::new(TestHandler::new(
            "test_tool",
            ToolCategory::FileWrite,
        )));
        assert_eq!(reg.len(), 1);
        assert_eq!(
            reg.get("test_tool").unwrap().category(),
            ToolCategory::FileWrite
        );
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
        reg.register(Box::new(TestHandler::new(
            "tool_a",
            ToolCategory::FileWrite,
        )));
        let defs = reg.tool_definitions();
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].name, "tool_a");
        assert_eq!(defs[1].name, "tool_b");
    }

    #[test]
    fn test_filtered_definitions_exclude_readonly() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(TestHandler::new("reader", ToolCategory::ReadOnly)));
        reg.register(Box::new(TestHandler::new(
            "writer",
            ToolCategory::FileWrite,
        )));
        reg.register(Box::new(TestHandler::new("runner", ToolCategory::Command)));
        let defs = reg.tool_definitions_filtered(&[ToolCategory::ReadOnly]);
        assert_eq!(defs.len(), 2);
        assert!(defs.iter().all(|d| d.name != "reader"));
    }

    #[test]
    fn test_filtered_definitions_exclude_multiple() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(TestHandler::new("reader", ToolCategory::ReadOnly)));
        reg.register(Box::new(TestHandler::new(
            "writer",
            ToolCategory::FileWrite,
        )));
        reg.register(Box::new(TestHandler::new("runner", ToolCategory::Command)));
        reg.register(Box::new(TestHandler::new(
            "info",
            ToolCategory::Informational,
        )));
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
        assert!(reg.get("read_file").is_some());
        assert!(reg.get("list_files").is_some());
        assert!(reg.get("search_files").is_some());
        assert!(reg.get("write_to_file").is_some());
        assert!(reg.get("apply_patch").is_some());
        assert!(reg.get("execute_command").is_some());
        assert!(reg.get("ask_followup_question").is_some());
        assert!(reg.get("attempt_completion").is_some());
        assert!(reg.get("plan_mode_respond").is_some());
        assert!(reg.len() >= 9);
    }

    #[test]
    fn test_with_defaults_categories() {
        let reg = ToolRegistry::with_defaults();
        assert_eq!(
            reg.get("read_file").unwrap().category(),
            ToolCategory::ReadOnly
        );
        assert_eq!(
            reg.get("execute_command").unwrap().category(),
            ToolCategory::Command
        );
    }
}
