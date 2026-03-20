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
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

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
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "bmp"
            | "ico"
            | "webp"
            | "svg"
            | "mp3"
            | "mp4"
            | "wav"
            | "avi"
            | "mkv"
            | "mov"
            | "zip"
            | "tar"
            | "gz"
            | "bz2"
            | "xz"
            | "7z"
            | "rar"
            | "pdf"
            | "doc"
            | "docx"
            | "xls"
            | "xlsx"
            | "ppt"
            | "pptx"
            | "exe"
            | "dll"
            | "so"
            | "dylib"
            | "o"
            | "a"
            | "wasm"
            | "ttf"
            | "woff"
            | "woff2"
            | "eot"
            | "pyc"
            | "pyo"
            | "class"
            | "db"
            | "sqlite"
            | "sqlite3"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_relative() {
        let resolved = resolve_path("/home/user/project", "src/main.rs");
        assert_eq!(resolved, PathBuf::from("/home/user/project/src/main.rs"));
    }

    #[test]
    fn test_resolve_absolute() {
        let resolved = resolve_path("/home/user/project", "/etc/config");
        assert_eq!(resolved, PathBuf::from("/etc/config"));
    }

    #[test]
    fn test_resolve_dot() {
        let resolved = resolve_path("/home/user", ".");
        assert_eq!(resolved, PathBuf::from("/home/user/."));
    }

    #[test]
    fn test_resolve_dotdot() {
        let resolved = resolve_path("/home/user/project", "../other");
        assert_eq!(resolved, PathBuf::from("/home/user/project/../other"));
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
