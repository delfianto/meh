//! `search_files` tool — regex search across files in a directory.

use crate::tool::{ToolCategory, ToolContext, ToolHandler, ToolResponse};
use crate::util::path;
use async_trait::async_trait;
use std::fmt::Write;
use std::path::{Path, PathBuf};

const MAX_RESULTS: usize = 50;
const MAX_FILE_SIZE: u64 = 1_048_576;
const MAX_FILES_SCANNED: usize = 5000;

/// Handler for searching file contents with regex patterns.
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
        false
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

    #[allow(clippy::too_many_lines)]
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

        let file_pattern = params.get("file_pattern").and_then(|v| v.as_str());

        #[allow(clippy::cast_possible_truncation)]
        let max_results = params
            .get("max_results")
            .and_then(serde_json::Value::as_u64)
            .map_or(MAX_RESULTS, |v| v as usize);

        let regex = match regex::Regex::new(regex_str) {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolResponse::error(format!("Invalid regex pattern: {e}")));
            }
        };

        let glob_pattern = if let Some(pattern) = file_pattern {
            match glob::Pattern::new(pattern) {
                Ok(p) => Some(p),
                Err(e) => {
                    return Ok(ToolResponse::error(format!("Invalid file pattern: {e}")));
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

        let files = collect_files(&full_path, glob_pattern.as_ref()).await;
        let (matches, files_scanned, files_with_matches) =
            search_in_files(&files, &full_path, &regex, max_results).await;

        if matches.is_empty() {
            return Ok(ToolResponse::success(format!(
                "No matches found for '{regex_str}' in {search_path} ({files_scanned} files scanned)"
            )));
        }

        let output = format_matches(&matches, files_with_matches, files_scanned, max_results);
        Ok(ToolResponse::success(output))
    }
}

/// Search through a list of files for regex matches.
async fn search_in_files(
    files: &[PathBuf],
    base_path: &Path,
    regex: &regex::Regex,
    max_results: usize,
) -> (Vec<(String, usize, String)>, usize, usize) {
    let mut matches: Vec<(String, usize, String)> = Vec::new();
    let mut files_scanned = 0;
    let mut files_with_matches = 0;

    for file_path in files {
        if matches.len() >= max_results {
            break;
        }

        files_scanned += 1;

        let Ok(metadata) = tokio::fs::metadata(file_path).await else {
            continue;
        };
        if metadata.len() > MAX_FILE_SIZE {
            continue;
        }

        let Ok(content) = tokio::fs::read(file_path).await else {
            continue;
        };

        let check_len = content.len().min(512);
        if content[..check_len].contains(&0) {
            continue;
        }

        let Ok(text) = String::from_utf8(content) else {
            continue;
        };

        let relative_path = file_path
            .strip_prefix(base_path)
            .unwrap_or(file_path)
            .to_string_lossy()
            .to_string();

        let mut file_had_match = false;
        for (line_no, line) in text.lines().enumerate() {
            if matches.len() >= max_results {
                break;
            }

            if regex.is_match(line) {
                matches.push((relative_path.clone(), line_no + 1, line.to_string()));
                file_had_match = true;
            }
        }

        if file_had_match {
            files_with_matches += 1;
        }
    }

    (matches, files_scanned, files_with_matches)
}

/// Format search matches into a grouped output string.
fn format_matches(
    matches: &[(String, usize, String)],
    files_with_matches: usize,
    files_scanned: usize,
    max_results: usize,
) -> String {
    let mut output = String::new();
    let mut current_file = String::new();

    for (file, line_no, content) in matches {
        if file != &current_file {
            if !current_file.is_empty() {
                output.push('\n');
            }
            let _ = writeln!(output, "{file}:");
            current_file.clone_from(file);
        }
        let display_content = if content.len() > 200 {
            format!("{}...", &content[..200])
        } else {
            content.clone()
        };
        let _ = writeln!(output, "  {line_no}: {display_content}");
    }

    let _ = write!(
        output,
        "\n{} matches in {files_with_matches} files ({files_scanned} files scanned)",
        matches.len(),
    );

    if matches.len() >= max_results {
        let _ = write!(output, "\n(results truncated at {max_results} matches)");
    }

    output
}

/// Recursively collect all files in a directory, respecting ignore patterns and glob filter.
async fn collect_files(dir: &Path, glob_pattern: Option<&glob::Pattern>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    let mut total_files = 0;

    while let Some(current) = stack.pop() {
        if total_files >= MAX_FILES_SCANNED {
            break;
        }

        let Ok(mut entries) = tokio::fs::read_dir(&current).await else {
            continue;
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let entry_path = entry.path();

            if path::should_ignore(&entry_path) {
                continue;
            }

            let Ok(metadata) = entry.metadata().await else {
                continue;
            };

            if metadata.is_dir() {
                stack.push(entry_path);
            } else if metadata.is_file() {
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

    files.sort();
    files
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
            .execute(serde_json::json!({"path": ".", "regex": "^fn"}), &ctx(cwd))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("2:"));
        assert!(result.content.contains("4:"));
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
            let _ = writeln!(content, "match line {i}");
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
        let mut binary_content = vec![0u8; 100];
        binary_content[50] = b'h';
        binary_content[51] = b'e';
        binary_content[52] = b'l';
        binary_content[53] = b'l';
        binary_content[54] = b'o';
        fs::write(dir.path().join("binary.bin"), &binary_content).unwrap();
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
        assert!(result.content.contains("text.txt"));
        assert!(!result.content.contains("binary.bin"));
    }

    #[tokio::test]
    async fn test_search_missing_required_params() {
        let handler = SearchFilesHandler;
        let result = handler
            .execute(serde_json::json!({"path": "."}), &ctx("/tmp"))
            .await;
        assert!(result.is_err());

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
