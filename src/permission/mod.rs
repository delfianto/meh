//! Permission system — controls which tool calls can execute without user approval.
//!
//! Every side-effecting tool call passes through the permission system
//! before execution. Three tiers control the approval behavior:
//!
//! ```text
//!   ToolCallRequest
//!         │
//!         ▼
//!   PermissionController::check()
//!         │
//!         ├── YOLO mode?  ──► auto-approve all
//!         │
//!         ├── Auto-approve rules match?
//!         │     ├── ReadOnly tools  ──► configurable (default: auto)
//!         │     ├── FileWrite tools ──► configurable (default: ask)
//!         │     ├── Command tools   ──► check command_perms patterns
//!         │     └── MCP tools       ──► configurable (default: ask)
//!         │
//!         ├── In always_allowed set? ──► auto-approve
//!         │
//!         └── Otherwise ──► prompt user via TUI
//! ```
//!
//! - `auto_approve` — per-category rules (read=auto, write=ask, etc.)
//! - `command_perms` — glob-based allow/deny patterns for shell commands,
//!   with operator splitting (`&&`, `||`, `|`, `;`) and subshell recursion
//! - `yolo` — bypasses all checks, approves everything

pub mod auto_approve;
pub mod command_perms;
pub mod yolo;
