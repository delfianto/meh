# STEP 34 — User Rules System (.mehrules)

## Objective
Implement the user rules system that loads custom instructions from `.mehrules` files (single file or directory with conditional rules). Rules are injected into the system prompt.

## Prerequisites
- STEP 33 (environment detection for path conditionals)
- STEP 29 (file watching for hot-reload)

## Detailed Instructions

### 34.1 Rule loading (`src/ignore/rules.rs`)

```rust
//! User rules loading with YAML frontmatter conditionals.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct UserRule {
    pub source: PathBuf,
    pub content: String,
    pub conditions: Option<RuleConditions>,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct RuleConditions {
    pub paths: Vec<String>,  // Glob patterns — rule applies only when these paths are involved
}

/// Load rules from a file or directory.
pub fn load_rules(workspace_root: &Path) -> anyhow::Result<Vec<UserRule>> {
    let mut rules = Vec::new();

    // 1. Load global rules from ~/.meh/rules/
    let global_dir = dirs::home_dir().map(|h| h.join(".meh/rules"));
    if let Some(dir) = global_dir {
        if dir.is_dir() {
            rules.extend(load_rules_from_dir(&dir)?);
        }
    }

    // 2. Load workspace rules from .mehrules (file or directory)
    let local_path = workspace_root.join(".mehrules");
    if local_path.is_file() {
        rules.push(parse_rule_file(&local_path)?);
    } else if local_path.is_dir() {
        rules.extend(load_rules_from_dir(&local_path)?);
    }

    Ok(rules)
}

/// Load all rule files from a directory (non-recursive, .md and .txt files).
fn load_rules_from_dir(dir: &Path) -> anyhow::Result<Vec<UserRule>> {
    let mut rules = Vec::new();
    let entries = std::fs::read_dir(dir)?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            match path.extension().and_then(|e| e.to_str()) {
                Some("md" | "txt") => {
                    rules.push(parse_rule_file(&path)?);
                }
                // Also load files with no extension
                None => {
                    rules.push(parse_rule_file(&path)?);
                }
                _ => {}
            }
        }
    }
    Ok(rules)
}

/// Parse a single rule file with optional YAML frontmatter.
fn parse_rule_file(path: &Path) -> anyhow::Result<UserRule> {
    let raw = std::fs::read_to_string(path)?;

    // Check for YAML frontmatter (--- ... ---)
    let (conditions, content) = if raw.starts_with("---\n") || raw.starts_with("---\r\n") {
        if let Some(end) = raw[3..].find("\n---") {
            let yaml = &raw[4..3 + end];
            let body = &raw[3 + end + 4..]; // Skip closing ---\n
            let conditions = parse_frontmatter(yaml)?;
            (Some(conditions), body.trim().to_string())
        } else {
            (None, raw)
        }
    } else {
        (None, raw)
    };

    Ok(UserRule {
        source: path.to_path_buf(),
        content,
        conditions,
        enabled: true,
    })
}

fn parse_frontmatter(yaml: &str) -> anyhow::Result<RuleConditions> {
    // Simple YAML parsing for "paths:" list
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
                paths.push(path.trim().to_string());
            } else {
                in_paths = false;
            }
        }
    }
    Ok(RuleConditions { paths })
}

/// Evaluate whether a rule applies based on its conditions and the active file paths.
pub fn evaluate_rule(rule: &UserRule, active_paths: &[&str]) -> bool {
    if !rule.enabled { return false; }
    match &rule.conditions {
        None => true, // No conditions = always applies
        Some(conds) if conds.paths.is_empty() => true,
        Some(conds) => {
            // Check if any active path matches any condition glob
            active_paths.iter().any(|path| {
                conds.paths.iter().any(|pattern| {
                    glob_match(pattern, path)
                })
            })
        }
    }
}

fn glob_match(pattern: &str, path: &str) -> bool {
    glob::Pattern::new(pattern).map(|p| p.matches(path)).unwrap_or(false)
}

/// Format rules for system prompt injection.
pub fn rules_to_prompt(rules: &[UserRule], active_paths: &[&str]) -> String {
    let applicable: Vec<&UserRule> = rules.iter()
        .filter(|r| evaluate_rule(r, active_paths))
        .collect();
    if applicable.is_empty() { return String::new(); }
    let mut s = String::from("# User Rules\n\n");
    for rule in applicable {
        s.push_str(&rule.content);
        s.push_str("\n\n");
    }
    s
}
```

### 34.2 Integration with system prompt builder

In the prompt builder (STEP 37), call `rules_to_prompt()` with the currently active file paths (from recent tool calls) and inject the result into the system prompt.

### 34.3 Hot-reload support

Watch `.mehrules` (file or directory) and `~/.meh/rules/` for changes using the STEP 29 file watcher. On change, reload rules and rebuild the system prompt for the next API call.

```rust
// In the file watcher setup:
watcher.watch(workspace_root.join(".mehrules"), RecursiveMode::Recursive)?;
if let Some(global_rules) = dirs::home_dir().map(|h| h.join(".meh/rules")) {
    if global_rules.exists() {
        watcher.watch(&global_rules, RecursiveMode::Recursive)?;
    }
}
```

## Tests

```rust
#[cfg(test)]
mod rules_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_load_simple_rule_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".mehrules"), "Always use snake_case.\n").unwrap();
        let rules = load_rules(dir.path()).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].content, "Always use snake_case.");
        assert!(rules[0].conditions.is_none());
    }

    #[test]
    fn test_load_rules_directory() {
        let dir = TempDir::new().unwrap();
        let rules_dir = dir.path().join(".mehrules");
        std::fs::create_dir(&rules_dir).unwrap();
        std::fs::write(rules_dir.join("style.md"), "Use 4-space indentation.").unwrap();
        std::fs::write(rules_dir.join("naming.txt"), "Use descriptive names.").unwrap();
        std::fs::write(rules_dir.join("ignored.rs"), "This should be skipped.").unwrap();
        let rules = load_rules(dir.path()).unwrap();
        assert_eq!(rules.len(), 2); // .md and .txt only
    }

    #[test]
    fn test_yaml_frontmatter_parsing() {
        let dir = TempDir::new().unwrap();
        let content = "---\npaths:\n  - \"src/**/*.rs\"\n  - \"tests/**/*.rs\"\n---\nUse Rust 2024 edition idioms.\n";
        std::fs::write(dir.path().join(".mehrules"), content).unwrap();
        let rules = load_rules(dir.path()).unwrap();
        assert_eq!(rules.len(), 1);
        let conds = rules[0].conditions.as_ref().unwrap();
        assert_eq!(conds.paths.len(), 2);
        assert_eq!(conds.paths[0], "src/**/*.rs");
        assert_eq!(rules[0].content, "Use Rust 2024 edition idioms.");
    }

    #[test]
    fn test_evaluate_unconditional_rule() {
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
    fn test_evaluate_conditional_rule_match() {
        let rule = UserRule {
            source: PathBuf::from("test"),
            content: "test".to_string(),
            conditions: Some(RuleConditions { paths: vec!["src/**/*.rs".to_string()] }),
            enabled: true,
        };
        assert!(evaluate_rule(&rule, &["src/main.rs"]));
        assert!(!evaluate_rule(&rule, &["docs/README.md"]));
    }

    #[test]
    fn test_evaluate_disabled_rule() {
        let rule = UserRule {
            source: PathBuf::from("test"),
            content: "test".to_string(),
            conditions: None,
            enabled: false,
        };
        assert!(!evaluate_rule(&rule, &[]));
    }

    #[test]
    fn test_rules_to_prompt_empty() {
        let rules: Vec<UserRule> = vec![];
        let result = rules_to_prompt(&rules, &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_rules_to_prompt_with_applicable_rules() {
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
                conditions: Some(RuleConditions { paths: vec!["*.py".to_string()] }),
                enabled: true,
            },
        ];
        let result = rules_to_prompt(&rules, &["main.rs"]);
        assert!(result.contains("Rule A"));
        assert!(!result.contains("Rule B")); // *.py doesn't match main.rs
    }

    #[test]
    fn test_glob_match() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("src/**/*.rs", "src/lib.rs"));
        assert!(!glob_match("*.rs", "main.py"));
    }
}
```

## Acceptance Criteria
- [x] .mehrules loaded as single file or directory
- [x] Global rules from ~/.meh/rules/
- [x] YAML frontmatter parsed for path conditions
- [x] Conditional rules evaluated against active paths (glob matching)
- [x] Disabled rules excluded
- [x] Rules injected into system prompt
- [ ] Hot-reload via file watcher
- [x] Only .md, .txt, and extensionless files loaded from directories
- [x] `cargo clippy -- -D warnings` passes
- [x] All tests pass
