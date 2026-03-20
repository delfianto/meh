//! `execute_command` tool — run shell commands with timeout.

use crate::tool::{ToolCategory, ToolContext, ToolHandler, ToolResponse};
use crate::util::process;
use async_trait::async_trait;
use std::fmt::Write;

/// Handler for executing shell commands.
pub struct ExecuteCommandHandler;

#[async_trait]
impl ToolHandler for ExecuteCommandHandler {
    fn name(&self) -> &str {
        "execute_command"
    }

    fn description(&self) -> &str {
        "Execute a shell command in the working directory. The command runs with a timeout and returns stdout/stderr."
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Command
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                }
            }
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolResponse> {
        let command = params
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: command"))?;

        let timeout = process::resolve_timeout(command);
        let output = process::execute_command(command, &ctx.cwd, timeout).await?;

        let mut result = String::new();

        if !output.stdout.is_empty() {
            let _ = writeln!(result, "{}", process::truncate_output(&output.stdout));
        }

        if !output.stderr.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            let _ = writeln!(
                result,
                "STDERR:\n{}",
                process::truncate_output(&output.stderr)
            );
        }

        if result.is_empty() {
            result = "(no output)".to_string();
        }

        if output.timed_out {
            let is_error = true;
            return Ok(ToolResponse {
                content: result,
                is_error,
            });
        }

        let exit_code = output.exit_code.unwrap_or(-1);
        if exit_code != 0 {
            let _ = write!(result, "\nExit code: {exit_code}");
            return Ok(ToolResponse {
                content: result,
                is_error: true,
            });
        }

        Ok(ToolResponse::success(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ToolContext {
        ToolContext {
            cwd: "/tmp".to_string(),
            auto_approved: true,
        }
    }

    #[tokio::test]
    async fn test_execute_handler() {
        let handler = ExecuteCommandHandler;
        let result = handler
            .execute(serde_json::json!({"command": "echo test"}), &ctx())
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("test"));
    }

    #[tokio::test]
    async fn test_execute_handler_missing_param() {
        let handler = ExecuteCommandHandler;
        assert!(
            handler
                .execute(serde_json::json!({}), &ctx())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_execute_handler_exit_code_in_output() {
        let handler = ExecuteCommandHandler;
        let result = handler
            .execute(serde_json::json!({"command": "false"}), &ctx())
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Exit code"));
    }

    #[tokio::test]
    async fn test_execute_handler_stderr() {
        let handler = ExecuteCommandHandler;
        let result = handler
            .execute(serde_json::json!({"command": "echo err >&2"}), &ctx())
            .await
            .unwrap();
        assert!(result.content.contains("STDERR"));
        assert!(result.content.contains("err"));
    }

    #[tokio::test]
    async fn test_execute_handler_cwd() {
        let handler = ExecuteCommandHandler;
        let ctx = ToolContext {
            cwd: "/".to_string(),
            auto_approved: true,
        };
        let result = handler
            .execute(serde_json::json!({"command": "pwd"}), &ctx)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains('/'));
    }

    #[test]
    fn test_execute_command_metadata() {
        let handler = ExecuteCommandHandler;
        assert_eq!(handler.name(), "execute_command");
        assert!(handler.requires_approval());
        assert_eq!(handler.category(), ToolCategory::Command);
    }
}
