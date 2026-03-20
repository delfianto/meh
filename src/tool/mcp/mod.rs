//! MCP (Model Context Protocol) client — manages connections to external tool servers.
//!
//! MCP servers expose additional tools that the LLM can call, extending
//! the built-in tool set at runtime. The `McpHub` manages the lifecycle
//! of server connections, and the `client` module handles the JSON-RPC
//! protocol over the configured transport.
//!
//! ```text
//!   config.toml
//!     mcp_servers = [...]
//!         │
//!         ▼
//!   McpHub::connect_all()
//!         │
//!         ├── Server A (stdio)  ──► spawn child process
//!         │     └── Client ◄──► JSON-RPC over stdin/stdout
//!         │
//!         └── Server B (SSE)    ──► HTTP connection
//!               └── Client ◄──► JSON-RPC over SSE
//!         │
//!         ▼
//!   McpHub::list_tools() ──► merged into ToolRegistry
//!   McpHub::call_tool()  ──► proxy through mcp_tool handler
//! ```
//!
//! Supported transports:
//! - **stdio** — spawns the server as a child process, communicates via stdin/stdout
//! - **SSE** — connects to an HTTP endpoint with Server-Sent Events for streaming

pub mod client;
pub mod transport;
pub mod types;
