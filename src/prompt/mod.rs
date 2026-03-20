//! System prompt construction — modular assembly of the full system prompt.
//!
//! The system prompt is not a static string. It is assembled dynamically
//! from multiple sources depending on the current mode, available tools,
//! user rules, and environment context.
//!
//! ```text
//!   build_system_prompt()
//!         │
//!         ├── base.rs          ──► core identity and behavioral instructions
//!         ├── tools_section.rs ──► tool definitions (filtered by mode)
//!         ├── rules.rs         ──► user rules from .meh/rules and .mehrules
//!         ├── environment.rs   ──► OS, shell, cwd, language detection
//!         └── context.rs       ──► workspace structure, file tree, git status
//!         │
//!         ▼
//!   String (complete system prompt)
//! ```
//!
//! In Plan mode, only read-only tool definitions are injected. In Act mode,
//! the full tool set is included. MCP server tools are appended dynamically
//! based on which servers are currently connected.

pub mod base;
pub mod context;
pub mod environment;
pub mod rules;
pub mod tools_section;

/// Builds the system prompt for the task agent.
///
/// This is a minimal implementation; STEP 37 will assemble
/// the full modular prompt from all submodules.
pub fn build_system_prompt(cwd: &str) -> String {
    format!(
        "You are a helpful AI coding assistant running in a terminal.\n\
         The user's working directory is: {cwd}\n\
         Respond concisely and helpfully."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_system_prompt_contains_cwd() {
        let prompt = build_system_prompt("/home/user/project");
        assert!(prompt.contains("/home/user/project"));
    }

    #[test]
    fn build_system_prompt_not_empty() {
        let prompt = build_system_prompt("/tmp");
        assert!(!prompt.is_empty());
    }
}
