# STEP 35 — Slash Commands

## Objective
Implement slash command parsing and dispatch for built-in commands (/help, /clear, /compact, /history, /settings).

## Prerequisites
- STEP 03 (TUI input), STEP 04 (controller)

## Detailed Instructions

### 35.1 Command parsing (`src/commands/mod.rs`)

```rust
//! Slash command parsing and dispatch.

#[derive(Debug, Clone, PartialEq)]
pub enum SlashCommand {
    Help,
    Clear,
    Compact,           // Trigger conversation summarization
    History,           // Show task history
    Settings,          // Open settings view
    NewTask,           // Start a new task
    Mode(String),      // Switch mode: /plan or /act
    Model(String),     // Switch model: /model claude-sonnet-4
    Yolo,              // Toggle YOLO mode
}

/// Parse a slash command from user input. Returns None if not a command.
pub fn parse_slash_command(input: &str) -> Option<(SlashCommand, String)> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') { return None; }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let cmd = parts[0];
    let args = parts.get(1).unwrap_or(&"").to_string();

    match cmd {
        "/help" | "/h" => Some((SlashCommand::Help, args)),
        "/clear" => Some((SlashCommand::Clear, args)),
        "/compact" | "/smol" => Some((SlashCommand::Compact, args)),
        "/history" => Some((SlashCommand::History, args)),
        "/settings" => Some((SlashCommand::Settings, args)),
        "/new" | "/newtask" => Some((SlashCommand::NewTask, args)),
        "/plan" => Some((SlashCommand::Mode("plan".to_string()), args)),
        "/act" => Some((SlashCommand::Mode("act".to_string()), args)),
        "/model" => Some((SlashCommand::Model(args.clone()), String::new())),
        "/yolo" => Some((SlashCommand::Yolo, args)),
        _ => None, // Unknown command — treat as regular input
    }
}

/// Get help text for all commands.
pub fn help_text() -> String {
    r#"Available commands:
  /help, /h       Show this help message
  /clear          Clear chat display (preserves API history)
  /compact, /smol Summarize conversation to free context space
  /history        Show task history
  /settings       Open settings view
  /new, /newtask  Start a new task
  /plan           Switch to Plan mode
  /act            Switch to Act mode
  /model <name>   Switch model (e.g., /model claude-sonnet-4)
  /yolo           Toggle YOLO (auto-approve) mode"#.to_string()
}
```

### 35.2 Command handlers in Controller (`src/controller/commands.rs`)

Each command triggers a specific action:

```rust
use crate::commands::SlashCommand;

impl Controller {
    pub async fn handle_slash_command(&mut self, cmd: SlashCommand, args: String) -> anyhow::Result<()> {
        match cmd {
            SlashCommand::Help => {
                let text = crate::commands::help_text();
                self.ui_tx.send(UiUpdate::AppendMessage {
                    role: ChatRole::System,
                    content: text,
                })?;
            }

            SlashCommand::Clear => {
                self.ui_tx.send(UiUpdate::ClearChat)?;
            }

            SlashCommand::Compact => {
                // Trigger conversation summarization (STEP 31)
                if let Some(agent) = &mut self.agent {
                    agent.force_summarize().await?;
                }
                self.ui_tx.send(UiUpdate::AppendMessage {
                    role: ChatRole::System,
                    content: "Conversation compacted.".to_string(),
                })?;
            }

            SlashCommand::History => {
                self.ui_tx.send(UiUpdate::ShowView(View::TaskHistory))?;
            }

            SlashCommand::Settings => {
                self.ui_tx.send(UiUpdate::ShowView(View::Settings))?;
            }

            SlashCommand::NewTask => {
                self.end_current_task().await?;
                self.ui_tx.send(UiUpdate::ClearChat)?;
                self.ui_tx.send(UiUpdate::AppendMessage {
                    role: ChatRole::System,
                    content: "New task started. What would you like to do?".to_string(),
                })?;
            }

            SlashCommand::Mode(mode) => {
                let new_mode = match mode.as_str() {
                    "plan" => Mode::Plan,
                    "act" => Mode::Act,
                    _ => {
                        self.ui_tx.send(UiUpdate::AppendMessage {
                            role: ChatRole::System,
                            content: format!("Unknown mode: {mode}. Use /plan or /act."),
                        })?;
                        return Ok(());
                    }
                };
                self.state.set_mode(new_mode);
                self.ui_tx.send(UiUpdate::StatusUpdate {
                    mode: Some(new_mode),
                    ..Default::default()
                })?;
                self.ui_tx.send(UiUpdate::AppendMessage {
                    role: ChatRole::System,
                    content: format!("Switched to {} mode.", mode),
                })?;
            }

            SlashCommand::Model(model_name) => {
                if model_name.is_empty() {
                    let current = &self.config.model_id;
                    self.ui_tx.send(UiUpdate::AppendMessage {
                        role: ChatRole::System,
                        content: format!("Current model: {current}"),
                    })?;
                } else {
                    self.config.model_id = model_name.clone();
                    self.ui_tx.send(UiUpdate::AppendMessage {
                        role: ChatRole::System,
                        content: format!("Model changed to: {model_name}"),
                    })?;
                }
            }

            SlashCommand::Yolo => {
                let new_val = !self.state.yolo_mode();
                self.state.set_yolo_mode(new_val);
                let status = if new_val { "enabled" } else { "disabled" };
                self.ui_tx.send(UiUpdate::AppendMessage {
                    role: ChatRole::System,
                    content: format!("YOLO mode {status}."),
                })?;
                self.ui_tx.send(UiUpdate::StatusUpdate {
                    yolo: Some(new_val),
                    ..Default::default()
                })?;
            }
        }
        Ok(())
    }
}
```

### 35.3 Integration with input handling

In TUI input handler, intercept before sending to controller:
```rust
if let Some(text) = input.handle_key(key) {
    if let Some((cmd, args)) = parse_slash_command(&text) {
        ctrl_tx.send(ControllerMessage::SlashCommand(cmd, args))?;
    } else {
        ctrl_tx.send(ControllerMessage::UserSubmit { text, images: vec![] })?;
    }
}
```

### 35.4 Tab completion for slash commands

Provide basic tab completion for command names:
```rust
/// Get completions for partial slash command input.
pub fn complete_command(partial: &str) -> Vec<&'static str> {
    let commands = [
        "/help", "/clear", "/compact", "/smol", "/history",
        "/settings", "/new", "/newtask", "/plan", "/act",
        "/model", "/yolo",
    ];
    commands.iter()
        .filter(|c| c.starts_with(partial))
        .copied()
        .collect()
}
```

## Tests

```rust
#[cfg(test)]
mod command_tests {
    use super::*;

    #[test]
    fn test_parse_help() {
        let (cmd, args) = parse_slash_command("/help").unwrap();
        assert_eq!(cmd, SlashCommand::Help);
        assert!(args.is_empty());
    }

    #[test]
    fn test_parse_help_alias() {
        let (cmd, _) = parse_slash_command("/h").unwrap();
        assert_eq!(cmd, SlashCommand::Help);
    }

    #[test]
    fn test_parse_compact_alias() {
        let (cmd, _) = parse_slash_command("/smol").unwrap();
        assert_eq!(cmd, SlashCommand::Compact);
    }

    #[test]
    fn test_parse_model_with_args() {
        let (cmd, _) = parse_slash_command("/model claude-sonnet-4").unwrap();
        assert_eq!(cmd, SlashCommand::Model("claude-sonnet-4".to_string()));
    }

    #[test]
    fn test_parse_model_no_args() {
        let (cmd, _) = parse_slash_command("/model").unwrap();
        assert_eq!(cmd, SlashCommand::Model(String::new()));
    }

    #[test]
    fn test_parse_mode_plan() {
        let (cmd, _) = parse_slash_command("/plan").unwrap();
        assert_eq!(cmd, SlashCommand::Mode("plan".to_string()));
    }

    #[test]
    fn test_parse_mode_act() {
        let (cmd, _) = parse_slash_command("/act").unwrap();
        assert_eq!(cmd, SlashCommand::Mode("act".to_string()));
    }

    #[test]
    fn test_parse_unknown_returns_none() {
        assert!(parse_slash_command("/foobar").is_none());
    }

    #[test]
    fn test_non_slash_returns_none() {
        assert!(parse_slash_command("hello world").is_none());
    }

    #[test]
    fn test_just_slash_returns_none() {
        assert!(parse_slash_command("/").is_none());
    }

    #[test]
    fn test_parse_with_extra_whitespace() {
        let (cmd, _) = parse_slash_command("  /help  ").unwrap();
        assert_eq!(cmd, SlashCommand::Help);
    }

    #[test]
    fn test_parse_yolo() {
        let (cmd, _) = parse_slash_command("/yolo").unwrap();
        assert_eq!(cmd, SlashCommand::Yolo);
    }

    #[test]
    fn test_parse_new_aliases() {
        let (cmd1, _) = parse_slash_command("/new").unwrap();
        let (cmd2, _) = parse_slash_command("/newtask").unwrap();
        assert_eq!(cmd1, SlashCommand::NewTask);
        assert_eq!(cmd2, SlashCommand::NewTask);
    }

    #[test]
    fn test_complete_command() {
        let completions = complete_command("/h");
        assert!(completions.contains(&"/help"));
        assert!(completions.contains(&"/history"));
    }

    #[test]
    fn test_complete_command_exact() {
        let completions = complete_command("/yolo");
        assert_eq!(completions, vec!["/yolo"]);
    }

    #[test]
    fn test_complete_command_no_match() {
        let completions = complete_command("/zzz");
        assert!(completions.is_empty());
    }

    #[test]
    fn test_help_text_non_empty() {
        let text = help_text();
        assert!(text.contains("/help"));
        assert!(text.contains("/compact"));
        assert!(text.contains("/model"));
    }
}
```

## Acceptance Criteria
- [x] All listed slash commands parse correctly
- [x] Command aliases work (/h, /smol, /newtask)
- [x] Unknown /commands treated as regular input (returns None)
- [x] Each command dispatched to correct handler in Controller
- [x] /help displays available commands
- [x] /model with no args shows current model
- [x] /model with args switches model
- [x] /yolo toggles YOLO mode
- [x] Tab completion for partial command names
- [x] `cargo clippy -- -D warnings` passes
- [x] All tests pass
