# STEP 15 — Execute Command Handler

## Objective
Implement the `execute_command` tool handler with timeout management, output streaming, and command permission validation. After this step, the LLM can run shell commands.

## Prerequisites
- STEP 13 complete (permission system)
- STEP 11 complete (tool system)

## Detailed Instructions

### 15.1 Command Permission Validation (`src/permission/command_perms.rs`)

```rust
//! Shell command validation against allow/deny patterns.

/// Command permission rules.
#[derive(Debug, Clone, Default)]
pub struct CommandPermissions {
    pub allow: Vec<String>,   // Glob patterns
    pub deny: Vec<String>,    // Glob patterns
    pub allow_redirects: bool,
}

/// Result of command validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandValidation {
    Allowed,
    Denied { reason: String },
    ParseError { reason: String },
}

impl CommandPermissions {
    pub fn new(allow: Vec<String>, deny: Vec<String>, allow_redirects: bool) -> Self;

    /// Validate a command string against rules.
    pub fn validate(&self, command: &str) -> CommandValidation {
        // 1. Check for dangerous characters outside quotes:
        //    - Backticks ` (command substitution, only safe in single quotes)
        //    - Raw newlines outside quotes
        // 2. Split command by operators: &&, ||, |, ;
        // 3. For each segment:
        //    a. Trim whitespace
        //    b. Check for redirect operators (>, >>, <) — block unless allow_redirects
        //    c. Match against deny patterns (deny takes precedence)
        //    d. Match against allow patterns
        //    e. If allow patterns exist and no match → Denied
        //    f. If no allow patterns exist → Allowed (backward compatible)
        // 4. Handle subshells: $(cmd) and (cmd) — recursively validate
    }
}
```

**Pattern matching**: Use glob-style matching:
- `*` matches any sequence of characters (including `/`)
- `?` matches a single character
- Patterns match against the full command segment (not just the binary name)

```rust
/// Match a command against a glob pattern.
fn matches_pattern(command: &str, pattern: &str) -> bool {
    // Convert glob to regex:
    // * → .*
    // ? → .
    // Escape other regex characters
    // Match against full command string
}
```

### 15.2 Process Utility (`src/util/process.rs`)

```rust
//! Subprocess execution with timeout and output capture.

use std::process::Stdio;
use tokio::process::Command;
use std::time::Duration;

/// Result of a command execution.
#[derive(Debug)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

/// Execute a command with timeout and output capture.
pub async fn execute_command(
    command: &str,
    cwd: &str,
    timeout: Duration,
    on_output: Option<&dyn Fn(&str)>, // Optional callback for streaming output
) -> anyhow::Result<CommandOutput> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // Read stdout and stderr concurrently
    let stdout_handle = tokio::spawn(async move {
        // Read from child.stdout
        // Optionally call on_output for each line
    });
    let stderr_handle = tokio::spawn(async move {
        // Read from child.stderr
    });

    // Wait with timeout
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => {
            let stdout = stdout_handle.await??;
            let stderr = stderr_handle.await??;
            Ok(CommandOutput {
                stdout,
                stderr,
                exit_code: status.code(),
                timed_out: false,
            })
        }
        Ok(Err(e)) => Err(e.into()),
        Err(_) => {
            // Timeout — kill the process
            let _ = child.kill().await;
            Ok(CommandOutput {
                stdout: String::new(),
                stderr: "Command timed out".to_string(),
                exit_code: None,
                timed_out: true,
            })
        }
    }
}

/// Determine timeout for a command based on its content.
pub fn resolve_timeout(command: &str) -> Duration {
    let long_running_patterns = [
        "npm", "yarn", "pnpm", "pip", "cargo", "pytest",
        "docker", "ffmpeg", "webpack", "make", "cmake",
        "mvn", "gradle", "go build", "go test",
    ];
    let cmd_lower = command.to_lowercase();
    if long_running_patterns.iter().any(|p| cmd_lower.contains(p)) {
        Duration::from_secs(300)
    } else {
        Duration::from_secs(30)
    }
}
```

### 15.3 ExecuteCommandHandler (`src/tool/handlers/execute_command.rs`)

```rust
//! execute_command tool — run shell commands.

pub struct ExecuteCommandHandler;

// Input schema: command (required string)

// Implementation:
// 1. Extract command string
// 2. Resolve timeout via resolve_timeout()
// 3. Execute via util::process::execute_command()
// 4. Format output: combine stdout and stderr
// 5. Include exit code in response
// 6. On timeout: include timeout message
// 7. Truncate output if > 100KB (include first 50KB + last 50KB with "[...truncated...]" marker)
```

## Tests

```rust
// command_perms tests
#[cfg(test)]
mod command_perms_tests {
    use super::*;

    #[test]
    fn test_no_rules_allows_all() {
        let perms = CommandPermissions::default();
        assert_eq!(perms.validate("ls -la"), CommandValidation::Allowed);
        assert_eq!(perms.validate("rm -rf /"), CommandValidation::Allowed);
    }

    #[test]
    fn test_allow_rules() {
        let perms = CommandPermissions::new(
            vec!["git *".to_string(), "cargo *".to_string(), "ls".to_string()],
            vec![],
            false,
        );
        assert_eq!(perms.validate("git status"), CommandValidation::Allowed);
        assert_eq!(perms.validate("cargo test"), CommandValidation::Allowed);
        assert_eq!(perms.validate("ls"), CommandValidation::Allowed);
        assert!(matches!(perms.validate("rm -rf /"), CommandValidation::Denied { .. }));
    }

    #[test]
    fn test_deny_takes_precedence() {
        let perms = CommandPermissions::new(
            vec!["*".to_string()],
            vec!["rm *".to_string(), "sudo *".to_string()],
            false,
        );
        assert_eq!(perms.validate("ls"), CommandValidation::Allowed);
        assert!(matches!(perms.validate("rm -rf /"), CommandValidation::Denied { .. }));
        assert!(matches!(perms.validate("sudo apt install"), CommandValidation::Denied { .. }));
    }

    #[test]
    fn test_redirect_blocked_by_default() {
        let perms = CommandPermissions::new(vec!["*".to_string()], vec![], false);
        assert!(matches!(perms.validate("echo hello > file.txt"), CommandValidation::Denied { .. }));
        assert!(matches!(perms.validate("cat < input.txt"), CommandValidation::Denied { .. }));
    }

    #[test]
    fn test_redirect_allowed_when_configured() {
        let perms = CommandPermissions::new(vec!["*".to_string()], vec![], true);
        assert_eq!(perms.validate("echo hello > file.txt"), CommandValidation::Allowed);
    }

    #[test]
    fn test_pipe_splits_segments() {
        let perms = CommandPermissions::new(
            vec!["cat *".to_string(), "grep *".to_string()],
            vec![],
            false,
        );
        assert_eq!(perms.validate("cat file.txt | grep pattern"), CommandValidation::Allowed);
    }

    #[test]
    fn test_chained_commands() {
        let perms = CommandPermissions::new(
            vec!["git *".to_string(), "echo *".to_string()],
            vec![],
            false,
        );
        assert_eq!(perms.validate("git add . && git commit -m 'test'"), CommandValidation::Allowed);
        assert!(matches!(perms.validate("git add . && rm -rf /"), CommandValidation::Denied { .. }));
    }

    #[test]
    fn test_dangerous_backticks() {
        let perms = CommandPermissions::new(vec!["echo *".to_string()], vec![], false);
        assert!(matches!(perms.validate("echo `rm -rf /`"), CommandValidation::Denied { .. }));
    }

    #[test]
    fn test_backticks_safe_in_single_quotes() {
        let perms = CommandPermissions::new(vec!["echo *".to_string()], vec![], false);
        assert_eq!(perms.validate("echo 'hello `world`'"), CommandValidation::Allowed);
    }

    #[test]
    fn test_pattern_matching() {
        assert!(matches_pattern("git status", "git *"));
        assert!(matches_pattern("cargo test --release", "cargo *"));
        assert!(!matches_pattern("rm -rf /", "git *"));
        assert!(matches_pattern("ls", "ls"));
        assert!(!matches_pattern("lsof", "ls"));
    }
}

// process tests
#[cfg(test)]
mod process_tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_simple_command() {
        let result = execute_command("echo hello", "/tmp", Duration::from_secs(5), None).await.unwrap();
        assert_eq!(result.stdout.trim(), "hello");
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.timed_out);
    }

    #[tokio::test]
    async fn test_execute_failing_command() {
        let result = execute_command("false", "/tmp", Duration::from_secs(5), None).await.unwrap();
        assert_ne!(result.exit_code, Some(0));
    }

    #[tokio::test]
    async fn test_execute_timeout() {
        let result = execute_command("sleep 10", "/tmp", Duration::from_millis(100), None).await.unwrap();
        assert!(result.timed_out);
    }

    #[tokio::test]
    async fn test_execute_stderr() {
        let result = execute_command("echo error >&2", "/tmp", Duration::from_secs(5), None).await.unwrap();
        assert!(result.stderr.contains("error"));
    }

    #[test]
    fn test_resolve_timeout_normal() {
        assert_eq!(resolve_timeout("ls -la"), Duration::from_secs(30));
        assert_eq!(resolve_timeout("echo hello"), Duration::from_secs(30));
    }

    #[test]
    fn test_resolve_timeout_long_running() {
        assert_eq!(resolve_timeout("cargo test"), Duration::from_secs(300));
        assert_eq!(resolve_timeout("npm install"), Duration::from_secs(300));
        assert_eq!(resolve_timeout("docker build ."), Duration::from_secs(300));
    }
}

// execute_command handler tests
#[cfg(test)]
mod handler_tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_handler() {
        let handler = ExecuteCommandHandler;
        let ctx = ToolContext { cwd: "/tmp".to_string(), auto_approved: true };
        let result = handler.execute(serde_json::json!({"command": "echo test"}), &ctx).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("test"));
    }

    #[tokio::test]
    async fn test_execute_handler_missing_param() {
        let handler = ExecuteCommandHandler;
        let ctx = ToolContext { cwd: "/tmp".to_string(), auto_approved: true };
        assert!(handler.execute(serde_json::json!({}), &ctx).await.is_err());
    }

    #[tokio::test]
    async fn test_execute_handler_exit_code_in_output() {
        let handler = ExecuteCommandHandler;
        let ctx = ToolContext { cwd: "/tmp".to_string(), auto_approved: true };
        let result = handler.execute(serde_json::json!({"command": "false"}), &ctx).await.unwrap();
        // Should include exit code info
        assert!(result.content.contains("exit") || result.is_error);
    }
}
```

## Acceptance Criteria
- [ ] Command permission validation: allow/deny glob patterns, deny precedence
- [ ] Dangerous character detection (backticks outside single quotes)
- [ ] Redirect blocking (default off, configurable)
- [ ] Command segment splitting (&&, ||, |, ;)
- [ ] Subprocess execution with stdout/stderr capture
- [ ] Timeout management (30s default, 300s for long-running)
- [ ] Process killed on timeout
- [ ] Output truncation for very large outputs (>100KB)
- [ ] execute_command handler wires everything together
- [ ] `cargo clippy -- -D warnings` passes
- [ ] All tests pass (20+ test cases)
