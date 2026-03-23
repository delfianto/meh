//! User rules loading from `.mehrules` files and `~/.meh/rules/` directory.
//!
//! Rules are custom instructions injected into the system prompt. They can
//! be unconditional (always active) or conditional on file path globs via
//! YAML frontmatter. Rules are loaded from two locations:
//!
//! 1. **Global**: `~/.meh/rules/` directory (`.md`, `.txt`, extensionless files)
//! 2. **Workspace**: `.mehrules` file or directory in the project root
//!
//! Files may include YAML frontmatter with `paths:` conditions:
//! ```text
//! ---
//! paths:
//!   - "src/**/*.rs"
//! ---
//! Use Rust 2024 edition idioms.
//! ```

use std::path::{Path, PathBuf};

/// A single user rule loaded from disk.
#[derive(Debug, Clone)]
pub struct UserRule {
    /// File the rule was loaded from.
    pub source: PathBuf,
    /// Rule text content (after frontmatter is stripped).
    pub content: String,
    /// Optional path conditions from YAML frontmatter.
    pub conditions: Option<RuleConditions>,
    /// Whether this rule is active.
    pub enabled: bool,
}

/// Conditions that control when a rule applies.
#[derive(Debug, Clone)]
pub struct RuleConditions {
    /// Glob patterns — rule applies only when active paths match.
    pub paths: Vec<String>,
}

/// Load rules from global (`~/.meh/rules/`) and workspace (`.mehrules`) locations.
pub fn load_rules(workspace_root: &Path) -> Vec<UserRule> {
    let mut rules = Vec::new();

    if let Some(dir) = dirs::home_dir().map(|h| h.join(".meh/rules")) {
        if dir.is_dir() {
            rules.extend(load_rules_from_dir(&dir));
        }
    }

    let local_path = workspace_root.join(".mehrules");
    if local_path.is_file() {
        if let Some(rule) = parse_rule_file(&local_path) {
            rules.push(rule);
        }
    } else if local_path.is_dir() {
        rules.extend(load_rules_from_dir(&local_path));
    }

    rules
}

/// Load all rule files from a directory (non-recursive).
///
/// Only `.md`, `.txt`, and extensionless files are loaded.
fn load_rules_from_dir(dir: &Path) -> Vec<UserRule> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut rules = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let dominated = path.extension().and_then(|e| e.to_str());
        match dominated {
            Some("md" | "txt") | None => {
                if let Some(rule) = parse_rule_file(&path) {
                    rules.push(rule);
                }
            }
            _ => {}
        }
    }
    rules
}

/// Parse a single rule file with optional YAML frontmatter.
fn parse_rule_file(path: &Path) -> Option<UserRule> {
    let raw = std::fs::read_to_string(path).ok()?;
    let (conditions, content) = extract_frontmatter(&raw);

    if content.is_empty() {
        return None;
    }

    Some(UserRule {
        source: path.to_path_buf(),
        content,
        conditions,
        enabled: true,
    })
}

/// Extract YAML frontmatter (delimited by `---`) from raw file content.
///
/// Returns `(conditions, body)`. If no frontmatter found, returns `(None, raw)`.
fn extract_frontmatter(raw: &str) -> (Option<RuleConditions>, String) {
    if !raw.starts_with("---\n") && !raw.starts_with("---\r\n") {
        return (None, raw.trim().to_string());
    }

    let after_opening = &raw[3..];
    let Some(end_pos) = after_opening.find("\n---") else {
        return (None, raw.trim().to_string());
    };

    let yaml = after_opening[1..end_pos].trim();
    let body_start = end_pos + 4; // skip "\n---"
    let body = if body_start < after_opening.len() {
        after_opening[body_start..].trim().to_string()
    } else {
        String::new()
    };

    let conditions = parse_frontmatter_yaml(yaml);
    (Some(conditions), body)
}

/// Parse simple YAML for `paths:` list.
fn parse_frontmatter_yaml(yaml: &str) -> RuleConditions {
    let mut paths = Vec::new();
    let mut in_paths = false;
    for line in yaml.lines() {
        let trimmed = line.trim();
        if trimmed == "paths:" {
            in_paths = true;
            continue;
        }
        if in_paths {
            if let Some(path) = trimmed.strip_prefix("- ") {
                let cleaned = path.trim().trim_matches('"').trim_matches('\'');
                paths.push(cleaned.to_string());
            } else {
                in_paths = false;
            }
        }
    }
    RuleConditions { paths }
}

/// Evaluate whether a rule applies based on its conditions and active file paths.
pub fn evaluate_rule(rule: &UserRule, active_paths: &[&str]) -> bool {
    if !rule.enabled {
        return false;
    }
    match &rule.conditions {
        None => true,
        Some(conds) if conds.paths.is_empty() => true,
        Some(conds) => active_paths
            .iter()
            .any(|path| conds.paths.iter().any(|pattern| glob_match(pattern, path))),
    }
}

/// Match a path against a glob pattern.
fn glob_match(pattern: &str, path: &str) -> bool {
    glob::Pattern::new(pattern).is_ok_and(|p| p.matches(path))
}

/// Format applicable rules for system prompt injection.
///
/// Returns an empty string if no rules apply.
pub fn rules_to_prompt(rules: &[UserRule], active_paths: &[&str]) -> String {
    let applicable: Vec<&UserRule> = rules
        .iter()
        .filter(|r| evaluate_rule(r, active_paths))
        .collect();
    if applicable.is_empty() {
        return String::new();
    }
    let mut s = String::from("# User Rules\n\n");
    for rule in applicable {
        s.push_str(&rule.content);
        s.push_str("\n\n");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn load_simple_rule_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".mehrules"), "Always use snake_case.\n").unwrap();
        let rules = load_rules(dir.path());
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].content, "Always use snake_case.");
        assert!(rules[0].conditions.is_none());
    }

    #[test]
    fn load_rules_directory() {
        let dir = TempDir::new().unwrap();
        let rules_dir = dir.path().join(".mehrules");
        std::fs::create_dir(&rules_dir).unwrap();
        std::fs::write(rules_dir.join("style.md"), "Use 4-space indentation.").unwrap();
        std::fs::write(rules_dir.join("naming.txt"), "Use descriptive names.").unwrap();
        std::fs::write(rules_dir.join("ignored.rs"), "This should be skipped.").unwrap();
        let rules = load_rules(dir.path());
        assert_eq!(rules.len(), 2);
    }

    #[test]
    fn load_rules_extensionless_files() {
        let dir = TempDir::new().unwrap();
        let rules_dir = dir.path().join(".mehrules");
        std::fs::create_dir(&rules_dir).unwrap();
        std::fs::write(rules_dir.join("general"), "Be concise.").unwrap();
        let rules = load_rules(dir.path());
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].content, "Be concise.");
    }

    #[test]
    fn yaml_frontmatter_parsing() {
        let dir = TempDir::new().unwrap();
        let content = "---\npaths:\n  - \"src/**/*.rs\"\n  - \"tests/**/*.rs\"\n---\nUse Rust 2024 edition idioms.\n";
        std::fs::write(dir.path().join(".mehrules"), content).unwrap();
        let rules = load_rules(dir.path());
        assert_eq!(rules.len(), 1);
        let conds = rules[0].conditions.as_ref().unwrap();
        assert_eq!(conds.paths.len(), 2);
        assert_eq!(conds.paths[0], "src/**/*.rs");
        assert_eq!(rules[0].content, "Use Rust 2024 edition idioms.");
    }

    #[test]
    fn yaml_frontmatter_single_quoted() {
        let content = "---\npaths:\n  - 'src/*.rs'\n---\nContent here.\n";
        let (conds, body) = extract_frontmatter(content);
        assert_eq!(conds.unwrap().paths, vec!["src/*.rs"]);
        assert_eq!(body, "Content here.");
    }

    #[test]
    fn yaml_frontmatter_unquoted() {
        let content = "---\npaths:\n  - src/*.rs\n---\nContent.\n";
        let (conds, body) = extract_frontmatter(content);
        assert_eq!(conds.unwrap().paths, vec!["src/*.rs"]);
        assert_eq!(body, "Content.");
    }

    #[test]
    fn no_frontmatter() {
        let content = "Just plain rules text.\nNo YAML here.";
        let (conds, body) = extract_frontmatter(content);
        assert!(conds.is_none());
        assert_eq!(body, "Just plain rules text.\nNo YAML here.");
    }

    #[test]
    fn empty_rule_file_skipped() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".mehrules"), "").unwrap();
        let rules = load_rules(dir.path());
        assert!(rules.is_empty());
    }

    #[test]
    fn whitespace_only_file_skipped() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".mehrules"), "   \n\n  ").unwrap();
        let rules = load_rules(dir.path());
        assert!(rules.is_empty());
    }

    #[test]
    fn evaluate_unconditional_rule() {
        let rule = UserRule {
            source: PathBuf::from("test"),
            content: "test".to_string(),
            conditions: None,
            enabled: true,
        };
        assert!(evaluate_rule(&rule, &[]));
        assert!(evaluate_rule(&rule, &["src/main.rs"]));
    }

    #[test]
    fn evaluate_conditional_rule_match() {
        let rule = UserRule {
            source: PathBuf::from("test"),
            content: "test".to_string(),
            conditions: Some(RuleConditions {
                paths: vec!["src/**/*.rs".to_string()],
            }),
            enabled: true,
        };
        assert!(evaluate_rule(&rule, &["src/main.rs"]));
        assert!(!evaluate_rule(&rule, &["docs/README.md"]));
    }

    #[test]
    fn evaluate_disabled_rule() {
        let rule = UserRule {
            source: PathBuf::from("test"),
            content: "test".to_string(),
            conditions: None,
            enabled: false,
        };
        assert!(!evaluate_rule(&rule, &[]));
    }

    #[test]
    fn evaluate_empty_conditions_always_applies() {
        let rule = UserRule {
            source: PathBuf::from("test"),
            content: "test".to_string(),
            conditions: Some(RuleConditions { paths: vec![] }),
            enabled: true,
        };
        assert!(evaluate_rule(&rule, &[]));
    }

    #[test]
    fn rules_to_prompt_empty() {
        let rules: Vec<UserRule> = vec![];
        let result = rules_to_prompt(&rules, &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn rules_to_prompt_with_applicable_rules() {
        let rules = vec![
            UserRule {
                source: PathBuf::from("a"),
                content: "Rule A".to_string(),
                conditions: None,
                enabled: true,
            },
            UserRule {
                source: PathBuf::from("b"),
                content: "Rule B".to_string(),
                conditions: Some(RuleConditions {
                    paths: vec!["*.py".to_string()],
                }),
                enabled: true,
            },
        ];
        let result = rules_to_prompt(&rules, &["main.rs"]);
        assert!(result.contains("Rule A"));
        assert!(!result.contains("Rule B"));
    }

    #[test]
    fn rules_to_prompt_header() {
        let rules = vec![UserRule {
            source: PathBuf::from("a"),
            content: "Some rule.".to_string(),
            conditions: None,
            enabled: true,
        }];
        let result = rules_to_prompt(&rules, &[]);
        assert!(result.starts_with("# User Rules"));
    }

    #[test]
    fn glob_match_basic() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("src/**/*.rs", "src/lib.rs"));
        assert!(!glob_match("*.rs", "main.py"));
    }

    #[test]
    fn glob_match_star_star() {
        assert!(glob_match("src/**/*.rs", "src/deep/nested/file.rs"));
    }

    #[test]
    fn glob_match_invalid_pattern() {
        assert!(!glob_match("[invalid", "anything"));
    }

    #[test]
    fn no_mehrules_returns_empty() {
        let dir = TempDir::new().unwrap();
        let rules = load_rules(dir.path());
        assert!(rules.is_empty());
    }

    #[test]
    fn load_rules_nonexistent_dir_safe() {
        let rules = load_rules(Path::new("/nonexistent/path"));
        assert!(rules.is_empty());
    }

    #[test]
    fn multiple_conditions_any_matches() {
        let rule = UserRule {
            source: PathBuf::from("test"),
            content: "test".to_string(),
            conditions: Some(RuleConditions {
                paths: vec!["*.rs".to_string(), "*.py".to_string()],
            }),
            enabled: true,
        };
        assert!(evaluate_rule(&rule, &["main.py"]));
        assert!(evaluate_rule(&rule, &["main.rs"]));
        assert!(!evaluate_rule(&rule, &["main.go"]));
    }
}
