//! Rule file loading and pattern utilities.
//!
//! Provides helpers for reading `.mehignore` files and extracting
//! patterns. Supports standard gitignore syntax.

use std::path::Path;

/// Read patterns from a `.mehignore` file, one per line.
///
/// Skips empty lines and comments (lines starting with `#`).
/// Returns an empty vec if the file doesn't exist.
pub fn read_patterns(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path).map_or_else(
        |_| Vec::new(),
        |content| {
            content
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(String::from)
                .collect()
        },
    )
}

/// Check if a pattern looks like a directory pattern (ends with `/`).
pub fn is_directory_pattern(pattern: &str) -> bool {
    pattern.ends_with('/')
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn read_patterns_from_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".mehignore");
        std::fs::write(
            &path,
            "# Comment\n*.log\n\n  node_modules/  \n# Another comment\n.env\n",
        )
        .unwrap();
        let patterns = read_patterns(&path);
        assert_eq!(patterns, vec!["*.log", "node_modules/", ".env"]);
    }

    #[test]
    fn read_patterns_missing_file() {
        let patterns = read_patterns(Path::new("/nonexistent/.mehignore"));
        assert!(patterns.is_empty());
    }

    #[test]
    fn read_patterns_empty_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".mehignore");
        std::fs::write(&path, "").unwrap();
        let patterns = read_patterns(&path);
        assert!(patterns.is_empty());
    }

    #[test]
    fn read_patterns_comments_only() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".mehignore");
        std::fs::write(&path, "# Just comments\n# Nothing else\n").unwrap();
        let patterns = read_patterns(&path);
        assert!(patterns.is_empty());
    }

    #[test]
    fn is_directory_pattern_true() {
        assert!(is_directory_pattern("node_modules/"));
        assert!(is_directory_pattern("build/"));
    }

    #[test]
    fn is_directory_pattern_false() {
        assert!(!is_directory_pattern("*.log"));
        assert!(!is_directory_pattern(".env"));
    }
}
