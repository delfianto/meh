# STEP 33 — Environment Detection

## Objective
Detect and collect environment information (OS, shell, workspace type, language) for system prompt context. This helps the LLM generate appropriate commands and understand the project.

## Prerequisites
- STEP 01 (project structure)

## Detailed Instructions

### 33.1 Environment info (`src/prompt/environment.rs`)

```rust
//! Environment detection — OS, shell, workspace type, language.

use std::path::Path;

#[derive(Debug, Clone)]
pub struct EnvironmentInfo {
    pub os: String,              // "macOS 15.2", "Linux 6.5", "Windows 11"
    pub arch: String,            // "aarch64", "x86_64"
    pub shell: String,           // "/bin/zsh", "/bin/bash"
    pub home_dir: String,
    pub cwd: String,
    pub workspace_type: Option<WorkspaceType>,
    pub detected_languages: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum WorkspaceType {
    Rust,           // Cargo.toml
    Node,           // package.json
    Python,         // pyproject.toml, setup.py, requirements.txt
    Go,             // go.mod
    Java,           // pom.xml, build.gradle
    DotNet,         // *.csproj, *.sln
    Mixed(Vec<String>),
}

impl EnvironmentInfo {
    pub fn detect(cwd: &str) -> Self {
        Self {
            os: detect_os(),
            arch: std::env::consts::ARCH.to_string(),
            shell: detect_shell(),
            home_dir: dirs::home_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
            cwd: cwd.to_string(),
            workspace_type: detect_workspace_type(Path::new(cwd)),
            detected_languages: detect_languages(Path::new(cwd)),
        }
    }

    /// Format for system prompt injection.
    pub fn to_prompt_section(&self) -> String {
        let mut s = format!(
            "# Environment\n- OS: {} ({})\n- Shell: {}\n- Working Directory: {}\n",
            self.os, self.arch, self.shell, self.cwd
        );
        if let Some(ref wt) = self.workspace_type {
            s.push_str(&format!("- Project Type: {}\n", workspace_type_name(wt)));
        }
        if !self.detected_languages.is_empty() {
            s.push_str(&format!("- Languages: {}\n", self.detected_languages.join(", ")));
        }
        s
    }
}

fn detect_os() -> String {
    #[cfg(target_os = "macos")]
    { format!("macOS {}", get_macos_version().unwrap_or_else(|| "unknown".to_string())) }
    #[cfg(target_os = "linux")]
    { format!("Linux {}", get_linux_version().unwrap_or_else(|| "unknown".to_string())) }
    #[cfg(target_os = "windows")]
    { "Windows".to_string() }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    { std::env::consts::OS.to_string() }
}

fn detect_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}

fn detect_workspace_type(cwd: &Path) -> Option<WorkspaceType> {
    let checks = [
        ("Cargo.toml", WorkspaceType::Rust),
        ("package.json", WorkspaceType::Node),
        ("go.mod", WorkspaceType::Go),
    ];
    let mut found = Vec::new();
    for (file, wtype) in &checks {
        if cwd.join(file).exists() {
            found.push(wtype.clone());
        }
    }
    // Also check pyproject.toml, setup.py, requirements.txt for Python
    if cwd.join("pyproject.toml").exists() || cwd.join("setup.py").exists() || cwd.join("requirements.txt").exists() {
        found.push(WorkspaceType::Python);
    }
    match found.len() {
        0 => None,
        1 => Some(found.remove(0)),
        _ => Some(WorkspaceType::Mixed(found.iter().map(workspace_type_name).collect())),
    }
}

fn detect_languages(cwd: &Path) -> Vec<String> {
    // Scan top-level files for common extensions
    let mut langs = std::collections::HashSet::new();
    if let Ok(entries) = std::fs::read_dir(cwd) {
        for entry in entries.flatten() {
            if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
                match ext {
                    "rs" => { langs.insert("Rust"); }
                    "ts" | "tsx" => { langs.insert("TypeScript"); }
                    "js" | "jsx" => { langs.insert("JavaScript"); }
                    "py" => { langs.insert("Python"); }
                    "go" => { langs.insert("Go"); }
                    "java" => { langs.insert("Java"); }
                    "rb" => { langs.insert("Ruby"); }
                    "cpp" | "cc" | "cxx" => { langs.insert("C++"); }
                    "c" | "h" => { langs.insert("C"); }
                    _ => {}
                }
            }
        }
    }
    langs.into_iter().map(String::from).collect()
}

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
```

## Tests

```rust
#[cfg(test)]
mod environment_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_detect_os_non_empty() {
        let os = detect_os();
        assert!(!os.is_empty());
    }

    #[test]
    fn test_detect_shell() {
        let shell = detect_shell();
        assert!(!shell.is_empty());
    }

    #[test]
    fn test_detect_workspace_type_rust() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"test\"\n").unwrap();
        let wt = detect_workspace_type(dir.path());
        assert!(matches!(wt, Some(WorkspaceType::Rust)));
    }

    #[test]
    fn test_detect_workspace_type_node() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        let wt = detect_workspace_type(dir.path());
        assert!(matches!(wt, Some(WorkspaceType::Node)));
    }

    #[test]
    fn test_detect_workspace_type_mixed() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        let wt = detect_workspace_type(dir.path());
        assert!(matches!(wt, Some(WorkspaceType::Mixed(_))));
    }

    #[test]
    fn test_detect_workspace_type_none() {
        let dir = TempDir::new().unwrap();
        let wt = detect_workspace_type(dir.path());
        assert!(wt.is_none());
    }

    #[test]
    fn test_detect_languages() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("main.rs"), "").unwrap();
        std::fs::write(dir.path().join("lib.rs"), "").unwrap();
        std::fs::write(dir.path().join("script.py"), "").unwrap();
        let langs = detect_languages(dir.path());
        assert!(langs.contains(&"Rust".to_string()));
        assert!(langs.contains(&"Python".to_string()));
    }

    #[test]
    fn test_to_prompt_section() {
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
        assert!(section.contains("Rust (Cargo)"));
    }

    #[test]
    fn test_workspace_type_name() {
        assert_eq!(workspace_type_name(&WorkspaceType::Rust), "Rust (Cargo)");
        assert_eq!(workspace_type_name(&WorkspaceType::Go), "Go");
        assert!(workspace_type_name(&WorkspaceType::Mixed(vec!["Rust".into(), "Node".into()])).contains("Mixed"));
    }
}
```

## Acceptance Criteria
- [x] OS detected with version string
- [x] Shell detected from $SHELL
- [x] Workspace type detected from config files
- [x] Languages detected from file extensions
- [x] Environment formatted for system prompt
- [x] Mixed workspace types handled correctly
- [x] Platform-specific version detection (macOS sw_vers, Linux /proc/version)
- [x] `cargo clippy -- -D warnings` passes
- [x] All tests pass
