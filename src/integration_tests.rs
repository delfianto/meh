//! Integration tests exercising full component pipelines.
//!
//! These tests verify end-to-end behavior across modules:
//! controller ↔ agent, prompt assembly, cost/token formatting,
//! task history, ignore controller, slash commands, and cancellation.

#[cfg(test)]
mod slash_command_dispatch {
    use crate::commands::{self, SlashCommand};
    use crate::controller::Controller;
    use crate::controller::messages::{ControllerMessage, UiUpdate};
    use crate::permission::PermissionMode;
    use crate::state::StateManager;
    use crate::tui::chat_view::ChatRole;

    async fn make_controller() -> (
        Controller,
        tokio::sync::mpsc::UnboundedSender<ControllerMessage>,
        tokio::sync::mpsc::Receiver<UiUpdate>,
    ) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let state = StateManager::new(Some(path)).await.unwrap();
        Controller::new(state, PermissionMode::Ask)
    }

    fn drain(rx: &mut tokio::sync::mpsc::Receiver<UiUpdate>) -> Vec<UiUpdate> {
        let mut v = Vec::new();
        while let Ok(u) = rx.try_recv() {
            v.push(u);
        }
        v
    }

    #[tokio::test]
    async fn help_displays_commands() {
        let (controller, ctrl_tx, mut ui_rx) = make_controller().await;
        let handle = tokio::spawn(controller.run());

        ctrl_tx
            .send(ControllerMessage::SlashCommand(
                SlashCommand::Help,
                String::new(),
            ))
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let updates = drain(&mut ui_rx);
        assert!(updates.iter().any(
            |u| matches!(u, UiUpdate::AppendMessage { content, .. } if content.contains("/help"))
        ));

        ctrl_tx.send(ControllerMessage::Quit).unwrap();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn yolo_toggles() {
        let (controller, ctrl_tx, mut ui_rx) = make_controller().await;
        let handle = tokio::spawn(controller.run());

        ctrl_tx
            .send(ControllerMessage::SlashCommand(
                SlashCommand::Yolo,
                String::new(),
            ))
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let updates = drain(&mut ui_rx);
        assert!(updates.iter().any(|u| matches!(
            u,
            UiUpdate::StatusUpdate {
                is_yolo: Some(true),
                ..
            }
        )));

        ctrl_tx.send(ControllerMessage::Quit).unwrap();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn mode_switch_plan() {
        let (controller, ctrl_tx, mut ui_rx) = make_controller().await;
        let handle = tokio::spawn(controller.run());

        ctrl_tx
            .send(ControllerMessage::SlashCommand(
                SlashCommand::Mode("plan".to_string()),
                String::new(),
            ))
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let updates = drain(&mut ui_rx);
        assert!(
            updates
                .iter()
                .any(|u| matches!(u, UiUpdate::StatusUpdate { mode: Some(m), .. } if m == "PLAN"))
        );

        ctrl_tx.send(ControllerMessage::Quit).unwrap();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn model_change() {
        let (controller, ctrl_tx, mut ui_rx) = make_controller().await;
        let handle = tokio::spawn(controller.run());

        ctrl_tx
            .send(ControllerMessage::SlashCommand(
                SlashCommand::Model("gpt-4o".to_string()),
                String::new(),
            ))
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let updates = drain(&mut ui_rx);
        assert!(updates.iter().any(
            |u| matches!(u, UiUpdate::AppendMessage { content, .. } if content.contains("gpt-4o"))
        ));

        ctrl_tx.send(ControllerMessage::Quit).unwrap();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn new_task_cancels_agent() {
        let (controller, ctrl_tx, mut ui_rx) = make_controller().await;
        let handle = tokio::spawn(controller.run());

        ctrl_tx
            .send(ControllerMessage::SlashCommand(
                SlashCommand::NewTask,
                String::new(),
            ))
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let updates = drain(&mut ui_rx);
        assert!(updates.iter().any(
            |u| matches!(u, UiUpdate::AppendMessage { content, .. } if content.contains("New task"))
        ));

        ctrl_tx.send(ControllerMessage::Quit).unwrap();
        let _ = handle.await;
    }

    #[test]
    fn all_commands_parse() {
        let inputs = [
            "/help",
            "/h",
            "/clear",
            "/compact",
            "/smol",
            "/history",
            "/settings",
            "/new",
            "/newtask",
            "/plan",
            "/act",
            "/model claude-sonnet-4",
            "/yolo",
        ];
        for input in inputs {
            assert!(
                commands::parse_slash_command(input).is_some(),
                "Failed to parse: {input}"
            );
        }
    }

    #[test]
    fn unknown_command_returns_none() {
        assert!(commands::parse_slash_command("/foobar").is_none());
        assert!(commands::parse_slash_command("hello").is_none());
        assert!(commands::parse_slash_command("/").is_none());
    }
}

#[cfg(test)]
mod cancellation_flow {
    use crate::controller::Controller;
    use crate::controller::messages::{ControllerMessage, UiUpdate};
    use crate::controller::task::TaskCancellation;
    use crate::permission::PermissionMode;
    use crate::state::StateManager;

    async fn make_controller() -> (
        Controller,
        tokio::sync::mpsc::UnboundedSender<ControllerMessage>,
        tokio::sync::mpsc::Receiver<UiUpdate>,
    ) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let state = StateManager::new(Some(path)).await.unwrap();
        Controller::new(state, PermissionMode::Ask)
    }

    fn drain(rx: &mut tokio::sync::mpsc::Receiver<UiUpdate>) -> Vec<UiUpdate> {
        let mut v = Vec::new();
        while let Ok(u) = rx.try_recv() {
            v.push(u);
        }
        v
    }

    #[tokio::test]
    async fn single_cancel_shows_message() {
        let (controller, ctrl_tx, mut ui_rx) = make_controller().await;
        let handle = tokio::spawn(controller.run());

        ctrl_tx.send(ControllerMessage::CancelTask).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let updates = drain(&mut ui_rx);

        assert!(updates.iter().any(
            |u| matches!(u, UiUpdate::AppendMessage { content, .. } if content.contains("cancelled"))
        ));
        assert!(updates.iter().any(|u| matches!(
            u,
            UiUpdate::StatusUpdate {
                is_streaming: Some(false),
                ..
            }
        )));

        ctrl_tx.send(ControllerMessage::Quit).unwrap();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn double_cancel_force_quits() {
        let (controller, ctrl_tx, mut ui_rx) = make_controller().await;
        let handle = tokio::spawn(controller.run());

        ctrl_tx.send(ControllerMessage::CancelTask).unwrap();
        ctrl_tx.send(ControllerMessage::CancelTask).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let updates = drain(&mut ui_rx);

        assert!(updates.iter().any(|u| matches!(u, UiUpdate::Quit)));
        let _ = handle.await;
    }

    #[test]
    fn token_reset_clears_double_cancel() {
        let mut tc = TaskCancellation::new();
        tc.cancel();
        assert!(tc.is_cancelled());
        tc.reset();
        assert!(!tc.is_cancelled());
        let is_double = tc.cancel();
        assert!(!is_double);
    }

    #[test]
    fn token_propagates_to_clones() {
        let mut tc = TaskCancellation::new();
        let token = tc.token();
        assert!(!token.is_cancelled());
        tc.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn reset_creates_new_token() {
        let mut tc = TaskCancellation::new();
        let old = tc.token();
        tc.cancel();
        tc.reset();
        let new = tc.token();
        assert!(old.is_cancelled());
        assert!(!new.is_cancelled());
    }
}

#[cfg(test)]
mod config_reload {
    use crate::controller::Controller;
    use crate::controller::messages::{ControllerMessage, UiUpdate};
    use crate::permission::PermissionMode;
    use crate::state::StateManager;
    use crate::state::config::AppConfig;

    #[tokio::test]
    async fn reload_updates_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        AppConfig::default().save(&path).unwrap();

        let state = StateManager::new(Some(path.clone())).await.unwrap();
        assert_eq!(state.config().await.provider.default, "anthropic");

        let mut new_config = AppConfig::default();
        new_config.provider.default = "openai".to_string();
        new_config.save(&path).unwrap();

        state.reload().await.unwrap();
        assert_eq!(state.config().await.provider.default, "openai");
    }

    #[tokio::test]
    async fn reload_invalid_preserves_old() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        AppConfig::default().save(&path).unwrap();

        let state = StateManager::new(Some(path.clone())).await.unwrap();
        std::fs::write(&path, "invalid [[[toml").unwrap();

        assert!(state.reload().await.is_err());
        assert_eq!(state.config().await.provider.default, "anthropic");
    }

    #[tokio::test]
    async fn controller_handles_reload_message() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        AppConfig::default().save(&path).unwrap();

        let state = StateManager::new(Some(path)).await.unwrap();
        let (controller, ctrl_tx, mut ui_rx) = Controller::new(state, PermissionMode::Ask);
        let handle = tokio::spawn(controller.run());

        ctrl_tx.send(ControllerMessage::ConfigReload).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut updates = Vec::new();
        while let Ok(u) = ui_rx.try_recv() {
            updates.push(u);
        }
        assert!(updates.iter().any(|u| matches!(u,
            UiUpdate::AppendMessage { content, .. } if content.contains("reloaded") || content.contains("Config")
        )));

        ctrl_tx.send(ControllerMessage::Quit).unwrap();
        let _ = handle.await;
    }
}

#[cfg(test)]
mod ignore_integration {
    use crate::ignore::IgnoreController;
    use std::path::Path;
    use tempfile::TempDir;

    #[test]
    fn default_patterns_block_secrets() {
        let dir = TempDir::new().unwrap();
        let ctrl = IgnoreController::new(dir.path());
        assert!(!ctrl.is_allowed(&dir.path().join(".env")));
        assert!(!ctrl.is_allowed(&dir.path().join(".env.production")));
        assert!(!ctrl.is_allowed(&dir.path().join("server.pem")));
        assert!(!ctrl.is_allowed(&dir.path().join("server.key")));
        assert!(!ctrl.is_allowed(&dir.path().join("credentials.json")));
        assert!(ctrl.is_allowed(&dir.path().join("src/main.rs")));
        assert!(ctrl.is_allowed(&dir.path().join("Cargo.toml")));
    }

    #[test]
    fn custom_patterns_override() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".mehignore"), "*.secret\nbuild/**\n").unwrap();
        let ctrl = IgnoreController::new(dir.path());
        assert!(!ctrl.is_allowed(&dir.path().join("api.secret")));
        assert!(ctrl.is_allowed(&dir.path().join("src/lib.rs")));
    }

    #[test]
    fn negation_overrides() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".mehignore"), "*.log\n!important.log\n").unwrap();
        let ctrl = IgnoreController::new(dir.path());
        assert!(!ctrl.is_allowed(&dir.path().join("debug.log")));
        assert!(ctrl.is_allowed(&dir.path().join("important.log")));
    }

    #[test]
    fn filter_paths_removes_blocked() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".mehignore"), "*.log\n").unwrap();
        let ctrl = IgnoreController::new(dir.path());
        let paths = vec![dir.path().join("main.rs"), dir.path().join("debug.log")];
        let filtered = ctrl.filter_paths(&paths);
        assert_eq!(filtered.len(), 1);
        assert!(filtered[0].to_string_lossy().contains("main.rs"));
    }

    #[test]
    fn outside_workspace_always_allowed() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".mehignore"), "*").unwrap();
        let ctrl = IgnoreController::new(dir.path());
        assert!(ctrl.is_allowed(Path::new("/etc/hosts")));
    }

    #[test]
    fn reload_picks_up_changes() {
        let dir = TempDir::new().unwrap();
        let mut ctrl = IgnoreController::new(dir.path());
        assert!(ctrl.is_allowed(&dir.path().join("test.log")));

        std::fs::write(dir.path().join(".mehignore"), "*.log\n").unwrap();
        ctrl.reload();
        assert!(!ctrl.is_allowed(&dir.path().join("test.log")));
    }
}

#[cfg(test)]
mod prompt_integration {
    use crate::prompt::environment::EnvironmentInfo;
    use crate::prompt::rules::{load_rules, rules_to_prompt};
    use crate::prompt::{PromptConfig, build_full_system_prompt};
    use crate::state::task_state::Mode;
    use tempfile::TempDir;

    #[test]
    fn prompt_includes_environment() {
        let env = EnvironmentInfo::detect(".");
        let config = PromptConfig {
            cwd: ".".to_string(),
            mode: Mode::Act,
            tool_definitions_xml: None,
            mcp_tools_description: String::new(),
            user_rules: String::new(),
            environment_info: env.to_prompt_section(),
            yolo_mode: false,
        };
        let prompt = build_full_system_prompt(&config);
        assert!(prompt.contains("OS:"));
        assert!(prompt.contains("Shell:"));
        assert!(prompt.contains("expert AI coding assistant"));
        assert!(prompt.contains("ACT MODE"));
    }

    #[test]
    fn prompt_includes_user_rules() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".mehrules"), "Always write tests first.\n").unwrap();
        let rules = load_rules(dir.path());
        let user_rules = rules_to_prompt(&rules, &[]);

        let config = PromptConfig {
            cwd: dir.path().to_string_lossy().to_string(),
            mode: Mode::Act,
            tool_definitions_xml: None,
            mcp_tools_description: String::new(),
            user_rules,
            environment_info: String::new(),
            yolo_mode: false,
        };
        let prompt = build_full_system_prompt(&config);
        assert!(prompt.contains("Always write tests first"));
    }

    #[test]
    fn prompt_yolo_vs_non_yolo() {
        let yolo = build_full_system_prompt(&PromptConfig {
            cwd: ".".to_string(),
            mode: Mode::Act,
            tool_definitions_xml: None,
            mcp_tools_description: String::new(),
            user_rules: String::new(),
            environment_info: String::new(),
            yolo_mode: true,
        });
        let normal = build_full_system_prompt(&PromptConfig {
            cwd: ".".to_string(),
            mode: Mode::Act,
            tool_definitions_xml: None,
            mcp_tools_description: String::new(),
            user_rules: String::new(),
            environment_info: String::new(),
            yolo_mode: false,
        });
        assert!(yolo.contains("auto-approved"));
        assert!(normal.contains("asked to approve"));
    }

    #[test]
    fn prompt_plan_vs_act_mode() {
        let plan = build_full_system_prompt(&PromptConfig {
            cwd: ".".to_string(),
            mode: Mode::Plan,
            tool_definitions_xml: None,
            mcp_tools_description: String::new(),
            user_rules: String::new(),
            environment_info: String::new(),
            yolo_mode: false,
        });
        let act = build_full_system_prompt(&PromptConfig {
            cwd: ".".to_string(),
            mode: Mode::Act,
            tool_definitions_xml: None,
            mcp_tools_description: String::new(),
            user_rules: String::new(),
            environment_info: String::new(),
            yolo_mode: false,
        });
        assert!(plan.contains("PLAN MODE"));
        assert!(plan.contains("CANNOT edit files"));
        assert!(act.contains("ACT MODE"));
        assert!(act.contains("full tool access"));
    }

    #[test]
    fn prompt_with_mcp_tools() {
        let config = PromptConfig {
            cwd: ".".to_string(),
            mode: Mode::Act,
            tool_definitions_xml: None,
            mcp_tools_description: "- github: create_issue, list_prs".to_string(),
            user_rules: String::new(),
            environment_info: String::new(),
            yolo_mode: false,
        };
        let prompt = build_full_system_prompt(&config);
        assert!(prompt.contains("MCP Server Tools"));
        assert!(prompt.contains("github"));
    }

    #[test]
    fn prompt_with_xml_tools() {
        let config = PromptConfig {
            cwd: ".".to_string(),
            mode: Mode::Act,
            tool_definitions_xml: Some(
                "<tools><tool name=\"read_file\"></tool></tools>".to_string(),
            ),
            mcp_tools_description: String::new(),
            user_rules: String::new(),
            environment_info: String::new(),
            yolo_mode: false,
        };
        let prompt = build_full_system_prompt(&config);
        assert!(prompt.contains("Available Tools"));
        assert!(prompt.contains("read_file"));
    }

    #[test]
    fn prompt_sections_ordered() {
        let config = PromptConfig {
            cwd: ".".to_string(),
            mode: Mode::Act,
            tool_definitions_xml: None,
            mcp_tools_description: String::new(),
            user_rules: "# User Rules\nCustom rule.".to_string(),
            environment_info: "# Environment\nTest env.".to_string(),
            yolo_mode: false,
        };
        let prompt = build_full_system_prompt(&config);
        let role_pos = prompt.find("expert AI coding assistant").unwrap();
        let env_pos = prompt.find("Test env").unwrap();
        let rules_pos = prompt.find("Custom rule").unwrap();
        let editing_pos = prompt.find("File Editing").unwrap();
        assert!(role_pos < env_pos);
        assert!(env_pos < rules_pos);
        assert!(rules_pos < editing_pos);
    }
}

#[cfg(test)]
mod cost_token_integration {
    use crate::provider::{ModelInfo, UsageInfo};
    use crate::util::cost::{self, CostLevel};
    use crate::util::tokens;

    #[test]
    fn format_tokens_all_ranges() {
        assert_eq!(tokens::format_tokens(0), "0");
        assert_eq!(tokens::format_tokens(999), "999");
        assert_eq!(tokens::format_tokens(1_000), "1.0k");
        assert_eq!(tokens::format_tokens(45_200), "45.2k");
        assert_eq!(tokens::format_tokens(1_000_000), "1.0M");
        assert_eq!(tokens::format_tokens(2_500_000), "2.5M");
    }

    #[test]
    fn format_cost_all_ranges() {
        assert_eq!(cost::format_cost(0.001), "$0.0010");
        assert_eq!(cost::format_cost(0.05), "$0.050");
        assert_eq!(cost::format_cost(1.5), "$1.50");
        assert_eq!(cost::format_cost(12.34), "$12.34");
    }

    #[test]
    fn cost_color_thresholds() {
        assert!(matches!(cost::cost_level(0.05), CostLevel::Normal));
        assert!(matches!(cost::cost_level(0.50), CostLevel::Moderate));
        assert!(matches!(cost::cost_level(5.0), CostLevel::Expensive));
    }

    #[test]
    fn calculate_cost_with_cache() {
        let usage = UsageInfo {
            input_tokens: 100_000,
            output_tokens: 50_000,
            cache_read_tokens: Some(80_000),
            cache_write_tokens: Some(20_000),
            thinking_tokens: None,
            total_cost: None,
        };
        let pricing = ModelInfo {
            id: String::new(),
            name: String::new(),
            provider: String::new(),
            max_tokens: 0,
            context_window: 0,
            supports_tools: false,
            supports_thinking: false,
            supports_images: false,
            input_price_per_mtok: 3.0,
            output_price_per_mtok: 15.0,
            cache_read_price_per_mtok: Some(0.30),
            cache_write_price_per_mtok: Some(3.75),
            thinking_price_per_mtok: None,
        };
        let c = cost::calculate_cost(&usage, &pricing);
        assert!(c > 0.0);
        assert!((c - 1.149).abs() < 0.01);
    }

    #[test]
    fn context_utilization_ranges() {
        assert!((tokens::context_utilization(10_000, 200_000) - 5.0).abs() < 0.01);
        assert!((tokens::context_utilization(150_000, 200_000) - 75.0).abs() < 0.01);
        assert!((tokens::context_utilization(190_000, 200_000) - 95.0).abs() < 0.01);
    }

    #[test]
    fn known_pricing_lookup() {
        assert!(cost::get_known_pricing("claude-sonnet-4-20250514").is_some());
        assert!(cost::get_known_pricing("gpt-4.1").is_some());
        assert!(cost::get_known_pricing("gemini-2.5-pro").is_some());
        assert!(cost::get_known_pricing("unknown-model").is_none());
    }
}

#[cfg(test)]
mod task_history_integration {
    use crate::provider::{ContentBlock, Message, MessageRole};
    use crate::state::history::*;
    use tempfile::TempDir;

    fn make_task(task_id: &str, title: &str) -> PersistedTask {
        PersistedTask {
            task_id: task_id.to_string(),
            title: title.to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            messages: vec![],
            mode: "act".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4".to_string(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost: 0.0,
            completed: false,
        }
    }

    #[test]
    fn save_load_roundtrip_with_messages() {
        let dir = TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
        let task = PersistedTask {
            task_id: "test-rt".to_string(),
            title: "Fix bug".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            messages: vec![
                PersistedMessage {
                    role: "user".to_string(),
                    content: vec![PersistedContent::Text {
                        text: "Fix the auth bug".to_string(),
                    }],
                    timestamp: chrono::Utc::now(),
                },
                PersistedMessage {
                    role: "assistant".to_string(),
                    content: vec![
                        PersistedContent::Thinking {
                            text: "Let me check...".to_string(),
                            signature: Some("sig".to_string()),
                        },
                        PersistedContent::Text {
                            text: "Fixed it.".to_string(),
                        },
                        PersistedContent::ToolUse {
                            id: "tc1".to_string(),
                            name: "read_file".to_string(),
                            input: serde_json::json!({"path": "src/auth.rs"}),
                        },
                    ],
                    timestamp: chrono::Utc::now(),
                },
            ],
            mode: "act".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4".to_string(),
            total_input_tokens: 5000,
            total_output_tokens: 2000,
            total_cost: 0.045,
            completed: true,
        };
        history.save_task(&task).unwrap();
        let loaded = history.load_task("test-rt").unwrap();
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[1].content.len(), 3);
        assert!((loaded.total_cost - 0.045).abs() < f64::EPSILON);
    }

    #[test]
    fn list_and_prune() {
        let dir = TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
        let now = chrono::Utc::now();
        for i in 0..10 {
            let mut task = make_task(&format!("t-{i}"), &format!("Task {i}"));
            task.updated_at = now + chrono::Duration::seconds(i64::from(i));
            history.save_task(&task).unwrap();
        }
        let listed = history.list_tasks().unwrap();
        assert_eq!(listed.len(), 10);
        assert_eq!(listed[0].task_id, "t-9");

        let pruned = history.prune(5).unwrap();
        assert_eq!(pruned, 5);
        assert_eq!(history.list_tasks().unwrap().len(), 5);
    }

    #[test]
    fn message_conversion_roundtrip() {
        let msg = Message {
            role: MessageRole::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    text: "hmm".to_string(),
                    signature: Some("sig".to_string()),
                },
                ContentBlock::Text("answer".to_string()),
                ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "read".to_string(),
                    input: serde_json::json!({"path": "a.rs"}),
                },
            ],
        };
        let persisted = PersistedMessage::from(&msg);
        let restored = Message::from(&persisted);
        assert_eq!(restored.content.len(), 3);
    }

    #[test]
    fn generate_title_truncates() {
        let short = generate_title("Fix bug");
        assert_eq!(short, "Fix bug");

        let long = generate_title(&"a".repeat(100));
        assert_eq!(long.len(), 80);
        assert!(long.ends_with("..."));
    }
}

#[cfg(test)]
mod workspace_context_integration {
    use crate::ignore::IgnoreController;
    use crate::prompt::context::workspace_context;
    use tempfile::TempDir;

    #[test]
    fn tree_respects_ignore() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("visible.rs"), "").unwrap();
        std::fs::write(dir.path().join(".env"), "SECRET=x").unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();

        let ignore = IgnoreController::new(dir.path());
        let ctx = workspace_context(dir.path(), &ignore, 3, 100);

        assert!(ctx.contains("visible.rs"));
        assert!(ctx.contains("src/"));
        assert!(!ctx.contains(".env"));
    }

    #[test]
    fn tree_max_depth() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b/c/d")).unwrap();
        std::fs::write(dir.path().join("a/b/c/d/deep.rs"), "").unwrap();

        let ignore = IgnoreController::permissive(dir.path());
        let ctx = workspace_context(dir.path(), &ignore, 1, 100);
        assert!(!ctx.contains("deep.rs"));
    }

    #[test]
    fn tree_truncates_at_max_entries() {
        let dir = TempDir::new().unwrap();
        for i in 0..20 {
            std::fs::write(dir.path().join(format!("file{i}.rs")), "").unwrap();
        }
        let ignore = IgnoreController::permissive(dir.path());
        let ctx = workspace_context(dir.path(), &ignore, 3, 5);
        assert!(ctx.contains("(truncated)"));
    }

    #[test]
    fn tree_skips_hidden_except_github() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".hidden")).unwrap();
        std::fs::create_dir(dir.path().join(".github")).unwrap();
        std::fs::write(dir.path().join("visible.rs"), "").unwrap();

        let ignore = IgnoreController::permissive(dir.path());
        let ctx = workspace_context(dir.path(), &ignore, 3, 100);
        assert!(!ctx.contains(".hidden"));
        assert!(ctx.contains(".github/"));
        assert!(ctx.contains("visible.rs"));
    }
}

#[cfg(test)]
mod error_handling_integration {
    use crate::error::{self, MehError};
    use crate::provider::common::ProviderError;
    use crate::state::config::AppConfig;
    use std::time::Duration;

    #[test]
    fn validate_config_catches_missing_key() {
        let config = AppConfig {
            provider: crate::state::config::ProviderConfig {
                anthropic: crate::state::config::ProviderSettings {
                    api_key_env: Some("MEH_NONEXISTENT_KEY_XYZ".to_string()),
                    api_key: None,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let errors = error::validate_config(&config);
        assert!(!errors.is_empty());
        assert!(matches!(&errors[0], MehError::NoApiKey { .. }));
    }

    #[test]
    fn map_auth_error() {
        let err: anyhow::Error = ProviderError::Auth("bad key".to_string()).into();
        let mapped = error::map_provider_error(&err, "anthropic");
        let msg = mapped.to_string();
        assert!(msg.contains("Authentication failed"));
        assert!(msg.contains("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn map_rate_limit_error() {
        let err: anyhow::Error = ProviderError::RateLimit {
            retry_after: Some(Duration::from_secs(5)),
        }
        .into();
        let mapped = error::map_provider_error(&err, "openai");
        let msg = mapped.to_string();
        assert!(msg.contains("Rate limited"));
        assert!(msg.contains("5s"));
    }

    #[test]
    fn map_server_error() {
        let err: anyhow::Error = ProviderError::Server {
            status: 500,
            message: "internal".to_string(),
        }
        .into();
        let mapped = error::map_provider_error(&err, "gemini");
        let msg = mapped.to_string();
        assert!(msg.contains("server error"));
        assert!(msg.contains("500"));
    }

    #[test]
    fn map_unknown_error_to_connection() {
        let err = anyhow::anyhow!("timeout");
        let mapped = error::map_provider_error(&err, "anthropic");
        assert!(matches!(mapped, MehError::ConnectionFailed { .. }));
    }

    #[test]
    fn all_error_variants_display() {
        let errors: Vec<MehError> = vec![
            MehError::AuthFailed {
                provider: "test".into(),
                env_var: "TEST_KEY".into(),
            },
            MehError::RateLimited {
                provider: "test".into(),
                retry_after: None,
            },
            MehError::ProviderServerError {
                provider: "test".into(),
                status: 503,
                message: "unavailable".into(),
            },
            MehError::ConnectionFailed {
                provider: "test".into(),
                url: "https://api.test.com".into(),
            },
            MehError::NoApiKey {
                provider: "test".into(),
                provider_lower: "test".into(),
                env_var: "TEST_KEY".into(),
            },
            MehError::ToolFailed {
                tool: "read_file".into(),
                reason: "not found".into(),
            },
            MehError::PermissionDenied {
                tool: "write".into(),
            },
            MehError::CommandTimeout {
                command: "npm install".into(),
                seconds: 300,
            },
            MehError::McpServerFailed {
                server: "fs".into(),
                reason: "not found".into(),
            },
            MehError::InvalidConfig {
                file: "config.toml".into(),
                reason: "bad".into(),
            },
            MehError::TaskNotFound {
                task_id: "abc".into(),
            },
            MehError::ContextWindowExceeded {
                used: 200_000,
                limit: 100_000,
            },
        ];
        for err in &errors {
            let msg = err.to_string();
            assert!(!msg.is_empty(), "Empty display for {err:?}");
        }
    }
}
