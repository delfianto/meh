# Meh — Dependencies & Cargo.toml Reference

**Rust edition**: `2024` (stable since Rust 1.85, enables async closures, improved RPIT)
**Minimum Rust version**: `1.85.0`

```toml
[package]
name = "meh"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"

[dependencies]
# Async runtime
tokio = { version = "1.50", features = ["full"] }
tokio-util = { version = "0.7", features = ["rt"] }
futures = "0.3"
async-stream = "0.3"

# TUI
ratatui = "0.30"
crossterm = "0.29"
tui-textarea = "0.7"

# HTTP & streaming
reqwest = { version = "0.13", features = ["json", "stream", "rustls-tls"] }
reqwest-eventsource = "0.6"
tokio-tungstenite = { version = "0.29", features = ["rustls-tls-webpki-roots"] }
eventsource-stream = "0.2"

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"

# MCP protocol — use the OFFICIAL Rust SDK
rmcp = { version = "1.2", features = ["client", "transport-child-process", "transport-sse", "transport-streamable-http"] }

# CLI
clap = { version = "4.6", features = ["derive"] }

# Utilities
anyhow = "1"
thiserror = "2"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
uuid = { version = "1", features = ["v4"] }
chrono = { version = "0.4", features = ["serde"] }
glob = "0.3"
regex = "1"
dirs = "6"
keyring = "3"
tiktoken-rs = "0.9"
similar = "2"
bytes = "1"
pin-project-lite = "0.2"
async-trait = "0.1"
notify = { version = "7", features = ["macos_kqueue"] }
ignore = "0.4"
picomatch = "0.1"

[dev-dependencies]
tokio-test = "0.4"
tempfile = "3"
mockall = "0.13"
assert_cmd = "2"
predicates = "3"

[profile.release]
lto = true
codegen-units = 1
strip = true

[lints.rust]
unsafe_code = "forbid"

[lints.clippy]
all = "warn"
pedantic = "warn"
nursery = "warn"
unwrap_used = "warn"
expect_used = "warn"
```

## Dependency Decisions

| Need | Crate | Rationale |
|------|-------|-----------|
| MCP protocol | `rmcp` (official SDK) | Official Rust MCP SDK from modelcontextprotocol org. Replaces hand-rolled JSON-RPC client (STEP 21-22 simplified). |
| Path ignore | `ignore` | Same library pattern as .gitignore. Powers `.mehignore` support. |
| File watching | `notify` | Config hot-reload (STEP 29). |
| Glob matching | `picomatch` | For `.mehrules` conditional path matching (same as Cline's picomatch). |
| Token counting | `tiktoken-rs` 0.9 | Latest with full model coverage. |

**Not using**: `async-openai`, `genai`, `rig`, or `llm` crates. Rationale: we need fine-grained control over streaming, tool call parsing, and provider-specific quirks (thinking blocks, signatures, cache control). The Provider trait + per-provider implementation gives us this control with ~200 lines per provider.
