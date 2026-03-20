//! Shell command validation against allow/deny patterns.

/// Glob-based command permission rules.
/// Commands are checked against allow patterns first, then deny patterns.
#[derive(Debug, Clone, Default)]
pub struct CommandPermissions {
    /// Glob patterns for commands that are always allowed.
    allow_patterns: Vec<glob::Pattern>,
    /// Glob patterns for commands that are always denied.
    deny_patterns: Vec<glob::Pattern>,
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
        }
    }

    /// Check whether a command is allowed.
    /// Returns `true` if allowed, `false` if denied or not matched.
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
}
