//! `apply_patch` tool — apply unified diff patches to one or more files.

use crate::tool::{ToolCategory, ToolContext, ToolHandler, ToolResponse};
use crate::util::path::resolve_path;
use async_trait::async_trait;
use std::fmt::Write;

const FUZZY_RANGE: usize = 5;

/// Handler for applying unified diff patches.
pub struct ApplyPatchHandler;

#[async_trait]
impl ToolHandler for ApplyPatchHandler {
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn description(&self) -> &str {
        "Apply a unified diff patch to modify existing files. Preferred over write_to_file for targeted edits."
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::FileWrite
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["patch"],
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "Unified diff patch text"
                }
            }
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolResponse> {
        let patch_text = params
            .get("patch")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: patch"))?;

        let file_patches = match parse_patch(patch_text) {
            Ok(p) => p,
            Err(e) => return Ok(ToolResponse::error(format!("Failed to parse patch: {e}"))),
        };

        if file_patches.is_empty() {
            return Ok(ToolResponse::error(
                "No file changes found in patch".to_string(),
            ));
        }

        let mut summary = String::new();
        for fp in &file_patches {
            match apply_file_patch(fp, &ctx.cwd).await {
                Ok(msg) => {
                    let _ = writeln!(summary, "{msg}");
                }
                Err(e) => return Ok(ToolResponse::error(format!("Patch failed: {e}"))),
            }
        }

        Ok(ToolResponse::success(summary.trim_end().to_string()))
    }
}

/// A parsed patch for a single file.
struct FilePatch {
    old_path: String,
    new_path: String,
    hunks: Vec<Hunk>,
}

/// A single hunk within a file patch.
struct Hunk {
    old_start: usize,
    lines: Vec<HunkLine>,
}

/// A line within a hunk.
enum HunkLine {
    Context(String),
    Remove(String),
    Add(String),
}

/// Parse a unified diff patch into per-file patches.
fn parse_patch(text: &str) -> anyhow::Result<Vec<FilePatch>> {
    let mut patches = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        if lines[i].starts_with("--- ") {
            if i + 1 >= lines.len() || !lines[i + 1].starts_with("+++ ") {
                anyhow::bail!("Expected +++ line after --- at line {}", i + 1);
            }

            let old_path = strip_path_prefix(lines[i].trim_start_matches("--- "));
            let new_path = strip_path_prefix(lines[i + 1].trim_start_matches("+++ "));
            i += 2;

            let mut hunks = Vec::new();
            while i < lines.len() && lines[i].starts_with("@@ ") {
                let (hunk, next_i) = parse_hunk(&lines, i)?;
                hunks.push(hunk);
                i = next_i;
            }

            patches.push(FilePatch {
                old_path,
                new_path,
                hunks,
            });
        } else {
            i += 1;
        }
    }

    Ok(patches)
}

/// Strip `a/` or `b/` prefix from diff paths.
fn strip_path_prefix(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.starts_with("a/") || trimmed.starts_with("b/") {
        trimmed[2..].to_string()
    } else {
        trimmed.to_string()
    }
}

/// Parse a single hunk starting at the `@@` line.
fn parse_hunk(lines: &[&str], start: usize) -> anyhow::Result<(Hunk, usize)> {
    let header = lines[start];
    let old_start = parse_hunk_header(header)?;

    let mut hunk_lines = Vec::new();
    let mut i = start + 1;

    while i < lines.len() {
        let line = lines[i];
        if line.starts_with("@@ ") || line.starts_with("--- ") {
            break;
        }

        if let Some(rest) = line.strip_prefix('-') {
            hunk_lines.push(HunkLine::Remove(rest.to_string()));
        } else if let Some(rest) = line.strip_prefix('+') {
            hunk_lines.push(HunkLine::Add(rest.to_string()));
        } else if let Some(rest) = line.strip_prefix(' ') {
            hunk_lines.push(HunkLine::Context(rest.to_string()));
        } else if line.is_empty() {
            hunk_lines.push(HunkLine::Context(String::new()));
        } else if line == "\\ No newline at end of file" {
            // Skip this marker
        } else {
            hunk_lines.push(HunkLine::Context(line.to_string()));
        }

        i += 1;
    }

    Ok((
        Hunk {
            old_start,
            lines: hunk_lines,
        },
        i,
    ))
}

/// Parse the `@@ -old_start,count +new_start,count @@` header.
fn parse_hunk_header(header: &str) -> anyhow::Result<usize> {
    let parts: Vec<&str> = header.split_whitespace().collect();
    if parts.len() < 3 || parts[0] != "@@" {
        anyhow::bail!("Invalid hunk header: {header}");
    }

    let old_range = parts[1].trim_start_matches('-');
    let old_start: usize = old_range
        .split(',')
        .next()
        .unwrap_or("0")
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid old start in hunk header: {header}"))?;

    Ok(old_start)
}

/// Apply a single file patch.
async fn apply_file_patch(fp: &FilePatch, cwd: &str) -> anyhow::Result<String> {
    let is_new_file = fp.old_path == "/dev/null";
    let is_delete = fp.new_path == "/dev/null";

    if is_delete {
        let path = resolve_path(cwd, &fp.old_path);
        crate::util::fs::delete_file_if_exists(&path).await?;
        return Ok(format!("Deleted: {}", fp.old_path));
    }

    if is_new_file {
        let path = resolve_path(cwd, &fp.new_path);
        let mut content = String::new();
        for line in &fp.hunks.iter().flat_map(|h| &h.lines).collect::<Vec<_>>() {
            if let HunkLine::Add(text) = line {
                content.push_str(text);
                content.push('\n');
            }
        }
        crate::util::fs::write_file_safe(&path, &content).await?;
        return Ok(format!("Created: {}", fp.new_path));
    }

    let path = resolve_path(cwd, &fp.new_path);
    let original = crate::util::fs::read_file_if_exists(&path)
        .await?
        .ok_or_else(|| anyhow::anyhow!("File not found: {}", fp.new_path))?;

    let mut lines: Vec<String> = original.lines().map(String::from).collect();

    let mut offset: isize = 0;

    for hunk in &fp.hunks {
        let context_lines: Vec<&str> = hunk
            .lines
            .iter()
            .filter_map(|l| match l {
                HunkLine::Context(s) | HunkLine::Remove(s) => Some(s.as_str()),
                HunkLine::Add(_) => None,
            })
            .collect();

        #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
        let target = (hunk.old_start as isize - 1 + offset).max(0) as usize;
        let match_pos = find_match_position(&lines, &context_lines, target)?;

        let (removed, added) = apply_hunk_at(&mut lines, &hunk.lines, match_pos);
        #[allow(clippy::cast_possible_wrap)]
        {
            offset += added as isize - removed as isize;
        }
    }

    let mut result = lines.join("\n");
    if original.ends_with('\n') && !result.ends_with('\n') {
        result.push('\n');
    }

    crate::util::fs::write_file_safe(&path, &result).await?;
    Ok(format!(
        "Patched: {} ({} hunks)",
        fp.new_path,
        fp.hunks.len()
    ))
}

/// Find the position where context lines match, with fuzzy search.
fn find_match_position(lines: &[String], context: &[&str], target: usize) -> anyhow::Result<usize> {
    if context.is_empty() {
        return Ok(target.min(lines.len()));
    }

    let start = target.saturating_sub(FUZZY_RANGE);
    let end = (target + FUZZY_RANGE + 1).min(lines.len().saturating_sub(context.len()) + 1);

    for pos in start..end {
        if matches_at(lines, context, pos) {
            return Ok(pos);
        }
    }

    anyhow::bail!(
        "Context mismatch: could not find matching lines near line {}",
        target + 1
    )
}

/// Check if context lines match at the given position.
fn matches_at(lines: &[String], context: &[&str], pos: usize) -> bool {
    if pos + context.len() > lines.len() {
        return false;
    }
    context
        .iter()
        .zip(&lines[pos..])
        .all(|(ctx, line)| *ctx == line)
}

/// Apply a hunk at the given position. Returns `(removed_count, added_count)`.
fn apply_hunk_at(
    lines: &mut Vec<String>,
    hunk_lines: &[HunkLine],
    mut pos: usize,
) -> (usize, usize) {
    let mut removed = 0;
    let mut added = 0;

    for hl in hunk_lines {
        match hl {
            HunkLine::Context(_) => {
                pos += 1;
            }
            HunkLine::Remove(_) => {
                if pos < lines.len() {
                    lines.remove(pos);
                    removed += 1;
                }
            }
            HunkLine::Add(text) => {
                lines.insert(pos, text.clone());
                pos += 1;
                added += 1;
            }
        }
    }

    (removed, added)
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
    async fn test_apply_simple_patch() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("test.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        let cwd = dir.path().to_str().unwrap();
        let handler = ApplyPatchHandler;
        let patch = "--- a/test.rs\n+++ b/test.rs\n@@ -1,3 +1,3 @@\n fn main() {\n-    println!(\"hello\");\n+    println!(\"world\");\n }\n";
        let result = handler
            .execute(serde_json::json!({"patch": patch}), &ctx(cwd))
            .await
            .unwrap();
        assert!(!result.is_error, "Error: {}", result.content);
        let content = fs::read_to_string(dir.path().join("test.rs")).unwrap();
        assert!(content.contains("world"));
        assert!(!content.contains("hello"));
    }

    #[tokio::test]
    async fn test_apply_patch_new_file() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_str().unwrap();
        let handler = ApplyPatchHandler;
        let patch = "--- /dev/null\n+++ b/new_file.rs\n@@ -0,0 +1,3 @@\n+fn new_function() {\n+    // new code\n+}\n";
        let result = handler
            .execute(serde_json::json!({"patch": patch}), &ctx(cwd))
            .await
            .unwrap();
        assert!(!result.is_error, "Error: {}", result.content);
        assert!(dir.path().join("new_file.rs").exists());
        let content = fs::read_to_string(dir.path().join("new_file.rs")).unwrap();
        assert!(content.contains("new_function"));
    }

    #[tokio::test]
    async fn test_apply_patch_delete_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("delete_me.rs"), "old content").unwrap();
        let cwd = dir.path().to_str().unwrap();
        let handler = ApplyPatchHandler;
        let patch = "--- a/delete_me.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-old content\n";
        let result = handler
            .execute(serde_json::json!({"patch": patch}), &ctx(cwd))
            .await
            .unwrap();
        assert!(!result.is_error, "Error: {}", result.content);
        assert!(!dir.path().join("delete_me.rs").exists());
    }

    #[tokio::test]
    async fn test_apply_multi_file_patch() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.rs"), "line1\nline2\n").unwrap();
        fs::write(dir.path().join("b.rs"), "foo\nbar\n").unwrap();
        let cwd = dir.path().to_str().unwrap();
        let handler = ApplyPatchHandler;
        let patch = "--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,2 @@\n line1\n-line2\n+line2_modified\n--- a/b.rs\n+++ b/b.rs\n@@ -1,2 +1,2 @@\n foo\n-bar\n+baz\n";
        let result = handler
            .execute(serde_json::json!({"patch": patch}), &ctx(cwd))
            .await
            .unwrap();
        assert!(!result.is_error, "Error: {}", result.content);
        assert!(
            fs::read_to_string(dir.path().join("a.rs"))
                .unwrap()
                .contains("line2_modified")
        );
        assert!(
            fs::read_to_string(dir.path().join("b.rs"))
                .unwrap()
                .contains("baz")
        );
    }

    #[tokio::test]
    async fn test_apply_patch_context_mismatch() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("test.rs"), "different\ncontent\nhere\n").unwrap();
        let cwd = dir.path().to_str().unwrap();
        let handler = ApplyPatchHandler;
        let patch = "--- a/test.rs\n+++ b/test.rs\n@@ -1,3 +1,3 @@\n not matching\n-context lines\n+new line\n at all\n";
        let result = handler
            .execute(serde_json::json!({"patch": patch}), &ctx(cwd))
            .await
            .unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_apply_patch_missing_param() {
        let handler = ApplyPatchHandler;
        assert!(
            handler
                .execute(serde_json::json!({}), &ctx("/tmp"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_apply_patch_empty() {
        let handler = ApplyPatchHandler;
        let result = handler
            .execute(serde_json::json!({"patch": ""}), &ctx("/tmp"))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("No file changes"));
    }

    #[test]
    fn test_strip_path_prefix() {
        assert_eq!(strip_path_prefix("a/src/main.rs"), "src/main.rs");
        assert_eq!(strip_path_prefix("b/src/main.rs"), "src/main.rs");
        assert_eq!(strip_path_prefix("/dev/null"), "/dev/null");
        assert_eq!(strip_path_prefix("plain.rs"), "plain.rs");
    }

    #[test]
    fn test_parse_hunk_header() {
        assert_eq!(parse_hunk_header("@@ -10,5 +12,7 @@").unwrap(), 10);
        assert_eq!(parse_hunk_header("@@ -1,3 +1,3 @@ fn main()").unwrap(), 1);
        assert_eq!(parse_hunk_header("@@ -0,0 +1,5 @@").unwrap(), 0);
    }

    #[test]
    fn test_matches_at() {
        let lines: Vec<String> = vec!["a", "b", "c", "d"]
            .into_iter()
            .map(String::from)
            .collect();
        assert!(matches_at(&lines, &["a", "b"], 0));
        assert!(matches_at(&lines, &["b", "c"], 1));
        assert!(!matches_at(&lines, &["a", "c"], 0));
        assert!(!matches_at(&lines, &["c", "d", "e"], 2));
    }

    #[test]
    fn test_apply_patch_metadata() {
        let handler = ApplyPatchHandler;
        assert_eq!(handler.name(), "apply_patch");
        assert!(handler.requires_approval());
        assert_eq!(handler.category(), ToolCategory::FileWrite);
    }
}
