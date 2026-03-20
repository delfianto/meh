//! Subprocess execution with timeout and output capture.

use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

const MAX_OUTPUT_SIZE: usize = 100 * 1024;
const HALF_OUTPUT_SIZE: usize = 50 * 1024;

/// Result of a command execution.
#[derive(Debug)]
pub struct CommandOutput {
    /// Standard output from the command.
    pub stdout: String,
    /// Standard error from the command.
    pub stderr: String,
    /// Exit code, if the process exited normally.
    pub exit_code: Option<i32>,
    /// Whether the command was killed due to timeout.
    pub timed_out: bool,
}

/// Execute a command with timeout and output capture.
pub async fn execute_command(
    command: &str,
    cwd: &str,
    timeout: Duration,
) -> anyhow::Result<CommandOutput> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut stdout_reader = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to capture stdout"))?;
    let mut stderr_reader = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to capture stderr"))?;

    let stdout_handle = tokio::spawn(async move {
        let mut buf = Vec::new();
        stdout_reader.read_to_end(&mut buf).await?;
        Ok::<_, std::io::Error>(String::from_utf8_lossy(&buf).to_string())
    });

    let stderr_handle = tokio::spawn(async move {
        let mut buf = Vec::new();
        stderr_reader.read_to_end(&mut buf).await?;
        Ok::<_, std::io::Error>(String::from_utf8_lossy(&buf).to_string())
    });

    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => {
            let stdout = stdout_handle.await.map_err(|e| anyhow::anyhow!("{e}"))??;
            let stderr = stderr_handle.await.map_err(|e| anyhow::anyhow!("{e}"))??;
            Ok(CommandOutput {
                stdout,
                stderr,
                exit_code: status.code(),
                timed_out: false,
            })
        }
        Ok(Err(e)) => Err(e.into()),
        Err(_) => {
            let _ = child.kill().await;
            let stdout = stdout_handle
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();
            let stderr = stderr_handle
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();
            Ok(CommandOutput {
                stdout,
                stderr: format!("{stderr}\nCommand timed out after {}s", timeout.as_secs()),
                exit_code: None,
                timed_out: true,
            })
        }
    }
}

/// Determine timeout for a command based on its content.
pub fn resolve_timeout(command: &str) -> Duration {
    let long_running_patterns = [
        "npm", "yarn", "pnpm", "pip", "cargo", "pytest", "docker", "ffmpeg", "webpack", "make",
        "cmake", "mvn", "gradle", "go build", "go test",
    ];
    let cmd_lower = command.to_lowercase();
    if long_running_patterns.iter().any(|p| cmd_lower.contains(p)) {
        Duration::from_secs(300)
    } else {
        Duration::from_secs(30)
    }
}

/// Truncate output if it exceeds the maximum size.
/// Keeps the first and last halves with a truncation marker.
pub fn truncate_output(output: &str) -> String {
    if output.len() <= MAX_OUTPUT_SIZE {
        return output.to_string();
    }

    let first = &output[..HALF_OUTPUT_SIZE];
    let last = &output[output.len() - HALF_OUTPUT_SIZE..];
    format!(
        "{first}\n\n[...truncated {} bytes...]\n\n{last}",
        output.len() - MAX_OUTPUT_SIZE
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_simple_command() {
        let result = execute_command("echo hello", "/tmp", Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(result.stdout.trim(), "hello");
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.timed_out);
    }

    #[tokio::test]
    async fn test_execute_failing_command() {
        let result = execute_command("false", "/tmp", Duration::from_secs(5))
            .await
            .unwrap();
        assert_ne!(result.exit_code, Some(0));
    }

    #[tokio::test]
    async fn test_execute_timeout() {
        let result = execute_command("sleep 10", "/tmp", Duration::from_millis(200))
            .await
            .unwrap();
        assert!(result.timed_out);
    }

    #[tokio::test]
    async fn test_execute_stderr() {
        let result = execute_command("echo error >&2", "/tmp", Duration::from_secs(5))
            .await
            .unwrap();
        assert!(result.stderr.contains("error"));
    }

    #[tokio::test]
    async fn test_execute_multiline() {
        let result = execute_command("echo line1 && echo line2", "/tmp", Duration::from_secs(5))
            .await
            .unwrap();
        assert!(result.stdout.contains("line1"));
        assert!(result.stdout.contains("line2"));
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

    #[test]
    fn test_truncate_short_output() {
        let short = "hello world";
        assert_eq!(truncate_output(short), short);
    }

    #[test]
    fn test_truncate_long_output() {
        let long = "x".repeat(200 * 1024);
        let truncated = truncate_output(&long);
        assert!(truncated.len() < long.len());
        assert!(truncated.contains("[...truncated"));
    }
}
