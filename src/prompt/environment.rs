//! Environment detection — OS, shell, workspace type, language.
//!
//! Collects runtime environment information for injection into the system
//! prompt. Helps the LLM generate platform-appropriate commands and
//! understand the project structure.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;

/// Collected environment information.
#[derive(Debug, Clone)]
pub struct EnvironmentInfo {
    /// OS name and version.
    pub os: String,
    /// CPU architecture.
    pub arch: String,
    /// Shell path.
    pub shell: String,
    /// Home directory.
    pub home_dir: String,
    /// Current working directory.
    pub cwd: String,
    /// Detected project type from config files.
    pub workspace_type: Option<WorkspaceType>,
    /// Languages detected from file extensions.
    pub detected_languages: Vec<String>,
}

/// Type of project workspace, detected from config files.
#[derive(Debug, Clone)]
pub enum WorkspaceType {
    /// Rust project (`Cargo.toml`).
    Rust,
    /// Node.js project (`package.json`).
    Node,
    /// Python project (`pyproject.toml`, `setup.py`, `requirements.txt`).
    Python,
    /// Go project (`go.mod`).
    Go,
    /// Java project (`pom.xml`, `build.gradle`).
    Java,
    /// .NET project (`*.csproj`, `*.sln`).
    DotNet,
    /// Multiple project types detected.
    Mixed(Vec<String>),
}

impl EnvironmentInfo {
    /// Detect environment from the given working directory.
    pub fn detect(cwd: &str) -> Self {
        let path = Path::new(cwd);
        Self {
            os: detect_os(),
            arch: std::env::consts::ARCH.to_string(),
            shell: detect_shell(),
            home_dir: dirs::home_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            cwd: cwd.to_string(),
            workspace_type: detect_workspace_type(path),
            detected_languages: detect_languages(path),
        }
    }

    /// Format as a section for inclusion in the system prompt.
    pub fn to_prompt_section(&self) -> String {
        let mut s = format!(
            "# Environment\n- OS: {} ({})\n- Shell: {}\n- Working Directory: {}\n",
            self.os, self.arch, self.shell, self.cwd
        );
        if let Some(ref wt) = self.workspace_type {
            let _ = writeln!(s, "- Project Type: {}", workspace_type_name(wt));
        }
        if !self.detected_languages.is_empty() {
            let _ = writeln!(s, "- Languages: {}", self.detected_languages.join(", "));
        }
        s
    }
}

/// Detect OS name with version.
fn detect_os() -> String {
    #[cfg(target_os = "macos")]
    {
        let version = get_macos_version().unwrap_or_else(|| "unknown".to_string());
        format!("macOS {version}")
    }
    #[cfg(target_os = "linux")]
    {
        let version = get_linux_version().unwrap_or_else(|| "unknown".to_string());
        format!("Linux {version}")
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        std::env::consts::OS.to_string()
    }
}

/// Detect the user's shell from the `SHELL` environment variable.
fn detect_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}

/// Detect project workspace type from well-known config files.
fn detect_workspace_type(cwd: &Path) -> Option<WorkspaceType> {
    let mut found: Vec<WorkspaceType> = Vec::new();

    if cwd.join("Cargo.toml").exists() {
        found.push(WorkspaceType::Rust);
    }
    if cwd.join("package.json").exists() {
        found.push(WorkspaceType::Node);
    }
    if cwd.join("go.mod").exists() {
        found.push(WorkspaceType::Go);
    }
    if cwd.join("pyproject.toml").exists()
        || cwd.join("setup.py").exists()
        || cwd.join("requirements.txt").exists()
    {
        found.push(WorkspaceType::Python);
    }
    if cwd.join("pom.xml").exists() || cwd.join("build.gradle").exists() {
        found.push(WorkspaceType::Java);
    }

    match found.len() {
        0 => None,
        1 => Some(found.remove(0)),
        _ => Some(WorkspaceType::Mixed(
            found.iter().map(workspace_type_name).collect(),
        )),
    }
}

/// Detect programming languages from top-level file extensions.
fn detect_languages(cwd: &Path) -> Vec<String> {
    let mut langs = HashSet::new();
    let Ok(entries) = std::fs::read_dir(cwd) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
            let lang = match ext {
                "rs" => Some("Rust"),
                "ts" | "tsx" => Some("TypeScript"),
                "js" | "jsx" | "mjs" => Some("JavaScript"),
                "py" => Some("Python"),
                "go" => Some("Go"),
                "java" => Some("Java"),
                "rb" => Some("Ruby"),
                "cpp" | "cc" | "cxx" => Some("C++"),
                "c" | "h" => Some("C"),
                "cs" => Some("C#"),
                "swift" => Some("Swift"),
                "kt" | "kts" => Some("Kotlin"),
                _ => None,
            };
            if let Some(l) = lang {
                langs.insert(l);
            }
        }
    }
    let mut result: Vec<String> = langs.into_iter().map(String::from).collect();
    result.sort();
    result
}

/// Human-readable name for a workspace type.
fn workspace_type_name(wt: &WorkspaceType) -> String {
    match wt {
        WorkspaceType::Rust => "Rust (Cargo)".to_string(),
        WorkspaceType::Node => "Node.js (npm/yarn)".to_string(),
        WorkspaceType::Python => "Python".to_string(),
        WorkspaceType::Go => "Go".to_string(),
        WorkspaceType::Java => "Java".to_string(),
        WorkspaceType::DotNet => ".NET".to_string(),
        WorkspaceType::Mixed(types) => format!("Mixed ({})", types.join(", ")),
    }
}

#[cfg(target_os = "macos")]
fn get_macos_version() -> Option<String> {
    std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
}

#[cfg(target_os = "linux")]
fn get_linux_version() -> Option<String> {
    std::fs::read_to_string("/proc/version")
        .ok()
        .and_then(|v| v.split_whitespace().nth(2).map(String::from))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn detect_os_non_empty() {
        let os = detect_os();
        assert!(!os.is_empty());
    }

    #[test]
    fn detect_shell_non_empty() {
        let shell = detect_shell();
        assert!(!shell.is_empty());
    }

    #[test]
    fn detect_workspace_type_rust() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"test\"\n",
        )
        .unwrap();
        let wt = detect_workspace_type(dir.path());
        assert!(matches!(wt, Some(WorkspaceType::Rust)));
    }

    #[test]
    fn detect_workspace_type_node() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        let wt = detect_workspace_type(dir.path());
        assert!(matches!(wt, Some(WorkspaceType::Node)));
    }

    #[test]
    fn detect_workspace_type_python() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("requirements.txt"), "flask\n").unwrap();
        let wt = detect_workspace_type(dir.path());
        assert!(matches!(wt, Some(WorkspaceType::Python)));
    }

    #[test]
    fn detect_workspace_type_go() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("go.mod"), "module example.com/app\n").unwrap();
        let wt = detect_workspace_type(dir.path());
        assert!(matches!(wt, Some(WorkspaceType::Go)));
    }

    #[test]
    fn detect_workspace_type_java() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("pom.xml"), "<project/>").unwrap();
        let wt = detect_workspace_type(dir.path());
        assert!(matches!(wt, Some(WorkspaceType::Java)));
    }

    #[test]
    fn detect_workspace_type_mixed() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        let wt = detect_workspace_type(dir.path());
        assert!(matches!(wt, Some(WorkspaceType::Mixed(_))));
    }

    #[test]
    fn detect_workspace_type_none() {
        let dir = TempDir::new().unwrap();
        let wt = detect_workspace_type(dir.path());
        assert!(wt.is_none());
    }

    #[test]
    fn detect_languages_from_extensions() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("main.rs"), "").unwrap();
        std::fs::write(dir.path().join("lib.rs"), "").unwrap();
        std::fs::write(dir.path().join("script.py"), "").unwrap();
        let langs = detect_languages(dir.path());
        assert!(langs.contains(&"Rust".to_string()));
        assert!(langs.contains(&"Python".to_string()));
    }

    #[test]
    fn detect_languages_deduplicates() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.rs"), "").unwrap();
        std::fs::write(dir.path().join("b.rs"), "").unwrap();
        std::fs::write(dir.path().join("c.rs"), "").unwrap();
        let langs = detect_languages(dir.path());
        assert_eq!(langs.iter().filter(|l| l.as_str() == "Rust").count(), 1);
    }

    #[test]
    fn detect_languages_sorted() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("x.rs"), "").unwrap();
        std::fs::write(dir.path().join("y.py"), "").unwrap();
        std::fs::write(dir.path().join("z.go"), "").unwrap();
        let langs = detect_languages(dir.path());
        let sorted: Vec<String> = {
            let mut v = langs.clone();
            v.sort();
            v
        };
        assert_eq!(langs, sorted);
    }

    #[test]
    fn detect_languages_empty_dir() {
        let dir = TempDir::new().unwrap();
        let langs = detect_languages(dir.path());
        assert!(langs.is_empty());
    }

    #[test]
    fn to_prompt_section_format() {
        let info = EnvironmentInfo {
            os: "macOS 15.2".to_string(),
            arch: "aarch64".to_string(),
            shell: "/bin/zsh".to_string(),
            home_dir: "/Users/test".to_string(),
            cwd: "/tmp/project".to_string(),
            workspace_type: Some(WorkspaceType::Rust),
            detected_languages: vec!["Rust".to_string()],
        };
        let section = info.to_prompt_section();
        assert!(section.contains("macOS 15.2"));
        assert!(section.contains("aarch64"));
        assert!(section.contains("/bin/zsh"));
        assert!(section.contains("/tmp/project"));
        assert!(section.contains("Rust (Cargo)"));
        assert!(section.contains("Languages: Rust"));
    }

    #[test]
    fn to_prompt_section_no_workspace() {
        let info = EnvironmentInfo {
            os: "Linux".to_string(),
            arch: "x86_64".to_string(),
            shell: "/bin/bash".to_string(),
            home_dir: "/home/user".to_string(),
            cwd: "/tmp".to_string(),
            workspace_type: None,
            detected_languages: vec![],
        };
        let section = info.to_prompt_section();
        assert!(!section.contains("Project Type"));
        assert!(!section.contains("Languages"));
    }

    #[test]
    fn workspace_type_name_all_variants() {
        assert_eq!(workspace_type_name(&WorkspaceType::Rust), "Rust (Cargo)");
        assert_eq!(
            workspace_type_name(&WorkspaceType::Node),
            "Node.js (npm/yarn)"
        );
        assert_eq!(workspace_type_name(&WorkspaceType::Python), "Python");
        assert_eq!(workspace_type_name(&WorkspaceType::Go), "Go");
        assert_eq!(workspace_type_name(&WorkspaceType::Java), "Java");
        assert_eq!(workspace_type_name(&WorkspaceType::DotNet), ".NET");
        assert!(
            workspace_type_name(&WorkspaceType::Mixed(vec!["Rust".into(), "Node".into()]))
                .contains("Mixed")
        );
    }

    #[test]
    fn environment_info_detect_works() {
        let dir = TempDir::new().unwrap();
        let info = EnvironmentInfo::detect(&dir.path().to_string_lossy());
        assert!(!info.os.is_empty());
        assert!(!info.arch.is_empty());
        assert!(!info.shell.is_empty());
    }
}
