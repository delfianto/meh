//! Shell command validation against allow/deny patterns.

/// Result of command validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandValidation {
    /// Command is allowed to execute.
    Allowed,
    /// Command is denied.
    Denied { reason: String },
}

/// Glob-based command permission rules.
/// Commands are checked against deny patterns first, then allow patterns.
#[derive(Debug, Clone, Default)]
pub struct CommandPermissions {
    /// Glob patterns for commands that are always allowed.
    allow_patterns: Vec<glob::Pattern>,
    /// Glob patterns for commands that are always denied.
    deny_patterns: Vec<glob::Pattern>,
    /// Whether redirect operators (>, >>, <) are allowed.
    allow_redirects: bool,
}

impl CommandPermissions {
    /// Create a new empty permission set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create permissions with the given allow and deny pattern strings.
    /// Invalid patterns are silently skipped.
    pub fn from_patterns(allow: &[&str], deny: &[&str]) -> Self {
        Self {
            allow_patterns: allow
                .iter()
                .filter_map(|p| glob::Pattern::new(p).ok())
                .collect(),
            deny_patterns: deny
                .iter()
                .filter_map(|p| glob::Pattern::new(p).ok())
                .collect(),
            allow_redirects: false,
        }
    }

    /// Create permissions with full configuration.
    pub fn with_config(allow: &[String], deny: &[String], allow_redirects: bool) -> Self {
        Self {
            allow_patterns: allow
                .iter()
                .filter_map(|p| glob::Pattern::new(p).ok())
                .collect(),
            deny_patterns: deny
                .iter()
                .filter_map(|p| glob::Pattern::new(p).ok())
                .collect(),
            allow_redirects,
        }
    }

    /// Check whether a command is allowed (simple check, no segment splitting).
    pub fn is_allowed(&self, command: &str) -> bool {
        let trimmed = command.trim();

        if self.deny_patterns.iter().any(|p| p.matches(trimmed)) {
            return false;
        }

        self.allow_patterns.iter().any(|p| p.matches(trimmed))
    }

    /// Check whether a command is explicitly denied.
    pub fn is_denied(&self, command: &str) -> bool {
        let trimmed = command.trim();
        self.deny_patterns.iter().any(|p| p.matches(trimmed))
    }

    /// Validate a command string against all rules, including segment splitting,
    /// redirect detection, and backtick detection.
    pub fn validate(&self, command: &str) -> CommandValidation {
        if has_dangerous_backticks(command) {
            return CommandValidation::Denied {
                reason: "Command contains backticks outside single quotes".to_string(),
            };
        }

        let segments = split_command_segments(command);

        for segment in &segments {
            let trimmed = segment.trim();
            if trimmed.is_empty() {
                continue;
            }

            if !self.allow_redirects && has_redirect(trimmed) {
                return CommandValidation::Denied {
                    reason: format!("Redirect operators not allowed: {trimmed}"),
                };
            }

            if self.deny_patterns.iter().any(|p| p.matches(trimmed)) {
                return CommandValidation::Denied {
                    reason: format!("Command matches deny pattern: {trimmed}"),
                };
            }

            if !self.allow_patterns.is_empty()
                && !self.allow_patterns.iter().any(|p| p.matches(trimmed))
            {
                return CommandValidation::Denied {
                    reason: format!("Command not in allow list: {trimmed}"),
                };
            }
        }

        CommandValidation::Allowed
    }
}

/// Split a command string by shell operators: `&&`, `||`, `|`, `;`.
fn split_command_segments(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while let Some(ch) = chars.next() {
        if ch == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            current.push(ch);
        } else if ch == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            current.push(ch);
        } else if in_single_quote || in_double_quote {
            current.push(ch);
        } else if (ch == '&' && chars.peek() == Some(&'&'))
            || (ch == '|' && chars.peek() == Some(&'|'))
        {
            chars.next();
            segments.push(std::mem::take(&mut current));
        } else if ch == '|' || ch == ';' {
            segments.push(std::mem::take(&mut current));
        } else {
            current.push(ch);
        }
    }

    if !current.trim().is_empty() {
        segments.push(current);
    }

    segments
}

/// Check for backticks outside single quotes.
fn has_dangerous_backticks(command: &str) -> bool {
    let mut in_single_quote = false;

    for ch in command.chars() {
        if ch == '\'' {
            in_single_quote = !in_single_quote;
        } else if ch == '`' && !in_single_quote {
            return true;
        }
    }

    false
}

/// Check if a command segment contains redirect operators outside quotes.
fn has_redirect(segment: &str) -> bool {
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut chars = segment.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
        } else if ch == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
        } else if !in_single_quote && !in_double_quote && (ch == '>' || ch == '<') {
            // Skip >> (still a redirect)
            if ch == '>' && chars.peek() == Some(&'>') {
                chars.next();
            }
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_permissions() {
        let perms = CommandPermissions::new();
        assert!(!perms.is_allowed("ls"));
        assert!(!perms.is_denied("ls"));
    }

    #[test]
    fn test_allow_pattern() {
        let perms = CommandPermissions::from_patterns(&["cargo *", "git *"], &[]);
        assert!(perms.is_allowed("cargo test"));
        assert!(perms.is_allowed("cargo build"));
        assert!(perms.is_allowed("git status"));
        assert!(!perms.is_allowed("rm -rf /"));
    }

    #[test]
    fn test_deny_overrides_allow() {
        let perms = CommandPermissions::from_patterns(&["cargo *"], &["cargo publish*"]);
        assert!(perms.is_allowed("cargo test"));
        assert!(!perms.is_allowed("cargo publish"));
    }

    #[test]
    fn test_deny_pattern() {
        let perms = CommandPermissions::from_patterns(&[], &["rm *"]);
        assert!(perms.is_denied("rm -rf /"));
        assert!(!perms.is_denied("ls"));
    }

    #[test]
    fn test_whitespace_trimming() {
        let perms = CommandPermissions::from_patterns(&["cargo *"], &[]);
        assert!(perms.is_allowed("  cargo test  "));
    }

    #[test]
    fn test_invalid_patterns_skipped() {
        let perms = CommandPermissions::from_patterns(&["[invalid", "cargo *"], &[]);
        assert!(perms.is_allowed("cargo test"));
    }

    #[test]
    fn test_validate_no_rules_allows_all() {
        let perms = CommandPermissions::default();
        assert_eq!(perms.validate("ls -la"), CommandValidation::Allowed);
        assert_eq!(perms.validate("rm -rf /"), CommandValidation::Allowed);
    }

    #[test]
    fn test_validate_allow_rules() {
        let perms = CommandPermissions::with_config(
            &vec!["git *".to_string(), "cargo *".to_string(), "ls".to_string()],
            &vec![],
            false,
        );
        assert_eq!(perms.validate("git status"), CommandValidation::Allowed);
        assert_eq!(perms.validate("cargo test"), CommandValidation::Allowed);
        assert_eq!(perms.validate("ls"), CommandValidation::Allowed);
        assert!(matches!(
            perms.validate("rm -rf /"),
            CommandValidation::Denied { .. }
        ));
    }

    #[test]
    fn test_validate_deny_takes_precedence() {
        let perms = CommandPermissions::with_config(
            &vec!["*".to_string()],
            &vec!["rm *".to_string(), "sudo *".to_string()],
            false,
        );
        assert_eq!(perms.validate("ls"), CommandValidation::Allowed);
        assert!(matches!(
            perms.validate("rm -rf /"),
            CommandValidation::Denied { .. }
        ));
        assert!(matches!(
            perms.validate("sudo apt install"),
            CommandValidation::Denied { .. }
        ));
    }

    #[test]
    fn test_redirect_blocked_by_default() {
        let perms = CommandPermissions::with_config(&vec!["*".to_string()], &vec![], false);
        assert!(matches!(
            perms.validate("echo hello > file.txt"),
            CommandValidation::Denied { .. }
        ));
        assert!(matches!(
            perms.validate("cat < input.txt"),
            CommandValidation::Denied { .. }
        ));
    }

    #[test]
    fn test_redirect_allowed_when_configured() {
        let perms = CommandPermissions::with_config(&vec!["*".to_string()], &vec![], true);
        assert_eq!(
            perms.validate("echo hello > file.txt"),
            CommandValidation::Allowed
        );
    }

    #[test]
    fn test_pipe_splits_segments() {
        let perms = CommandPermissions::with_config(
            &vec!["cat *".to_string(), "grep *".to_string()],
            &vec![],
            false,
        );
        assert_eq!(
            perms.validate("cat file.txt | grep pattern"),
            CommandValidation::Allowed
        );
    }

    #[test]
    fn test_chained_commands() {
        let perms = CommandPermissions::with_config(
            &vec!["git *".to_string(), "echo *".to_string()],
            &vec![],
            false,
        );
        assert_eq!(
            perms.validate("git add . && git commit -m 'test'"),
            CommandValidation::Allowed
        );
        assert!(matches!(
            perms.validate("git add . && rm -rf /"),
            CommandValidation::Denied { .. }
        ));
    }

    #[test]
    fn test_dangerous_backticks() {
        let perms = CommandPermissions::with_config(&vec!["echo *".to_string()], &vec![], false);
        assert!(matches!(
            perms.validate("echo `rm -rf /`"),
            CommandValidation::Denied { .. }
        ));
    }

    #[test]
    fn test_backticks_safe_in_single_quotes() {
        let perms = CommandPermissions::with_config(&vec!["echo *".to_string()], &vec![], false);
        assert_eq!(
            perms.validate("echo 'hello `world`'"),
            CommandValidation::Allowed
        );
    }

    #[test]
    fn test_semicolon_splits() {
        let perms = CommandPermissions::with_config(&vec!["echo *".to_string()], &vec![], false);
        assert_eq!(perms.validate("echo a; echo b"), CommandValidation::Allowed);
        assert!(matches!(
            perms.validate("echo a; rm -rf /"),
            CommandValidation::Denied { .. }
        ));
    }

    #[test]
    fn test_has_redirect() {
        assert!(has_redirect("echo hello > file.txt"));
        assert!(has_redirect("cat >> file.txt"));
        assert!(has_redirect("cat < input.txt"));
        assert!(!has_redirect("echo hello"));
        assert!(!has_redirect("echo '>' hello"));
    }

    #[test]
    fn test_split_segments() {
        let segs = split_command_segments("a && b || c | d ; e");
        assert_eq!(segs.len(), 5);
    }
}
