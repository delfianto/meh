//! `list_files` tool — list directory contents with optional recursive traversal.

use crate::tool::{ToolCategory, ToolContext, ToolHandler, ToolResponse};
use crate::util::path;
use async_trait::async_trait;
use std::fmt::Write;
use std::path::{Path, PathBuf};

const MAX_DEPTH: usize = 5;
const MAX_ENTRIES: usize = 500;

/// Handler for listing directory contents.
pub struct ListFilesHandler;

#[async_trait]
impl ToolHandler for ListFilesHandler {
    fn name(&self) -> &str {
        "list_files"
    }

    fn description(&self) -> &str {
        "List files and directories at the specified path. Use recursive=true to list \
         subdirectories (up to 5 levels deep, max 500 entries)."
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
                    "description": "Directory path to list (relative to working directory or absolute)"
                },
                "recursive": {
                    "type": "boolean",
                    "description": "Whether to list subdirectories recursively (default: false)"
                }
            }
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolResponse> {
        let dir_path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: path"))?;

        let recursive = params
            .get("recursive")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let full_path = path::resolve_path(&ctx.cwd, dir_path);

        if !full_path.is_dir() {
            return Ok(ToolResponse::error(format!(
                "Not a directory or does not exist: {dir_path}"
            )));
        }

        let (entries, total_count, truncated) = if recursive {
            list_recursive(&full_path).await
        } else {
            list_flat(&full_path).await?
        };

        if entries.is_empty() {
            return Ok(ToolResponse::success(format!(
                "Directory: {dir_path}\n(empty directory)"
            )));
        }

        let mut output = format!("Directory: {dir_path}\n");
        for entry in &entries {
            let _ = writeln!(output, "  {entry}");
        }

        if truncated {
            let _ = write!(
                output,
                "\n(truncated: showing {MAX_ENTRIES} of {total_count} entries)"
            );
        }

        Ok(ToolResponse::success(output))
    }
}

/// Read a single directory level, sorted dirs-first then alphabetically.
async fn read_sorted_children(dir: &std::path::Path) -> Vec<(String, bool, u64)> {
    let Ok(mut dir_entries) = tokio::fs::read_dir(dir).await else {
        return Vec::new();
    };

    let mut children: Vec<(String, bool, u64)> = Vec::new();

    while let Ok(Some(entry)) = dir_entries.next_entry().await {
        let file_name = entry.file_name().to_string_lossy().to_string();
        let entry_path = entry.path();

        if path::should_ignore(&entry_path) {
            continue;
        }

        let Ok(metadata) = entry.metadata().await else {
            continue;
        };

        let is_dir = metadata.is_dir();
        let size = if is_dir { 0 } else { metadata.len() };

        children.push((file_name, is_dir, size));
    }

    children.sort_by(|a, b| match (a.1, b.1) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.0.cmp(&b.0),
    });

    children
}

/// List directory contents recursively with depth and entry limits.
async fn list_recursive(full_path: &Path) -> (Vec<String>, usize, bool) {
    let mut entries = Vec::new();
    let mut total_count = 0;
    let mut stack: Vec<(PathBuf, usize)> = vec![(full_path.to_path_buf(), 0)];

    while let Some((current_dir, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            continue;
        }

        let children = read_sorted_children(&current_dir).await;

        let prefix = current_dir
            .strip_prefix(full_path)
            .unwrap_or(&current_dir)
            .to_string_lossy()
            .to_string();

        for (name, is_dir, size) in children {
            if is_dir {
                stack.push((current_dir.join(&name), depth + 1));
            }

            total_count += 1;
            if total_count <= MAX_ENTRIES {
                let display_path = if prefix.is_empty() {
                    name
                } else {
                    format!("{prefix}/{name}")
                };

                if is_dir {
                    entries.push(format!("{display_path}/"));
                } else {
                    entries.push(format!("{display_path} ({})", format_file_size(size)));
                }
            }
        }
    }

    let truncated = total_count > MAX_ENTRIES;
    (entries, total_count, truncated)
}

/// List a single directory (non-recursive).
async fn list_flat(full_path: &Path) -> anyhow::Result<(Vec<String>, usize, bool)> {
    let children = read_sorted_children(full_path).await;
    let total_count = children.len();

    let entries: Vec<String> = children
        .iter()
        .map(|(name, is_dir, size)| {
            if *is_dir {
                format!("{name}/")
            } else {
                format!("{name} ({})", format_file_size(*size))
            }
        })
        .collect();

    Ok((entries, total_count, false))
}

/// Format a file size in human-readable form.
#[allow(clippy::cast_precision_loss)]
pub fn format_file_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
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
            auto_approved: false,
        }
    }

    #[tokio::test]
    async fn test_list_simple_dir() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.rs"), "").unwrap();
        fs::write(dir.path().join("b.rs"), "").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        let handler = ListFilesHandler;
        let cwd = dir.path().to_str().unwrap();
        let result = handler
            .execute(serde_json::json!({"path": "."}), &ctx(cwd))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("a.rs"));
        assert!(result.content.contains("b.rs"));
        assert!(result.content.contains("sub/"));
    }

    #[tokio::test]
    async fn test_list_directories_first() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("file.txt"), "").unwrap();
        fs::create_dir(dir.path().join("aaa_dir")).unwrap();
        let handler = ListFilesHandler;
        let cwd = dir.path().to_str().unwrap();
        let result = handler
            .execute(serde_json::json!({"path": "."}), &ctx(cwd))
            .await
            .unwrap();
        assert!(!result.is_error);
        let dir_pos = result.content.find("aaa_dir/").unwrap();
        let file_pos = result.content.find("file.txt").unwrap();
        assert!(dir_pos < file_pos);
    }

    #[tokio::test]
    async fn test_list_recursive() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("src/util")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(dir.path().join("src/util/fs.rs"), "").unwrap();
        let handler = ListFilesHandler;
        let cwd = dir.path().to_str().unwrap();
        let result = handler
            .execute(
                serde_json::json!({"path": ".", "recursive": true}),
                &ctx(cwd),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("main.rs"));
        assert!(result.content.contains("fs.rs"));
    }

    #[tokio::test]
    async fn test_list_nonexistent_dir() {
        let handler = ListFilesHandler;
        let result = handler
            .execute(
                serde_json::json!({"path": "/nonexistent_dir_xyz"}),
                &ctx("/tmp"),
            )
            .await
            .unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_list_empty_dir() {
        let dir = TempDir::new().unwrap();
        let handler = ListFilesHandler;
        let cwd = dir.path().to_str().unwrap();
        let result = handler
            .execute(serde_json::json!({"path": "."}), &ctx(cwd))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("empty"));
    }

    #[tokio::test]
    async fn test_list_ignores_node_modules() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("node_modules")).unwrap();
        fs::write(dir.path().join("node_modules/package.json"), "{}").unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        let handler = ListFilesHandler;
        let cwd = dir.path().to_str().unwrap();
        let result = handler
            .execute(
                serde_json::json!({"path": ".", "recursive": true}),
                &ctx(cwd),
            )
            .await
            .unwrap();
        assert!(!result.content.contains("node_modules"));
    }

    #[tokio::test]
    async fn test_list_ignores_git_dir() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".git/config"), "").unwrap();
        fs::write(dir.path().join("README.md"), "").unwrap();
        let handler = ListFilesHandler;
        let cwd = dir.path().to_str().unwrap();
        let result = handler
            .execute(
                serde_json::json!({"path": ".", "recursive": true}),
                &ctx(cwd),
            )
            .await
            .unwrap();
        assert!(!result.content.contains(".git"));
        assert!(result.content.contains("README.md"));
    }

    #[tokio::test]
    async fn test_list_shows_file_sizes() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("small.txt"), "hi").unwrap();
        let handler = ListFilesHandler;
        let cwd = dir.path().to_str().unwrap();
        let result = handler
            .execute(serde_json::json!({"path": "."}), &ctx(cwd))
            .await
            .unwrap();
        assert!(result.content.contains("B"));
    }

    #[test]
    fn test_format_file_size() {
        assert_eq!(format_file_size(0), "0 B");
        assert_eq!(format_file_size(512), "512 B");
        assert_eq!(format_file_size(1024), "1.0 KB");
        assert_eq!(format_file_size(1_048_576), "1.0 MB");
        assert_eq!(format_file_size(1_073_741_824), "1.0 GB");
    }

    #[test]
    fn test_list_files_metadata() {
        let handler = ListFilesHandler;
        assert_eq!(handler.name(), "list_files");
        assert!(!handler.requires_approval());
        assert_eq!(handler.category(), ToolCategory::ReadOnly);
    }
}
