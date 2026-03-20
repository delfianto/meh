# STEP 12 — Read-Only Tool Handlers (read_file, list_files, search_files)

## Objective
Implement the three read-only tool handlers. These are the safest tools and the first ones the LLM will use. After this step, the LLM can read files, list directories, and search code.

## Prerequisites
- STEP 11 complete (ToolHandler trait and ToolRegistry defined)

## Detailed Instructions

### 12.1 ReadFileHandler (`src/tool/handlers/read_file.rs`)

```rust
//! read_file tool — reads file contents with optional line range.

use crate::tool::{ToolCategory, ToolContext, ToolHandler, ToolResponse};
use async_trait::async_trait;

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
        false // Read-only, safe by default
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

        let line_start = params
            .get("line_start")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let line_end = params
            .get("line_end")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        // Resolve path relative to cwd
        let full_path = crate::util::path::resolve_path(&ctx.cwd, path);

        // Read file
        match tokio::fs::read_to_string(&full_path).await {
            Ok(content) => {
                let lines: Vec<&str> = content.lines().collect();
                let total_lines = lines.len();

                // Apply line range (convert from 1-indexed to 0-indexed)
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
                        "line_start ({}) is greater than line_end ({})",
                        start + 1,
                        end
                    )));
                }

                // Format with line numbers
                let mut output = String::new();
                for (i, line) in lines[start..end].iter().enumerate() {
                    output.push_str(&format!("{:>6}\t{}\n", start + i + 1, line));
                }

                if output.is_empty() {
                    output = "(empty file)".to_string();
                }

                // Add metadata header
                let header = if line_start.is_some() || line_end.is_some() {
                    format!(
                        "File: {} (lines {}-{} of {total_lines})\n",
                        path,
                        start + 1,
                        end
                    )
                } else {
                    format!("File: {} ({total_lines} lines)\n", path)
                };

                Ok(ToolResponse::success(format!("{header}{output}")))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(ToolResponse::error(format!("File not found: {path}")))
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                Ok(ToolResponse::error(format!(
                    "Permission denied: {path}"
                )))
            }
            Err(e) => Ok(ToolResponse::error(format!(
                "Failed to read file: {e}"
            ))),
        }
    }
}
```

### 12.2 ListFilesHandler (`src/tool/handlers/list_files.rs`)

```rust
//! list_files tool — list directory contents with optional recursive traversal.

use crate::tool::{ToolCategory, ToolContext, ToolHandler, ToolResponse};
use crate::util::path;
use async_trait::async_trait;
use std::path::PathBuf;

const MAX_DEPTH: usize = 5;
const MAX_ENTRIES: usize = 500;

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
        false // Read-only, safe by default
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
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let full_path = path::resolve_path(&ctx.cwd, dir_path);

        if !full_path.is_dir() {
            return Ok(ToolResponse::error(format!(
                "Not a directory or does not exist: {dir_path}"
            )));
        }

        let mut entries = Vec::new();
        let mut total_count = 0;
        let truncated;

        if recursive {
            // Stack-based recursive traversal
            let mut stack: Vec<(PathBuf, usize)> = vec![(full_path.clone(), 0)];

            while let Some((current_dir, depth)) = stack.pop() {
                if depth > MAX_DEPTH {
                    continue;
                }

                let mut dir_entries = match tokio::fs::read_dir(&current_dir).await {
                    Ok(rd) => rd,
                    Err(_) => continue,
                };

                let mut children: Vec<(String, bool, u64)> = Vec::new();

                while let Ok(Some(entry)) = dir_entries.next_entry().await {
                    let file_name = entry.file_name().to_string_lossy().to_string();
                    let entry_path = entry.path();

                    // Skip ignored paths
                    if path::should_ignore(&entry_path) {
                        continue;
                    }

                    let metadata = match entry.metadata().await {
                        Ok(m) => m,
                        Err(_) => continue,
                    };

                    let is_dir = metadata.is_dir();
                    let size = if is_dir { 0 } else { metadata.len() };

                    children.push((file_name, is_dir, size));

                    if is_dir {
                        stack.push((entry_path, depth + 1));
                    }
                }

                // Sort: directories first, then files, alphabetically
                children.sort_by(|a, b| match (a.1, b.1) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => a.0.cmp(&b.0),
                });

                let prefix = current_dir
                    .strip_prefix(&full_path)
                    .unwrap_or(&current_dir)
                    .to_string_lossy()
                    .to_string();

                for (name, is_dir, size) in children {
                    total_count += 1;
                    if total_count <= MAX_ENTRIES {
                        let display_path = if prefix.is_empty() {
                            name.clone()
                        } else {
                            format!("{prefix}/{name}")
                        };

                        if is_dir {
                            entries.push(format!("{display_path}/"));
                        } else {
                            entries.push(format!(
                                "{display_path} ({})",
                                format_file_size(size)
                            ));
                        }
                    }
                }
            }

            truncated = total_count > MAX_ENTRIES;
        } else {
            // Non-recursive: single directory listing
            let mut dir_entries = match tokio::fs::read_dir(&full_path).await {
                Ok(rd) => rd,
                Err(e) => {
                    return Ok(ToolResponse::error(format!(
                        "Failed to read directory: {e}"
                    )))
                }
            };

            let mut children: Vec<(String, bool, u64)> = Vec::new();

            while let Ok(Some(entry)) = dir_entries.next_entry().await {
                let file_name = entry.file_name().to_string_lossy().to_string();
                let entry_path = entry.path();

                if path::should_ignore(&entry_path) {
                    continue;
                }

                let metadata = match entry.metadata().await {
                    Ok(m) => m,
                    Err(_) => continue,
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

            for (name, is_dir, size) in &children {
                if *is_dir {
                    entries.push(format!("{name}/"));
                } else {
                    entries.push(format!("{name} ({})", format_file_size(*size)));
                }
            }

            total_count = children.len();
            truncated = false;
        }

        if entries.is_empty() {
            return Ok(ToolResponse::success(format!(
                "Directory: {dir_path}\n(empty directory)"
            )));
        }

        let mut output = format!("Directory: {dir_path}\n");
        for entry in &entries {
            output.push_str(&format!("  {entry}\n"));
        }

        if truncated {
            output.push_str(&format!(
                "\n(truncated: showing {MAX_ENTRIES} of {total_count} entries)"
            ));
        }

        Ok(ToolResponse::success(output))
    }
}

/// Format a file size in human-readable form.
fn format_file_size(bytes: u64) -> String {
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
```

### 12.3 SearchFilesHandler (`src/tool/handlers/search_files.rs`)

```rust
//! search_files tool — regex search across files in a directory.

use crate::tool::{ToolCategory, ToolContext, ToolHandler, ToolResponse};
use crate::util::path;
use async_trait::async_trait;
use std::path::PathBuf;

const MAX_RESULTS: usize = 50;
const MAX_FILE_SIZE: u64 = 1_048_576; // 1 MB
const MAX_FILES_SCANNED: usize = 5000;

pub struct SearchFilesHandler;

#[async_trait]
impl ToolHandler for SearchFilesHandler {
    fn name(&self) -> &str {
        "search_files"
    }

    fn description(&self) -> &str {
        "Search for a regex pattern across files in a directory. Returns matching lines \
         with file paths and line numbers. Skips binary files and files larger than 1MB."
    }

    fn requires_approval(&self) -> bool {
        false // Read-only, safe by default
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::ReadOnly
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["path", "regex"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory path to search in (relative to working directory or absolute)"
                },
                "regex": {
                    "type": "string",
                    "description": "Regular expression pattern to search for"
                },
                "file_pattern": {
                    "type": "string",
                    "description": "Glob pattern to filter files (e.g., '*.rs', '*.{ts,tsx}')"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of matching lines to return (default: 50)"
                }
            }
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolResponse> {
        let search_path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: path"))?;

        let regex_str = params
            .get("regex")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: regex"))?;

        let file_pattern = params
            .get("file_pattern")
            .and_then(|v| v.as_str());

        let max_results = params
            .get("max_results")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(MAX_RESULTS);

        // Compile regex
        let regex = match regex::Regex::new(regex_str) {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolResponse::error(format!(
                    "Invalid regex pattern: {e}"
                )))
            }
        };

        // Compile glob pattern if provided
        let glob_pattern = if let Some(pattern) = file_pattern {
            match glob::Pattern::new(pattern) {
                Ok(p) => Some(p),
                Err(e) => {
                    return Ok(ToolResponse::error(format!(
                        "Invalid file pattern: {e}"
                    )))
                }
            }
        } else {
            None
        };

        let full_path = path::resolve_path(&ctx.cwd, search_path);

        if !full_path.is_dir() {
            return Ok(ToolResponse::error(format!(
                "Not a directory or does not exist: {search_path}"
            )));
        }

        // Collect all files to search
        let files = collect_files(&full_path, &glob_pattern).await;

        // Search files
        let mut matches: Vec<(String, usize, String)> = Vec::new(); // (file, line_no, line_content)
        let mut files_scanned = 0;
        let mut files_with_matches = 0;

        for file_path in &files {
            if matches.len() >= max_results {
                break;
            }

            files_scanned += 1;

            // Check file size
            let metadata = match tokio::fs::metadata(file_path).await {
                Ok(m) => m,
                Err(_) => continue,
            };

            if metadata.len() > MAX_FILE_SIZE {
                continue;
            }

            // Read file
            let content = match tokio::fs::read(file_path).await {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Skip binary files (check first 512 bytes for null bytes)
            let check_len = content.len().min(512);
            if content[..check_len].contains(&0) {
                continue;
            }

            let text = match String::from_utf8(content) {
                Ok(t) => t,
                Err(_) => continue, // Not valid UTF-8, skip
            };

            let relative_path = file_path
                .strip_prefix(&full_path)
                .unwrap_or(file_path)
                .to_string_lossy()
                .to_string();

            let mut file_had_match = false;
            for (line_no, line) in text.lines().enumerate() {
                if matches.len() >= max_results {
                    break;
                }

                if regex.is_match(line) {
                    matches.push((
                        relative_path.clone(),
                        line_no + 1,
                        line.to_string(),
                    ));
                    file_had_match = true;
                }
            }

            if file_had_match {
                files_with_matches += 1;
            }
        }

        if matches.is_empty() {
            return Ok(ToolResponse::success(format!(
                "No matches found for '{}' in {} ({} files scanned)",
                regex_str, search_path, files_scanned
            )));
        }

        // Format output grouped by file
        let mut output = String::new();
        let mut current_file = String::new();

        for (file, line_no, content) in &matches {
            if file != &current_file {
                if !current_file.is_empty() {
                    output.push('\n');
                }
                output.push_str(&format!("{file}:\n"));
                current_file = file.clone();
            }
            // Truncate long lines
            let display_content = if content.len() > 200 {
                format!("{}...", &content[..200])
            } else {
                content.clone()
            };
            output.push_str(&format!("  {line_no}: {display_content}\n"));
        }

        output.push_str(&format!(
            "\n{} matches in {} files ({} files scanned)",
            matches.len(),
            files_with_matches,
            files_scanned
        ));

        if matches.len() >= max_results {
            output.push_str(&format!(
                "\n(results truncated at {max_results} matches)"
            ));
        }

        Ok(ToolResponse::success(output))
    }
}

/// Recursively collect all files in a directory, respecting ignore patterns and glob filter.
async fn collect_files(
    dir: &PathBuf,
    glob_pattern: &Option<glob::Pattern>,
) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.clone()];
    let mut total_files = 0;

    while let Some(current) = stack.pop() {
        if total_files >= MAX_FILES_SCANNED {
            break;
        }

        let mut entries = match tokio::fs::read_dir(&current).await {
            Ok(e) => e,
            Err(_) => continue,
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let entry_path = entry.path();

            if path::should_ignore(&entry_path) {
                continue;
            }

            let metadata = match entry.metadata().await {
                Ok(m) => m,
                Err(_) => continue,
            };

            if metadata.is_dir() {
                stack.push(entry_path);
            } else if metadata.is_file() {
                // Apply glob filter
                if let Some(pattern) = glob_pattern {
                    let file_name = entry_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("");
                    if !pattern.matches(file_name) {
                        continue;
                    }
                }

                files.push(entry_path);
                total_files += 1;
            }
        }
    }

    files.sort(); // Deterministic ordering
    files
}
```

### 12.4 Path utility (`src/util/path.rs`)

```rust
//! Path resolution utilities.

use std::path::{Path, PathBuf};

/// Resolve a path that may be relative against a base directory.
/// If the path is absolute, it is returned as-is.
/// If relative, it is joined with the base directory.
pub fn resolve_path(base: &str, relative: &str) -> PathBuf {
    let path = Path::new(relative);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(base).join(path)
    }
}

/// Check if a path should be ignored based on common ignore patterns.
/// This checks the final component of the path (file/directory name).
pub fn should_ignore(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    matches!(
        name,
        "node_modules"
            | "target"
            | ".git"
            | "__pycache__"
            | "dist"
            | "build"
            | ".DS_Store"
            | "Thumbs.db"
            | ".next"
            | ".nuxt"
            | "vendor"
            | ".venv"
            | "venv"
            | ".idea"
            | ".vscode"
            | "coverage"
            | ".cache"
            | ".tox"
            | ".mypy_cache"
            | ".pytest_cache"
            | ".ruff_cache"
    )
}

/// Check if a file path looks like a binary file based on its extension.
pub fn is_likely_binary(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "ico" | "webp" | "svg"
            | "mp3" | "mp4" | "wav" | "avi" | "mkv" | "mov"
            | "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar"
            | "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx"
            | "exe" | "dll" | "so" | "dylib" | "o" | "a"
            | "wasm" | "ttf" | "woff" | "woff2" | "eot"
            | "pyc" | "pyo" | "class"
            | "db" | "sqlite" | "sqlite3"
    )
}
```

### 12.5 Util module (`src/util/mod.rs`)

```rust
//! Utility modules.

pub mod path;
```

### 12.6 Handlers mod.rs update

Ensure `src/tool/handlers/mod.rs` includes all handler modules:

```rust
pub mod read_file;
pub mod list_files;
pub mod search_files;
pub mod write_file;
pub mod apply_patch;
pub mod execute_command;
pub mod ask_followup;
pub mod attempt_completion;
pub mod plan_mode_respond;
pub mod mcp_tool;
```

## Tests

```rust
// ─── read_file tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod read_file_tests {
    use super::read_file::*;
    use crate::tool::{ToolContext, ToolHandler};
    use tempfile::TempDir;
    use std::fs;

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
        assert!(result.content.contains("3 lines")); // metadata
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
        assert!(!result.content.contains("\t" .to_owned() + "a\n")); // line 1 excluded
        assert!(!result.content.contains("\te\n")); // line 5 excluded
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
            .execute(
                serde_json::json!({"path": "nonexistent.txt"}),
                &ctx("/tmp"),
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn test_read_missing_path_param() {
        let handler = ReadFileHandler;
        let result = handler.execute(serde_json::json!({}), &ctx("/tmp")).await;
        assert!(result.is_err()); // Missing required param returns Err
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
        // Should have line numbers with tab separator
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

// ─── list_files tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod list_files_tests {
    use super::list_files::*;
    use crate::tool::{ToolContext, ToolHandler};
    use tempfile::TempDir;
    use std::fs;

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
        // Directory should appear before file in output
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
        fs::write(
            dir.path().join("node_modules/package.json"),
            "{}",
        )
        .unwrap();
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
        assert!(result.content.contains("B")); // Should show size in bytes
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

// ─── search_files tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod search_files_tests {
    use super::search_files::*;
    use crate::tool::{ToolContext, ToolHandler};
    use tempfile::TempDir;
    use std::fs;

    fn ctx(cwd: &str) -> ToolContext {
        ToolContext {
            cwd: cwd.to_string(),
            auto_approved: false,
        }
    }

    #[tokio::test]
    async fn test_search_basic() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("test.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        let handler = SearchFilesHandler;
        let cwd = dir.path().to_str().unwrap();
        let result = handler
            .execute(
                serde_json::json!({"path": ".", "regex": "println"}),
                &ctx(cwd),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("println"));
        assert!(result.content.contains("test.rs"));
    }

    #[tokio::test]
    async fn test_search_with_file_pattern() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("test.rs"), "fn main() {}").unwrap();
        fs::write(dir.path().join("test.txt"), "fn main() {}").unwrap();
        let handler = SearchFilesHandler;
        let cwd = dir.path().to_str().unwrap();
        let result = handler
            .execute(
                serde_json::json!({"path": ".", "regex": "fn main", "file_pattern": "*.rs"}),
                &ctx(cwd),
            )
            .await
            .unwrap();
        assert!(result.content.contains("test.rs"));
        assert!(!result.content.contains("test.txt"));
    }

    #[tokio::test]
    async fn test_search_no_matches() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("test.rs"), "hello world").unwrap();
        let handler = SearchFilesHandler;
        let cwd = dir.path().to_str().unwrap();
        let result = handler
            .execute(
                serde_json::json!({"path": ".", "regex": "zzzznotfound"}),
                &ctx(cwd),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("No matches"));
    }

    #[tokio::test]
    async fn test_search_invalid_regex() {
        let handler = SearchFilesHandler;
        let result = handler
            .execute(
                serde_json::json!({"path": ".", "regex": "[invalid"}),
                &ctx("/tmp"),
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Invalid regex"));
    }

    #[tokio::test]
    async fn test_search_shows_line_numbers() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("code.rs"),
            "// comment\nfn foo() {}\n// another\nfn bar() {}\n",
        )
        .unwrap();
        let handler = SearchFilesHandler;
        let cwd = dir.path().to_str().unwrap();
        let result = handler
            .execute(
                serde_json::json!({"path": ".", "regex": "^fn"}),
                &ctx(cwd),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("2:")); // fn foo is on line 2
        assert!(result.content.contains("4:")); // fn bar is on line 4
    }

    #[tokio::test]
    async fn test_search_recursive() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn hello() {}").unwrap();
        fs::write(dir.path().join("README.md"), "hello world").unwrap();
        let handler = SearchFilesHandler;
        let cwd = dir.path().to_str().unwrap();
        let result = handler
            .execute(
                serde_json::json!({"path": ".", "regex": "hello"}),
                &ctx(cwd),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        // Should find matches in both files
        assert!(result.content.contains("2 matches"));
    }

    #[tokio::test]
    async fn test_search_nonexistent_dir() {
        let handler = SearchFilesHandler;
        let result = handler
            .execute(
                serde_json::json!({"path": "/nonexistent_xyz", "regex": "test"}),
                &ctx("/tmp"),
            )
            .await
            .unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_search_max_results() {
        let dir = TempDir::new().unwrap();
        let mut content = String::new();
        for i in 0..100 {
            content.push_str(&format!("match line {i}\n"));
        }
        fs::write(dir.path().join("many.txt"), &content).unwrap();
        let handler = SearchFilesHandler;
        let cwd = dir.path().to_str().unwrap();
        let result = handler
            .execute(
                serde_json::json!({"path": ".", "regex": "match", "max_results": 5}),
                &ctx(cwd),
            )
            .await
            .unwrap();
        assert!(result.content.contains("truncated"));
    }

    #[tokio::test]
    async fn test_search_skips_binary_files() {
        let dir = TempDir::new().unwrap();
        // Create a file with null bytes (binary)
        let mut binary_content = vec![0u8; 100];
        binary_content[50] = b'h';
        binary_content[51] = b'e';
        binary_content[52] = b'l';
        binary_content[53] = b'l';
        binary_content[54] = b'o';
        fs::write(dir.path().join("binary.bin"), &binary_content).unwrap();
        // Create a text file
        fs::write(dir.path().join("text.txt"), "hello world").unwrap();
        let handler = SearchFilesHandler;
        let cwd = dir.path().to_str().unwrap();
        let result = handler
            .execute(
                serde_json::json!({"path": ".", "regex": "hello"}),
                &ctx(cwd),
            )
            .await
            .unwrap();
        // Should only find match in text file, not binary
        assert!(result.content.contains("text.txt"));
        assert!(!result.content.contains("binary.bin"));
    }

    #[tokio::test]
    async fn test_search_missing_required_params() {
        let handler = SearchFilesHandler;
        // Missing regex
        let result = handler
            .execute(serde_json::json!({"path": "."}), &ctx("/tmp"))
            .await;
        assert!(result.is_err());

        // Missing path
        let result = handler
            .execute(serde_json::json!({"regex": "test"}), &ctx("/tmp"))
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn test_search_files_metadata() {
        let handler = SearchFilesHandler;
        assert_eq!(handler.name(), "search_files");
        assert!(!handler.requires_approval());
        assert_eq!(handler.category(), ToolCategory::ReadOnly);
    }
}

// ─── path utility tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod path_tests {
    use crate::util::path::*;
    use std::path::Path;

    #[test]
    fn test_resolve_relative() {
        let resolved = resolve_path("/home/user/project", "src/main.rs");
        assert_eq!(
            resolved,
            std::path::PathBuf::from("/home/user/project/src/main.rs")
        );
    }

    #[test]
    fn test_resolve_absolute() {
        let resolved = resolve_path("/home/user/project", "/etc/config");
        assert_eq!(resolved, std::path::PathBuf::from("/etc/config"));
    }

    #[test]
    fn test_resolve_dot() {
        let resolved = resolve_path("/home/user", ".");
        assert_eq!(resolved, std::path::PathBuf::from("/home/user/."));
    }

    #[test]
    fn test_resolve_dotdot() {
        let resolved = resolve_path("/home/user/project", "../other");
        assert_eq!(
            resolved,
            std::path::PathBuf::from("/home/user/project/../other")
        );
    }

    #[test]
    fn test_should_ignore_node_modules() {
        assert!(should_ignore(Path::new("node_modules")));
        assert!(should_ignore(Path::new("/path/to/node_modules")));
    }

    #[test]
    fn test_should_ignore_target() {
        assert!(should_ignore(Path::new("target")));
    }

    #[test]
    fn test_should_ignore_git() {
        assert!(should_ignore(Path::new(".git")));
    }

    #[test]
    fn test_should_ignore_pycache() {
        assert!(should_ignore(Path::new("__pycache__")));
    }

    #[test]
    fn test_should_not_ignore_src() {
        assert!(!should_ignore(Path::new("src")));
    }

    #[test]
    fn test_should_not_ignore_regular_file() {
        assert!(!should_ignore(Path::new("main.rs")));
    }

    #[test]
    fn test_should_ignore_venv() {
        assert!(should_ignore(Path::new(".venv")));
        assert!(should_ignore(Path::new("venv")));
    }

    #[test]
    fn test_should_ignore_ide_dirs() {
        assert!(should_ignore(Path::new(".idea")));
        assert!(should_ignore(Path::new(".vscode")));
    }

    #[test]
    fn test_is_likely_binary() {
        assert!(is_likely_binary(Path::new("image.png")));
        assert!(is_likely_binary(Path::new("archive.zip")));
        assert!(is_likely_binary(Path::new("program.exe")));
        assert!(is_likely_binary(Path::new("data.sqlite")));
        assert!(!is_likely_binary(Path::new("code.rs")));
        assert!(!is_likely_binary(Path::new("config.toml")));
        assert!(!is_likely_binary(Path::new("README.md")));
        assert!(!is_likely_binary(Path::new("noext")));
    }
}
```

## Acceptance Criteria
- [ ] read_file reads files with line numbers in `{line_no}\t{content}` format
- [ ] read_file supports line_start and line_end for partial reads
- [ ] read_file includes file metadata header (path, line count or range)
- [ ] read_file returns helpful errors for missing files, permission denied, invalid ranges
- [ ] read_file handles empty files gracefully
- [ ] list_files shows directory contents with directories first, then files alphabetically
- [ ] list_files shows file sizes in human-readable format
- [ ] list_files handles recursive traversal with depth limit (5) and entry limit (500)
- [ ] list_files respects ignore patterns (node_modules, .git, target, etc.)
- [ ] list_files shows truncation notice when entry limit is reached
- [ ] search_files finds regex matches with file:line:content format grouped by file
- [ ] search_files supports file_pattern glob filtering
- [ ] search_files skips binary files (null byte detection)
- [ ] search_files skips files larger than 1MB
- [ ] search_files limits to max_results (default 50)
- [ ] search_files returns match count summary
- [ ] All handlers return ToolResponse (never panic on bad input)
- [ ] Path resolution works for both relative and absolute paths
- [ ] should_ignore covers all common patterns
- [ ] is_likely_binary covers common binary extensions
- [ ] `cargo clippy -- -D warnings` passes
- [ ] All tests pass (30+ test cases across all handlers and utilities)
