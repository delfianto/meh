//! `plan_mode_respond` tool — present plan and request mode switch.
//!
//! The tool handler validates params and returns the plan text.
//! The agent loop detects this tool and handles mode switching:
//! displays the plan to the user, and if `switch_to_act` is requested,
//! asks for approval before switching to act mode.

use crate::tool::{ToolCategory, ToolContext, ToolHandler, ToolResponse};
use async_trait::async_trait;
use std::fmt::Write;

/// Handler for presenting a plan in plan mode.
pub struct PlanModeRespondHandler;

#[async_trait]
impl ToolHandler for PlanModeRespondHandler {
    fn name(&self) -> &str {
        "plan_mode_respond"
    }

    fn description(&self) -> &str {
        "Present a plan to the user and optionally request switching to act mode to execute it."
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Informational
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["response"],
            "properties": {
                "response": {
                    "type": "string",
                    "description": "The plan or response to present to the user"
                },
                "options": {
                    "type": "object",
                    "properties": {
                        "switch_to_act": {
                            "type": "boolean",
                            "description": "Whether to request switching to act mode"
                        }
                    }
                }
            }
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResponse> {
        let response = params
            .get("response")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: response"))?;

        let switch_to_act = params
            .get("options")
            .and_then(|o| o.get("switch_to_act"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let mut output = response.to_string();

        if switch_to_act {
            let _ = write!(output, "\n\n[Requesting switch to act mode]");
        }

        Ok(ToolResponse::success(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ToolContext {
        ToolContext {
            cwd: "/tmp".to_string(),
            auto_approved: false,
        }
    }

    #[tokio::test]
    async fn test_plan_mode_respond() {
        let handler = PlanModeRespondHandler;
        let result = handler
            .execute(
                serde_json::json!({"response": "Here is my plan:\n1. Read the file\n2. Fix the bug"}),
                &ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("plan"));
    }

    #[tokio::test]
    async fn test_plan_mode_with_switch() {
        let handler = PlanModeRespondHandler;
        let result = handler
            .execute(
                serde_json::json!({
                    "response": "Plan ready.",
                    "options": {"switch_to_act": true}
                }),
                &ctx(),
            )
            .await
            .unwrap();
        assert!(result.content.contains("Plan ready"));
        assert!(result.content.contains("switch to act mode"));
    }

    #[tokio::test]
    async fn test_plan_mode_without_switch() {
        let handler = PlanModeRespondHandler;
        let result = handler
            .execute(
                serde_json::json!({
                    "response": "Still thinking.",
                    "options": {"switch_to_act": false}
                }),
                &ctx(),
            )
            .await
            .unwrap();
        assert!(result.content.contains("Still thinking"));
        assert!(!result.content.contains("switch to act mode"));
    }

    #[tokio::test]
    async fn test_plan_mode_missing_response() {
        let handler = PlanModeRespondHandler;
        let result = handler.execute(serde_json::json!({}), &ctx()).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_plan_mode_no_approval() {
        let handler = PlanModeRespondHandler;
        assert!(!handler.requires_approval());
        assert_eq!(handler.category(), ToolCategory::Informational);
    }

    #[test]
    fn test_plan_mode_metadata() {
        let handler = PlanModeRespondHandler;
        assert_eq!(handler.name(), "plan_mode_respond");
    }
}
