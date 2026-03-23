//! Slash command parsing, dispatch, and tab completion.
//!
//! Users type `/command` in the input to trigger built-in actions.
//! Commands are intercepted in the TUI input handler before being
//! sent to the controller. Unknown `/` prefixed input is passed
//! through as regular text.
//!
//! ```text
//!   User types "/help"
//!         │
//!         ▼
//!   parse_slash_command("/help")
//!         │
//!         ├── Some(SlashCommand::Help, "")
//!         │         │
//!         │         ▼
//!         │   ControllerMessage::SlashCommand(Help, "")
//!         │         │
//!         │         ▼
//!         │   Controller::handle_slash_command()
//!         │
//!         └── None (not a command) → UserSubmit
//! ```

/// Recognized slash commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    /// Show help text listing all commands.
    Help,
    /// Clear the chat display (preserves API history).
    Clear,
    /// Trigger conversation summarization to free context.
    Compact,
    /// Show task history view.
    History,
    /// Open settings view.
    Settings,
    /// Start a new task (end current one).
    NewTask,
    /// Switch mode: `/plan` or `/act`.
    Mode(String),
    /// Switch model: `/model claude-sonnet-4`.
    Model(String),
    /// Toggle YOLO (auto-approve) mode.
    Yolo,
}

/// Parse a slash command from user input.
///
/// Returns `None` if input is not a command or is an unknown command.
/// Unknown `/` commands return `None` so they're treated as regular input.
pub fn parse_slash_command(input: &str) -> Option<(SlashCommand, String)> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') || trimmed.len() < 2 {
        return None;
    }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let cmd = parts[0];
    let args = parts.get(1).map_or_else(String::new, |s| (*s).to_string());

    match cmd {
        "/help" | "/h" => Some((SlashCommand::Help, args)),
        "/clear" => Some((SlashCommand::Clear, args)),
        "/compact" | "/smol" => Some((SlashCommand::Compact, args)),
        "/history" => Some((SlashCommand::History, args)),
        "/settings" => Some((SlashCommand::Settings, args)),
        "/new" | "/newtask" => Some((SlashCommand::NewTask, args)),
        "/plan" => Some((SlashCommand::Mode("plan".to_string()), args)),
        "/act" => Some((SlashCommand::Mode("act".to_string()), args)),
        "/model" => Some((SlashCommand::Model(args), String::new())),
        "/yolo" => Some((SlashCommand::Yolo, args)),
        _ => None,
    }
}

/// Get help text listing all available commands.
pub fn help_text() -> String {
    "Available commands:\n  \
     /help, /h       Show this help message\n  \
     /clear          Clear chat display (preserves API history)\n  \
     /compact, /smol Summarize conversation to free context space\n  \
     /history        Show task history\n  \
     /settings       Open settings view\n  \
     /new, /newtask  Start a new task\n  \
     /plan           Switch to Plan mode\n  \
     /act            Switch to Act mode\n  \
     /model <name>   Switch model (e.g., /model claude-sonnet-4)\n  \
     /yolo           Toggle YOLO (auto-approve) mode"
        .to_string()
}

/// Get tab completions for a partial slash command.
pub fn complete_command(partial: &str) -> Vec<&'static str> {
    const COMMANDS: &[&str] = &[
        "/help",
        "/clear",
        "/compact",
        "/smol",
        "/history",
        "/settings",
        "/new",
        "/newtask",
        "/plan",
        "/act",
        "/model",
        "/yolo",
    ];
    COMMANDS
        .iter()
        .filter(|c| c.starts_with(partial))
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_help() {
        let (cmd, args) = parse_slash_command("/help").unwrap();
        assert_eq!(cmd, SlashCommand::Help);
        assert!(args.is_empty());
    }

    #[test]
    fn parse_help_alias() {
        let (cmd, _) = parse_slash_command("/h").unwrap();
        assert_eq!(cmd, SlashCommand::Help);
    }

    #[test]
    fn parse_compact_alias() {
        let (cmd, _) = parse_slash_command("/smol").unwrap();
        assert_eq!(cmd, SlashCommand::Compact);
    }

    #[test]
    fn parse_model_with_args() {
        let (cmd, _) = parse_slash_command("/model claude-sonnet-4").unwrap();
        assert_eq!(cmd, SlashCommand::Model("claude-sonnet-4".to_string()));
    }

    #[test]
    fn parse_model_no_args() {
        let (cmd, _) = parse_slash_command("/model").unwrap();
        assert_eq!(cmd, SlashCommand::Model(String::new()));
    }

    #[test]
    fn parse_mode_plan() {
        let (cmd, _) = parse_slash_command("/plan").unwrap();
        assert_eq!(cmd, SlashCommand::Mode("plan".to_string()));
    }

    #[test]
    fn parse_mode_act() {
        let (cmd, _) = parse_slash_command("/act").unwrap();
        assert_eq!(cmd, SlashCommand::Mode("act".to_string()));
    }

    #[test]
    fn parse_unknown_returns_none() {
        assert!(parse_slash_command("/foobar").is_none());
    }

    #[test]
    fn non_slash_returns_none() {
        assert!(parse_slash_command("hello world").is_none());
    }

    #[test]
    fn just_slash_returns_none() {
        assert!(parse_slash_command("/").is_none());
    }

    #[test]
    fn parse_with_extra_whitespace() {
        let (cmd, _) = parse_slash_command("  /help  ").unwrap();
        assert_eq!(cmd, SlashCommand::Help);
    }

    #[test]
    fn parse_yolo() {
        let (cmd, _) = parse_slash_command("/yolo").unwrap();
        assert_eq!(cmd, SlashCommand::Yolo);
    }

    #[test]
    fn parse_new_aliases() {
        let (cmd1, _) = parse_slash_command("/new").unwrap();
        let (cmd2, _) = parse_slash_command("/newtask").unwrap();
        assert_eq!(cmd1, SlashCommand::NewTask);
        assert_eq!(cmd2, SlashCommand::NewTask);
    }

    #[test]
    fn parse_clear() {
        let (cmd, _) = parse_slash_command("/clear").unwrap();
        assert_eq!(cmd, SlashCommand::Clear);
    }

    #[test]
    fn parse_history() {
        let (cmd, _) = parse_slash_command("/history").unwrap();
        assert_eq!(cmd, SlashCommand::History);
    }

    #[test]
    fn parse_settings() {
        let (cmd, _) = parse_slash_command("/settings").unwrap();
        assert_eq!(cmd, SlashCommand::Settings);
    }

    #[test]
    fn complete_command_partial() {
        let completions = complete_command("/h");
        assert!(completions.contains(&"/help"));
        assert!(completions.contains(&"/history"));
    }

    #[test]
    fn complete_command_exact() {
        let completions = complete_command("/yolo");
        assert_eq!(completions, vec!["/yolo"]);
    }

    #[test]
    fn complete_command_no_match() {
        let completions = complete_command("/zzz");
        assert!(completions.is_empty());
    }

    #[test]
    fn complete_command_all() {
        let completions = complete_command("/");
        assert!(completions.len() >= 10);
    }

    #[test]
    fn help_text_non_empty() {
        let text = help_text();
        assert!(text.contains("/help"));
        assert!(text.contains("/compact"));
        assert!(text.contains("/model"));
        assert!(text.contains("/yolo"));
    }

    #[test]
    fn empty_input_returns_none() {
        assert!(parse_slash_command("").is_none());
    }

    #[test]
    fn whitespace_only_returns_none() {
        assert!(parse_slash_command("   ").is_none());
    }
}
