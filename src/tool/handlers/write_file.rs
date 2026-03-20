//! `write_file` tool — create or overwrite files.

use crate::tool::{ToolCategory, ToolContext, ToolHandler, ToolResponse};
use async_trait::async_trait;

/// Handler for writing file contents.
pub struct WriteFileHandler;

#[async_trait]
impl ToolHandler for WriteFileHandler {
    fn name(&self) -> &str {
        "write_to_file"
    }

    fn description(&self) -> &str {
        "Create a new file or overwrite an existing file with the provided content."
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::FileWrite
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["path", "content"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path relative to working directory"
                },
                "content": {
                    "type": "string",
                    "description": "Complete file content to write"
                }
            }
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolResponse> {
        let path_str = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: path"))?;
        let content = params
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: content"))?;

        let full_path = crate::util::path::resolve_path(&ctx.cwd, path_str);

        if let Some(parent) = full_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to create directories: {e}"))?;
        }

        let existed = full_path.exists();

        tokio::fs::write(&full_path, content)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write file: {e}"))?;

        let action = if existed { "Updated" } else { "Created" };
        let line_count = content.lines().count();
        Ok(ToolResponse::success(format!(
            "{action} file: {path_str} ({line_count} lines)"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn ctx(cwd: &str) -> ToolContext {
        ToolContext {
            cwd: cwd.to_string(),
            auto_approved: true,
        }
    }

    #[tokio::test]
    async fn test_write_new_file() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_str().unwrap();
        let handler = WriteFileHandler;
        let result = handler
            .execute(
                serde_json::json!({"path": "new_file.txt", "content": "hello world\n"}),
                &ctx(cwd),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("Created"));
        let content = fs::read_to_string(dir.path().join("new_file.txt")).unwrap();
        assert_eq!(content, "hello world\n");
    }

    #[tokio::test]
    async fn test_write_overwrites_existing() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("existing.txt"), "old content").unwrap();
        let cwd = dir.path().to_str().unwrap();
        let handler = WriteFileHandler;
        let result = handler
            .execute(
                serde_json::json!({"path": "existing.txt", "content": "new content"}),
                &ctx(cwd),
            )
            .await
            .unwrap();
        assert!(result.content.contains("Updated"));
        let content = fs::read_to_string(dir.path().join("existing.txt")).unwrap();
        assert_eq!(content, "new content");
    }

    #[tokio::test]
    async fn test_write_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_str().unwrap();
        let handler = WriteFileHandler;
        let result = handler
            .execute(
                serde_json::json!({"path": "deep/nested/dir/file.txt", "content": "nested content"}),
                &ctx(cwd),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(dir.path().join("deep/nested/dir/file.txt").exists());
    }

    #[tokio::test]
    async fn test_write_missing_params() {
        let handler = WriteFileHandler;
        assert!(
            handler
                .execute(serde_json::json!({"path": "test.txt"}), &ctx("/tmp"))
                .await
                .is_err()
        );
        assert!(
            handler
                .execute(serde_json::json!({"content": "test"}), &ctx("/tmp"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_write_empty_content() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_str().unwrap();
        let handler = WriteFileHandler;
        let result = handler
            .execute(
                serde_json::json!({"path": "empty.txt", "content": ""}),
                &ctx(cwd),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("0 lines"));
    }

    #[test]
    fn test_write_file_metadata() {
        let handler = WriteFileHandler;
        assert_eq!(handler.name(), "write_to_file");
        assert!(handler.requires_approval());
        assert_eq!(handler.category(), ToolCategory::FileWrite);
    }
}
