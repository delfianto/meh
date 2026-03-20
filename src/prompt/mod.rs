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
