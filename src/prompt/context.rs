//! Workspace context — file tree summary for system prompt.
//!
//! Scans the workspace directory and builds a compact file tree,
//! respecting `.mehignore` rules. Hidden directories are skipped
//! except `.github` and `.vscode`.

use crate::ignore::IgnoreController;
use std::fmt::Write as _;
use std::path::Path;

/// Build a workspace context section showing the file tree.
///
/// Scans up to `max_depth` levels and `max_entries` total entries.
pub fn workspace_context(
    cwd: &Path,
    ignore: &IgnoreController,
    max_depth: usize,
    max_entries: usize,
) -> String {
    let mut entries = Vec::new();
    collect_tree(cwd, ignore, 0, max_depth, &mut entries, max_entries);

    if entries.is_empty() {
        return String::new();
    }

    let mut s = String::from("# Workspace Structure\n```\n");
    for (depth, name, is_dir) in &entries {
        let indent = "  ".repeat(*depth);
        let suffix = if *is_dir { "/" } else { "" };
        let _ = writeln!(s, "{indent}{name}{suffix}");
    }
    if entries.len() >= max_entries {
        s.push_str("  ... (truncated)\n");
    }
    s.push_str("```\n");
    s
}

/// Recursively collect directory entries into a flat list.
fn collect_tree(
    dir: &Path,
    ignore: &IgnoreController,
    depth: usize,
    max_depth: usize,
    entries: &mut Vec<(usize, String, bool)>,
    max_entries: usize,
) {
    if depth > max_depth || entries.len() >= max_entries {
        return;
    }

    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };

    let mut children: Vec<_> = read_dir
        .flatten()
        .filter(|e| ignore.is_allowed(&e.path()))
        .collect();

    children.sort_by(|a, b| {
        let a_dir = a.path().is_dir();
        let b_dir = b.path().is_dir();
        b_dir.cmp(&a_dir).then(a.file_name().cmp(&b.file_name()))
    });

    for entry in children {
        if entries.len() >= max_entries {
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') && !matches!(name.as_str(), ".github" | ".vscode") {
            continue;
        }
        let is_dir = entry.path().is_dir();
        entries.push((depth, name, is_dir));
        if is_dir {
            collect_tree(
                &entry.path(),
                ignore,
                depth + 1,
                max_depth,
                entries,
                max_entries,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn empty_directory() {
        let dir = TempDir::new().unwrap();
        let ignore = IgnoreController::permissive(dir.path());
        let ctx = workspace_context(dir.path(), &ignore, 3, 100);
        assert!(ctx.is_empty());
    }

    #[test]
    fn basic_tree() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("main.rs"), "").unwrap();
        std::fs::write(dir.path().join("lib.rs"), "").unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/mod.rs"), "").unwrap();

        let ignore = IgnoreController::permissive(dir.path());
        let ctx = workspace_context(dir.path(), &ignore, 3, 100);
        assert!(ctx.contains("Workspace Structure"));
        assert!(ctx.contains("src/"));
        assert!(ctx.contains("main.rs"));
    }

    #[test]
    fn max_entries_truncation() {
        let dir = TempDir::new().unwrap();
        for i in 0..20 {
            std::fs::write(dir.path().join(format!("file{i}.rs")), "").unwrap();
        }
        let ignore = IgnoreController::permissive(dir.path());
        let ctx = workspace_context(dir.path(), &ignore, 3, 5);
        assert!(ctx.contains("(truncated)"));
    }

    #[test]
    fn skips_hidden_dirs() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".hidden")).unwrap();
        std::fs::write(dir.path().join(".hidden/secret"), "").unwrap();
        std::fs::write(dir.path().join("visible.rs"), "").unwrap();

        let ignore = IgnoreController::permissive(dir.path());
        let ctx = workspace_context(dir.path(), &ignore, 3, 100);
        assert!(!ctx.contains(".hidden"));
        assert!(ctx.contains("visible.rs"));
    }

    #[test]
    fn allows_github_dir() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".github")).unwrap();
        std::fs::write(dir.path().join(".github/workflows.yml"), "").unwrap();

        let ignore = IgnoreController::permissive(dir.path());
        let ctx = workspace_context(dir.path(), &ignore, 3, 100);
        assert!(ctx.contains(".github/"));
    }

    #[test]
    fn directories_sorted_first() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("z_file.rs"), "").unwrap();
        std::fs::create_dir(dir.path().join("a_dir")).unwrap();

        let ignore = IgnoreController::permissive(dir.path());
        let ctx = workspace_context(dir.path(), &ignore, 3, 100);
        let dir_pos = ctx.find("a_dir/").unwrap();
        let file_pos = ctx.find("z_file.rs").unwrap();
        assert!(dir_pos < file_pos);
    }

    #[test]
    fn max_depth_respected() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b/c/d")).unwrap();
        std::fs::write(dir.path().join("a/b/c/d/deep.rs"), "").unwrap();

        let ignore = IgnoreController::permissive(dir.path());
        let ctx = workspace_context(dir.path(), &ignore, 1, 100);
        assert!(!ctx.contains("deep.rs"));
    }
}
