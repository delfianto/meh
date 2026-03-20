//! Tool executor — runs a tool handler by name.

use crate::tool::{ToolContext, ToolRegistry, ToolResponse};

/// Execute a tool by name with the given parameters.
/// Returns a `ToolResponse`. If the tool is not found, returns an error response.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{ToolCategory, ToolHandler};

    struct EchoHandler;

    #[async_trait::async_trait]
    impl ToolHandler for EchoHandler {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echo input"
        }
        fn category(&self) -> ToolCategory {
            ToolCategory::Informational
        }
        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }

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
        fn name(&self) -> &str {
            "fail"
        }
        fn description(&self) -> &str {
            "Always fails"
        }
        fn category(&self) -> ToolCategory {
            ToolCategory::Informational
        }
        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }

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
        let ctx = ToolContext {
            cwd: "/tmp".to_string(),
            auto_approved: false,
        };
        let result = execute_tool(&reg, "echo", serde_json::json!({"text": "hello"}), &ctx).await;
        assert!(!result.is_error);
        assert_eq!(result.content, "hello");
    }

    #[tokio::test]
    async fn test_execute_unknown_tool() {
        let reg = ToolRegistry::new();
        let ctx = ToolContext {
            cwd: "/tmp".to_string(),
            auto_approved: false,
        };
        let result = execute_tool(&reg, "nonexistent", serde_json::json!({}), &ctx).await;
        assert!(result.is_error);
        assert!(result.content.contains("Unknown tool"));
    }

    #[tokio::test]
    async fn test_execute_failing_tool() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(FailingHandler));
        let ctx = ToolContext {
            cwd: "/tmp".to_string(),
            auto_approved: false,
        };
        let result = execute_tool(&reg, "fail", serde_json::json!({}), &ctx).await;
        assert!(result.is_error);
        assert!(result.content.contains("execution failed"));
    }
}
