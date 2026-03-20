# STEP 32 — .mehignore Path Protection

## Objective
Implement `.mehignore` file support using the `ignore` crate. Tools respect this file to prevent reading, writing, or searching protected paths. Security is fail-closed.

## Prerequisites
- STEP 12 (read-only tools), STEP 14 (write tools), STEP 15 (execute_command)

## Detailed Instructions

### 32.1 IgnoreController (`src/ignore/mod.rs`)

```rust
//! Path protection via .mehignore — prevents tools from accessing protected files.

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::{Path, PathBuf};
use notify::Watcher;

pub struct IgnoreController {
    gitignore: Option<Gitignore>,
    workspace_root: PathBuf,
}

impl IgnoreController {
    /// Load .mehignore from workspace root. Missing file = no restrictions.
    pub fn new(workspace_root: &Path) -> anyhow::Result<Self> {
        let ignore_path = workspace_root.join(".mehignore");
        let gitignore = if ignore_path.exists() {
            let mut builder = GitignoreBuilder::new(workspace_root);
            builder.add(&ignore_path);
            Some(builder.build()?)
        } else {
            None
        };
        Ok(Self { gitignore, workspace_root: workspace_root.to_path_buf() })
    }

    /// Check if a path is accessible (not ignored).
    pub fn is_allowed(&self, path: &Path) -> bool {
        // Paths outside workspace are always allowed
        let relative = match path.strip_prefix(&self.workspace_root) {
            Ok(rel) => rel,
            Err(_) => return true,
        };
        match &self.gitignore {
            Some(gi) => !gi.matched(relative, path.is_dir()).is_ignore(),
            None => true,
        }
    }

    /// Filter a list of paths, returning only accessible ones.
    /// On error, returns empty vec (fail-closed).
    pub fn filter_paths(&self, paths: &[PathBuf]) -> Vec<PathBuf> {
        paths.iter().filter(|p| self.is_allowed(p)).cloned().collect()
    }

    /// Validate a command string — check if any file arguments are protected.
    pub fn validate_command(&self, command: &str) -> bool {
        // Parse command for file-like arguments (paths)
        // Check each against ignore rules
        // Return false if any protected path referenced
        true // TODO: implement shell argument extraction
    }

    /// Reload .mehignore (called on file change).
    pub fn reload(&mut self) -> anyhow::Result<()> {
        *self = Self::new(&self.workspace_root)?;
        Ok(())
    }
}
```

### 32.2 Integration with tool handlers

Every tool that accesses files must check the IgnoreController:
- `read_file`: Check before reading
- `write_file`: Check before writing
- `apply_patch`: Check each file in the patch
- `list_files`: Filter results
- `search_files`: Filter results
- `execute_command`: Validate command arguments

Add `ignore_controller: Arc<IgnoreController>` to `ToolContext`.

### 32.3 Default .mehignore patterns

When no `.mehignore` exists, apply sensible defaults:
```
# Secrets
.env
.env.*
*.pem
*.key
credentials.json
secrets.yaml

# Large binaries
*.zip
*.tar.gz
*.dmg
*.exe
```

### 32.4 File watcher integration

Use the existing STEP 29 file watcher infrastructure to watch `.mehignore` for changes and call `reload()`.

## Tests

```rust
#[cfg(test)]
mod ignore_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_no_ignore_file_allows_all() {
        let dir = TempDir::new().unwrap();
        let ctrl = IgnoreController::new(dir.path()).unwrap();
        assert!(ctrl.is_allowed(&dir.path().join("anything.txt")));
    }

    #[test]
    fn test_ignore_pattern() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".mehignore"), "*.secret\nnode_modules/\n").unwrap();
        let ctrl = IgnoreController::new(dir.path()).unwrap();
        assert!(!ctrl.is_allowed(&dir.path().join("password.secret")));
        assert!(!ctrl.is_allowed(&dir.path().join("node_modules/pkg/index.js")));
        assert!(ctrl.is_allowed(&dir.path().join("src/main.rs")));
    }

    #[test]
    fn test_outside_workspace_always_allowed() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".mehignore"), "*").unwrap();
        let ctrl = IgnoreController::new(dir.path()).unwrap();
        assert!(ctrl.is_allowed(Path::new("/etc/hosts"))); // Outside workspace
    }

    #[test]
    fn test_filter_paths() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".mehignore"), "*.log\n").unwrap();
        let ctrl = IgnoreController::new(dir.path()).unwrap();
        let paths = vec![
            dir.path().join("main.rs"),
            dir.path().join("debug.log"),
            dir.path().join("src/lib.rs"),
        ];
        let filtered = ctrl.filter_paths(&paths);
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|p| !p.to_str().unwrap().contains(".log")));
    }

    #[test]
    fn test_negation_pattern() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".mehignore"), "*.log\n!important.log\n").unwrap();
        let ctrl = IgnoreController::new(dir.path()).unwrap();
        assert!(!ctrl.is_allowed(&dir.path().join("debug.log")));
        assert!(ctrl.is_allowed(&dir.path().join("important.log")));
    }

    #[test]
    fn test_reload() {
        let dir = TempDir::new().unwrap();
        let mut ctrl = IgnoreController::new(dir.path()).unwrap();
        assert!(ctrl.is_allowed(&dir.path().join("test.log")));
        std::fs::write(dir.path().join(".mehignore"), "*.log\n").unwrap();
        ctrl.reload().unwrap();
        assert!(!ctrl.is_allowed(&dir.path().join("test.log")));
    }
}
```

## Acceptance Criteria
- [ ] .mehignore loaded from workspace root using `ignore` crate
- [ ] gitignore syntax supported (glob, negation, directory)
- [ ] All file-accessing tools check IgnoreController
- [ ] Paths outside workspace always allowed
- [ ] Missing .mehignore = no restrictions
- [ ] filter_paths returns empty on error (fail-closed)
- [ ] Hot-reload via file watcher
- [ ] `cargo clippy -- -D warnings` passes
- [ ] All tests pass
