//! Path protection via `.mehignore` — prevents tools from accessing protected files.
//!
//! Works like `.gitignore` but for the tool system. Before any file read,
//! write, or search operation, the path is checked against the ignore rules.
//! Protected paths are rejected before the tool handler ever sees them.
//!
//! ```text
//!   Tool request (path)
//!         │
//!         ▼
//!   IgnoreController::is_allowed(path)
//!         │
//!         ├── check .mehignore in project root
//!         ├── check default rules (.env, *.pem, etc.)
//!         └── fallback: allow if no rules match
//!         │
//!         ▼
//!     allowed / denied
//! ```
//!
//! Security is fail-closed: if ignore rules cannot be parsed, all paths
//! within the workspace are denied.

pub mod rules;

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::{Path, PathBuf};

/// Default ignore patterns applied when no `.mehignore` exists.
const DEFAULT_PATTERNS: &[&str] = &[
    ".env",
    ".env.*",
    "*.pem",
    "*.key",
    "credentials.json",
    "secrets.yaml",
    "secrets.yml",
    "*.p12",
    "*.pfx",
];

/// Controls which paths tools are allowed to access.
pub struct IgnoreController {
    gitignore: Option<Gitignore>,
    workspace_root: PathBuf,
}

impl IgnoreController {
    /// Load `.mehignore` from workspace root. Missing file = default restrictions only.
    pub fn new(workspace_root: &Path) -> Self {
        let ignore_path = workspace_root.join(".mehignore");
        let gitignore = if ignore_path.exists() {
            let mut builder = GitignoreBuilder::new(workspace_root);
            builder.add(&ignore_path);
            match builder.build() {
                Ok(gi) => Some(gi),
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to parse .mehignore, using defaults");
                    Some(build_default_ignores(workspace_root))
                }
            }
        } else {
            Some(build_default_ignores(workspace_root))
        };
        Self {
            gitignore,
            workspace_root: workspace_root.to_path_buf(),
        }
    }

    /// Create a controller with no restrictions (for testing).
    pub fn permissive(workspace_root: &Path) -> Self {
        Self {
            gitignore: None,
            workspace_root: workspace_root.to_path_buf(),
        }
    }

    /// Check if a path is accessible (not ignored).
    pub fn is_allowed(&self, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(&self.workspace_root) else {
            return true;
        };
        self.gitignore
            .as_ref()
            .is_none_or(|gi| !gi.matched(relative, path.is_dir()).is_ignore())
    }

    /// Filter a list of paths, returning only accessible ones.
    pub fn filter_paths(&self, paths: &[PathBuf]) -> Vec<PathBuf> {
        paths
            .iter()
            .filter(|p| self.is_allowed(p))
            .cloned()
            .collect()
    }

    /// Reload `.mehignore` (called on file change).
    pub fn reload(&mut self) {
        *self = Self::new(&self.workspace_root);
    }

    /// Returns the workspace root path.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }
}

/// Build a `Gitignore` from the default patterns.
fn build_default_ignores(workspace_root: &Path) -> Gitignore {
    let mut builder = GitignoreBuilder::new(workspace_root);
    for pattern in DEFAULT_PATTERNS {
        let _ = builder.add_line(None, pattern);
    }
    builder.build().unwrap_or_else(|_| {
        GitignoreBuilder::new(workspace_root)
            .build()
            .unwrap_or_else(|e| panic!("Failed to build empty gitignore: {e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn no_ignore_file_uses_defaults() {
        let dir = TempDir::new().unwrap();
        let ctrl = IgnoreController::new(dir.path());
        assert!(ctrl.is_allowed(&dir.path().join("src/main.rs")));
        assert!(!ctrl.is_allowed(&dir.path().join(".env")));
        assert!(!ctrl.is_allowed(&dir.path().join("server.pem")));
    }

    #[test]
    fn custom_ignore_pattern() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".mehignore"), "*.secret\nnode_modules/\n").unwrap();
        let ctrl = IgnoreController::new(dir.path());
        assert!(!ctrl.is_allowed(&dir.path().join("password.secret")));
        assert!(ctrl.is_allowed(&dir.path().join("src/main.rs")));
    }

    #[test]
    fn outside_workspace_always_allowed() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".mehignore"), "*").unwrap();
        let ctrl = IgnoreController::new(dir.path());
        assert!(ctrl.is_allowed(Path::new("/etc/hosts")));
    }

    #[test]
    fn filter_paths_removes_ignored() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".mehignore"), "*.log\n").unwrap();
        let ctrl = IgnoreController::new(dir.path());
        let paths = vec![
            dir.path().join("main.rs"),
            dir.path().join("debug.log"),
            dir.path().join("src/lib.rs"),
        ];
        let filtered = ctrl.filter_paths(&paths);
        assert_eq!(filtered.len(), 2);
        assert!(
            filtered
                .iter()
                .all(|p| !p.to_string_lossy().contains(".log"))
        );
    }

    #[test]
    fn negation_pattern() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".mehignore"), "*.log\n!important.log\n").unwrap();
        let ctrl = IgnoreController::new(dir.path());
        assert!(!ctrl.is_allowed(&dir.path().join("debug.log")));
        assert!(ctrl.is_allowed(&dir.path().join("important.log")));
    }

    #[test]
    fn reload_picks_up_changes() {
        let dir = TempDir::new().unwrap();
        let mut ctrl = IgnoreController::new(dir.path());
        assert!(ctrl.is_allowed(&dir.path().join("test.log")));

        std::fs::write(dir.path().join(".mehignore"), "*.log\n").unwrap();
        ctrl.reload();
        assert!(!ctrl.is_allowed(&dir.path().join("test.log")));
    }

    #[test]
    fn permissive_allows_all() {
        let dir = TempDir::new().unwrap();
        let ctrl = IgnoreController::permissive(dir.path());
        assert!(ctrl.is_allowed(&dir.path().join(".env")));
        assert!(ctrl.is_allowed(&dir.path().join("server.pem")));
    }

    #[test]
    fn default_patterns_block_secrets() {
        let dir = TempDir::new().unwrap();
        let ctrl = IgnoreController::new(dir.path());
        assert!(!ctrl.is_allowed(&dir.path().join(".env")));
        assert!(!ctrl.is_allowed(&dir.path().join(".env.production")));
        assert!(!ctrl.is_allowed(&dir.path().join("server.key")));
        assert!(!ctrl.is_allowed(&dir.path().join("credentials.json")));
        assert!(!ctrl.is_allowed(&dir.path().join("secrets.yaml")));
    }

    #[test]
    fn default_patterns_allow_normal_files() {
        let dir = TempDir::new().unwrap();
        let ctrl = IgnoreController::new(dir.path());
        assert!(ctrl.is_allowed(&dir.path().join("src/main.rs")));
        assert!(ctrl.is_allowed(&dir.path().join("Cargo.toml")));
        assert!(ctrl.is_allowed(&dir.path().join("README.md")));
    }

    #[test]
    fn workspace_root_accessor() {
        let dir = TempDir::new().unwrap();
        let ctrl = IgnoreController::new(dir.path());
        assert_eq!(ctrl.workspace_root(), dir.path());
    }

    #[test]
    fn filter_paths_empty_input() {
        let dir = TempDir::new().unwrap();
        let ctrl = IgnoreController::new(dir.path());
        let filtered = ctrl.filter_paths(&[]);
        assert!(filtered.is_empty());
    }

    #[test]
    fn directory_pattern() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".mehignore"), "build/**\ntarget/**\n").unwrap();
        let ctrl = IgnoreController::new(dir.path());
        assert!(!ctrl.is_allowed(&dir.path().join("build/output.js")));
        assert!(!ctrl.is_allowed(&dir.path().join("target/debug/binary")));
        assert!(ctrl.is_allowed(&dir.path().join("src/build.rs")));
    }
}
