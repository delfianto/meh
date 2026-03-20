# STEP 14 — Write Tool Handlers (write_file, apply_patch)

## Objective
Implement file modification tools. After this step, the LLM can create/overwrite files and apply multi-file patches.

## Prerequisites
- STEP 11-12 complete
- STEP 13 complete (permission system gates these)

## Detailed Instructions

### 14.1 WriteFileHandler (`src/tool/handlers/write_file.rs`)

```rust
//! write_file tool — create or overwrite files.

use crate::tool::{ToolHandler, ToolCategory, ToolContext, ToolResponse};
use async_trait::async_trait;
use std::path::Path;

pub struct WriteFileHandler;

#[async_trait]
impl ToolHandler for WriteFileHandler {
    fn name(&self) -> &str { "write_file" }
    fn description(&self) -> &str {
        "Create a new file or overwrite an existing file with the provided content."
    }
    fn requires_approval(&self) -> bool { true }
    fn category(&self) -> ToolCategory { ToolCategory::FileWrite }

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

    async fn execute(&self, params: serde_json::Value, ctx: &ToolContext) -> anyhow::Result<ToolResponse> {
        let path_str = params.get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: path"))?;
        let content = params.get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: content"))?;

        let full_path = crate::util::path::resolve_path(&ctx.cwd, path_str);

        // Create parent directories if needed
        if let Some(parent) = full_path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| anyhow::anyhow!("Failed to create directories: {e}"))?;
        }

        // Check if file existed
        let existed = full_path.exists();

        // Write file
        tokio::fs::write(&full_path, content).await
            .map_err(|e| anyhow::anyhow!("Failed to write file: {e}"))?;

        let action = if existed { "Updated" } else { "Created" };
        let line_count = content.lines().count();
        Ok(ToolResponse::success(format!(
            "{action} file: {path_str} ({line_count} lines)"
        )))
    }
}
```

### 14.2 ApplyPatchHandler (`src/tool/handlers/apply_patch.rs`)

Implement a unified diff patch applicator supporting multi-file patches.

```rust
//! apply_patch tool — apply unified diff patches to one or more files.

pub struct ApplyPatchHandler;
```

**Patch format supported** (simplified unified diff):
```
--- a/path/to/file.rs
+++ b/path/to/file.rs
@@ -10,5 +10,6 @@
 unchanged line
-removed line
+added line
+another added line
 unchanged line
```

**Implementation details**:

1. Parse the patch text:
   - Split into file hunks by `--- a/` / `+++ b/` markers
   - For each file hunk, split into change hunks by `@@` markers
   - Parse `@@ -old_start,old_count +new_start,new_count @@` header
   - Lines starting with ` ` are context, `-` are removals, `+` are additions

2. Apply each hunk:
   - Read the original file
   - Split into lines
   - For each hunk, find the matching position (using context lines for fuzzy matching)
   - Apply removals and additions
   - Write the modified file

3. For new files (--- /dev/null): create the file with added lines
4. For deleted files (+++ /dev/null): delete the file

Input schema:
```json
{
    "type": "object",
    "required": ["patch"],
    "properties": {
        "patch": {
            "type": "string",
            "description": "Unified diff patch text"
        }
    }
}
```

Use the `similar` crate for diff utilities if needed, but implement patch application manually for control.

**Fuzzy matching**: If the exact context lines don't match at the expected position, search nearby (plus or minus 5 lines) for a matching context window. This handles cases where prior patches shifted line numbers.

### 14.3 File system utility (`src/util/fs.rs`)

```rust
//! File system utilities.

/// Read a file, returning (content, existed).
pub async fn read_file_if_exists(path: &std::path::Path) -> anyhow::Result<Option<String>> {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Write a file, creating parent directories as needed.
pub async fn write_file_safe(path: &std::path::Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, content).await?;
    Ok(())
}

/// Delete a file if it exists.
pub async fn delete_file_if_exists(path: &std::path::Path) -> anyhow::Result<bool> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
```

## Tests

```rust
// write_file tests
#[cfg(test)]
mod write_file_tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_write_new_file() {
        let dir = TempDir::new().unwrap();
        let ctx = ToolContext { cwd: dir.path().to_str().unwrap().to_string(), auto_approved: true };
        let handler = WriteFileHandler;
        let result = handler.execute(serde_json::json!({
            "path": "new_file.txt",
            "content": "hello world\n"
        }), &ctx).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("Created"));
        let content = std::fs::read_to_string(dir.path().join("new_file.txt")).unwrap();
        assert_eq!(content, "hello world\n");
    }

    #[tokio::test]
    async fn test_write_overwrites_existing() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("existing.txt"), "old content").unwrap();
        let ctx = ToolContext { cwd: dir.path().to_str().unwrap().to_string(), auto_approved: true };
        let handler = WriteFileHandler;
        let result = handler.execute(serde_json::json!({
            "path": "existing.txt",
            "content": "new content"
        }), &ctx).await.unwrap();
        assert!(result.content.contains("Updated"));
        let content = std::fs::read_to_string(dir.path().join("existing.txt")).unwrap();
        assert_eq!(content, "new content");
    }

    #[tokio::test]
    async fn test_write_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let ctx = ToolContext { cwd: dir.path().to_str().unwrap().to_string(), auto_approved: true };
        let handler = WriteFileHandler;
        let result = handler.execute(serde_json::json!({
            "path": "deep/nested/dir/file.txt",
            "content": "nested content"
        }), &ctx).await.unwrap();
        assert!(!result.is_error);
        assert!(dir.path().join("deep/nested/dir/file.txt").exists());
    }

    #[tokio::test]
    async fn test_write_missing_params() {
        let ctx = ToolContext { cwd: "/tmp".to_string(), auto_approved: true };
        let handler = WriteFileHandler;
        assert!(handler.execute(serde_json::json!({"path": "test.txt"}), &ctx).await.is_err());
        assert!(handler.execute(serde_json::json!({"content": "test"}), &ctx).await.is_err());
    }
}

// apply_patch tests
#[cfg(test)]
mod apply_patch_tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_apply_simple_patch() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.rs"), "fn main() {\n    println!(\"hello\");\n}\n").unwrap();
        let ctx = ToolContext { cwd: dir.path().to_str().unwrap().to_string(), auto_approved: true };
        let handler = ApplyPatchHandler;
        let patch = r#"--- a/test.rs
+++ b/test.rs
@@ -1,3 +1,3 @@
 fn main() {
-    println!("hello");
+    println!("world");
 }
"#;
        let result = handler.execute(serde_json::json!({"patch": patch}), &ctx).await.unwrap();
        assert!(!result.is_error);
        let content = std::fs::read_to_string(dir.path().join("test.rs")).unwrap();
        assert!(content.contains("world"));
        assert!(!content.contains("hello"));
    }

    #[tokio::test]
    async fn test_apply_patch_new_file() {
        let dir = TempDir::new().unwrap();
        let ctx = ToolContext { cwd: dir.path().to_str().unwrap().to_string(), auto_approved: true };
        let handler = ApplyPatchHandler;
        let patch = r#"--- /dev/null
+++ b/new_file.rs
@@ -0,0 +1,3 @@
+fn new_function() {
+    // new code
+}
"#;
        let result = handler.execute(serde_json::json!({"patch": patch}), &ctx).await.unwrap();
        assert!(!result.is_error);
        assert!(dir.path().join("new_file.rs").exists());
    }

    #[tokio::test]
    async fn test_apply_patch_delete_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("delete_me.rs"), "old content").unwrap();
        let ctx = ToolContext { cwd: dir.path().to_str().unwrap().to_string(), auto_approved: true };
        let handler = ApplyPatchHandler;
        let patch = r#"--- a/delete_me.rs
+++ /dev/null
@@ -1 +0,0 @@
-old content
"#;
        let result = handler.execute(serde_json::json!({"patch": patch}), &ctx).await.unwrap();
        assert!(!result.is_error);
        assert!(!dir.path().join("delete_me.rs").exists());
    }

    #[tokio::test]
    async fn test_apply_multi_file_patch() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.rs"), "line1\nline2\n").unwrap();
        std::fs::write(dir.path().join("b.rs"), "foo\nbar\n").unwrap();
        let ctx = ToolContext { cwd: dir.path().to_str().unwrap().to_string(), auto_approved: true };
        let handler = ApplyPatchHandler;
        let patch = r#"--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,2 @@
 line1
-line2
+line2_modified
--- a/b.rs
+++ b/b.rs
@@ -1,2 +1,2 @@
 foo
-bar
+baz
"#;
        let result = handler.execute(serde_json::json!({"patch": patch}), &ctx).await.unwrap();
        assert!(!result.is_error);
        assert!(std::fs::read_to_string(dir.path().join("a.rs")).unwrap().contains("line2_modified"));
        assert!(std::fs::read_to_string(dir.path().join("b.rs")).unwrap().contains("baz"));
    }

    #[tokio::test]
    async fn test_apply_patch_context_mismatch() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.rs"), "different\ncontent\nhere\n").unwrap();
        let ctx = ToolContext { cwd: dir.path().to_str().unwrap().to_string(), auto_approved: true };
        let handler = ApplyPatchHandler;
        let patch = r#"--- a/test.rs
+++ b/test.rs
@@ -1,3 +1,3 @@
 not matching
-context lines
+new line
 at all
"#;
        let result = handler.execute(serde_json::json!({"patch": patch}), &ctx).await.unwrap();
        // Should return error since context doesn't match
        assert!(result.is_error);
    }
}

// fs utility tests
#[cfg(test)]
mod fs_tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_read_file_if_exists() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, "content").unwrap();
        assert_eq!(read_file_if_exists(&path).await.unwrap(), Some("content".to_string()));
    }

    #[tokio::test]
    async fn test_read_file_not_exists() {
        let path = std::path::Path::new("/nonexistent/file.txt");
        assert_eq!(read_file_if_exists(path).await.unwrap(), None);
    }

    #[tokio::test]
    async fn test_write_file_safe() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a/b/c.txt");
        write_file_safe(&path, "nested").await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "nested");
    }

    #[tokio::test]
    async fn test_delete_file_if_exists() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("del.txt");
        std::fs::write(&path, "x").unwrap();
        assert!(delete_file_if_exists(&path).await.unwrap());
        assert!(!delete_file_if_exists(&path).await.unwrap()); // Already gone
    }
}
```

## Acceptance Criteria
- [x] write_file creates new files and overwrites existing ones
- [x] write_file creates parent directories as needed
- [x] apply_patch applies single-file unified diffs
- [x] apply_patch handles multi-file patches
- [x] apply_patch creates new files (--- /dev/null)
- [x] apply_patch deletes files (+++ /dev/null)
- [x] apply_patch reports error on context mismatch
- [x] Fuzzy context matching handles plus or minus 5 line offset
- [x] Both tools require approval (category = FileWrite)
- [x] `cargo clippy -- -D warnings` passes
- [x] All tests pass (21 test cases)

**Completed**: PR #11
