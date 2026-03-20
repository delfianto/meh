//! Tool system — handler trait, registry, executor, and MCP integration.
//!
//! Tools are the actions the LLM can take on the user's behalf.
//! Each tool implements the `ToolHandler` trait and is registered
//! in a `ToolRegistry`. The `executor` module routes incoming tool
//! calls to the correct handler after permission checks.
//!
//! ```text
//!   LLM emits tool_use
//!         │
//!         ▼
//!   StreamProcessor parses ToolCallComplete
//!         │
//!         ▼
//!   Agent sends ToolCallRequest ──► Controller
//!         │
//!         ▼
//!   PermissionController::check()
//!         │
//!         ├── auto-approved ──► Executor::run()
//!         └── needs approval ──► TUI prompt ──► ApprovalResponse
//!                                                    │
//!                                              ┌─────┴─────┐
//!                                           approved     denied
//!                                              │            │
//!                                        Executor::run()  error result
//!         │
//!         ▼
//!   ToolRegistry::get(name) ──► dyn ToolHandler::execute()
//!         │
//!         ▼
//!   ToolCallResult ──► Agent ──► next API call
//! ```
//!
//! Tool categories determine default permission behavior:
//! - `ReadOnly` — `read_file`, `list_files`, `search_files`
//! - `FileWrite` — `write_file`, `apply_patch`
//! - `Command` — `execute_command`
//! - `Mcp` — dynamically registered MCP server tools
//! - `Informational` — `ask_followup`, `attempt_completion`, `plan_mode_respond`

pub mod definitions;
pub mod executor;
pub mod handlers;
pub mod mcp;
