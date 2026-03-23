//! Tool handler implementations — one module per tool.
//!
//! Each handler implements the `ToolHandler` trait, declaring its name,
//! category, and execution logic. Handlers receive parsed JSON arguments
//! and a `ToolContext` providing access to the filesystem, permissions,
//! and ignore rules.
//!
//! ```text
//!   ┌──────────────────────────────────────────────────────┐
//!   │                    handlers/                          │
//!   │                                                      │
//!   │  ReadOnly:        read_file, list_files, search_files│
//!   │  FileWrite:       write_file, apply_patch            │
//!   │  Command:         execute_command                    │
//!   │  Informational:   ask_followup, attempt_completion,  │
//!   │                   plan_mode_respond                  │
//!   │  MCP:             mcp_tool (proxy to MCP servers)    │
//!   └──────────────────────────────────────────────────────┘
//! ```

pub mod apply_patch;
pub mod ask_followup;
pub mod attempt_completion;
pub mod delegate_task;
pub mod execute_command;
pub mod list_files;
pub mod mcp_tool;
pub mod plan_mode_respond;
pub mod read_file;
pub mod search_files;
pub mod write_file;
