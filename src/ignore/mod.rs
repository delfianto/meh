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
//!         ├── check .mehignore in parent dirs
//!         └── check built-in rules (.git/, node_modules/, etc.)
//!         │
//!         ▼
//!     allowed / denied
//! ```
//!
//! Rule files use gitignore syntax with optional YAML frontmatter for
//! metadata. Rules are loaded once at startup and can be hot-reloaded
//! when the config watcher detects changes.

pub mod rules;
