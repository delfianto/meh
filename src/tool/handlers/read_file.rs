//! `read_file` tool — reads file contents with optional line range.

use crate::tool::{ToolCategory, ToolContext, ToolHandler, ToolResponse};
use async_trait::async_trait;
use std::fmt::Write;

/// Handler for reading file contents.
pub struct ReadFileHandler;

#[async_trait]
impl ToolHandler for ReadFileHandler {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file at the specified path. Use line_start and line_end for \
         partial reads of large files. Output includes line numbers for reference."
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::ReadOnly
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path of the file to read (relative to working directory or absolute)"
                },
                "line_start": {
                    "type": "integer",
                    "description": "Starting line number (1-indexed, inclusive). Omit to start from the beginning."
                },
                "line_end": {
                    "type": "integer",
                    "description": "Ending line number (1-indexed, inclusive). Omit to read to the end."
                }
            }
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolResponse> {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: path"))?;

        #[allow(clippy::cast_possible_truncation)]
        let line_start = params
            .get("line_start")
            .and_then(serde_json::Value::as_u64)
            .map(|v| v as usize);
        #[allow(clippy::cast_possible_truncation)]
        let line_end = params
            .get("line_end")
            .and_then(serde_json::Value::as_u64)
            .map(|v| v as usize);

        let full_path = crate::util::path::resolve_path(&ctx.cwd, path);

        match tokio::fs::read_to_string(&full_path).await {
            Ok(content) => {
                let lines: Vec<&str> = content.lines().collect();
                let total_lines = lines.len();

                let start = line_start.unwrap_or(1).saturating_sub(1);
                let end = line_end.unwrap_or(total_lines).min(total_lines);

                if start >= total_lines && total_lines > 0 {
                    return Ok(ToolResponse::error(format!(
                        "line_start ({}) exceeds file length ({total_lines} lines)",
                        start + 1
                    )));
                }

                if start > end {
                    return Ok(ToolResponse::error(format!(
                        "line_start ({}) is greater than line_end ({end})",
                        start + 1,
                    )));
                }

                let mut output = String::new();
                for (i, line) in lines[start..end].iter().enumerate() {
                    let _ = writeln!(output, "{:>6}\t{line}", start + i + 1);
                }

                if output.is_empty() {
                    output = "(empty file)".to_string();
                }

                let header = if line_start.is_some() || line_end.is_some() {
                    format!(
                        "File: {path} (lines {}-{end} of {total_lines})\n",
                        start + 1,
                    )
                } else {
                    format!("File: {path} ({total_lines} lines)\n")
                };

                Ok(ToolResponse::success(format!("{header}{output}")))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(ToolResponse::error(format!("File not found: {path}")))
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                Ok(ToolResponse::error(format!("Permission denied: {path}")))
            }
            Err(e) => Ok(ToolResponse::error(format!("Failed to read file: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_file(dir: &TempDir, name: &str, content: &str) -> String {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, content).unwrap();
        dir.path().to_str().unwrap().to_string()
    }

    fn ctx(cwd: &str) -> ToolContext {
        ToolContext {
            cwd: cwd.to_string(),
            auto_approved: false,
        }
    }

    #[tokio::test]
    async fn test_read_entire_file() {
        let dir = TempDir::new().unwrap();
        let cwd = setup_test_file(&dir, "test.txt", "line 1\nline 2\nline 3\n");
        let handler = ReadFileHandler;
        let result = handler
            .execute(serde_json::json!({"path": "test.txt"}), &ctx(&cwd))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("line 1"));
        assert!(result.content.contains("line 2"));
        assert!(result.content.contains("line 3"));
        assert!(result.content.contains("3 lines"));
    }

    #[tokio::test]
    async fn test_read_with_line_range() {
        let dir = TempDir::new().unwrap();
        let cwd = setup_test_file(&dir, "test.txt", "a\nb\nc\nd\ne\n");
        let handler = ReadFileHandler;
        let result = handler
            .execute(
                serde_json::json!({"path": "test.txt", "line_start": 2, "line_end": 4}),
                &ctx(&cwd),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("b"));
        assert!(result.content.contains("c"));
        assert!(result.content.contains("d"));
        assert!(!result.content.contains("\ta\n"));
        assert!(!result.content.contains("\te\n"));
    }

    #[tokio::test]
    async fn test_read_single_line() {
        let dir = TempDir::new().unwrap();
        let cwd = setup_test_file(&dir, "test.txt", "alpha\nbeta\ngamma\n");
        let handler = ReadFileHandler;
        let result = handler
            .execute(
                serde_json::json!({"path": "test.txt", "line_start": 2, "line_end": 2}),
                &ctx(&cwd),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("beta"));
    }

    #[tokio::test]
    async fn test_read_nonexistent_file() {
        let handler = ReadFileHandler;
        let result = handler
            .execute(serde_json::json!({"path": "nonexistent.txt"}), &ctx("/tmp"))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn test_read_missing_path_param() {
        let handler = ReadFileHandler;
        let result = handler.execute(serde_json::json!({}), &ctx("/tmp")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_read_empty_file() {
        let dir = TempDir::new().unwrap();
        let cwd = setup_test_file(&dir, "empty.txt", "");
        let handler = ReadFileHandler;
        let result = handler
            .execute(serde_json::json!({"path": "empty.txt"}), &ctx(&cwd))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("empty"));
    }

    #[tokio::test]
    async fn test_read_line_numbers_formatted() {
        let dir = TempDir::new().unwrap();
        let cwd = setup_test_file(&dir, "test.txt", "hello\nworld\n");
        let handler = ReadFileHandler;
        let result = handler
            .execute(serde_json::json!({"path": "test.txt"}), &ctx(&cwd))
            .await
            .unwrap();
        assert!(result.content.contains("1\thello"));
        assert!(result.content.contains("2\tworld"));
    }

    #[tokio::test]
    async fn test_read_absolute_path() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("abs.txt");
        fs::write(&file_path, "absolute content").unwrap();
        let handler = ReadFileHandler;
        let result = handler
            .execute(
                serde_json::json!({"path": file_path.to_str().unwrap()}),
                &ctx("/tmp"),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("absolute content"));
    }

    #[tokio::test]
    async fn test_read_line_start_exceeds_length() {
        let dir = TempDir::new().unwrap();
        let cwd = setup_test_file(&dir, "short.txt", "one\ntwo\n");
        let handler = ReadFileHandler;
        let result = handler
            .execute(
                serde_json::json!({"path": "short.txt", "line_start": 100}),
                &ctx(&cwd),
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("exceeds"));
    }

    #[test]
    fn test_read_file_metadata() {
        let handler = ReadFileHandler;
        assert_eq!(handler.name(), "read_file");
        assert!(!handler.requires_approval());
        assert_eq!(handler.category(), ToolCategory::ReadOnly);
    }
}
