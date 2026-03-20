# STEP 01 — Project Scaffolding, Cargo.toml, Module Structure

## Objective
Set up the complete Rust project skeleton with all crate dependencies, module declarations, and stub files so that `cargo build` and `cargo test` pass with zero errors and zero warnings from day one.

## Detailed Instructions

### 1.1 Initialize the project
- Run `cargo init --name meh` in the project root
- The binary target is `meh`

### 1.2 Cargo.toml — EXACT dependencies
Write the complete Cargo.toml with:

```toml
[package]
name = "meh"
version = "0.1.0"
edition = "2021"
rust-version = "1.75"
description = "Terminal-based AI coding assistant"
license = "MIT"

[dependencies]
tokio = { version = "1", features = ["full"] }
futures = "0.3"
async-trait = "0.1"
pin-project-lite = "0.2"
bytes = "1"

ratatui = "0.29"
crossterm = "0.28"
tui-textarea = "0.7"

reqwest = { version = "0.12", features = ["json", "stream", "rustls-tls"] }
reqwest-eventsource = "0.6"
tokio-tungstenite = { version = "0.24", features = ["rustls-tls-webpki-roots"] }
eventsource-stream = "0.2"

serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"

clap = { version = "4", features = ["derive"] }

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
tiktoken-rs = "0.6"
similar = "2"

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

### 1.3 Create ALL module stub files

Create every file in the module tree. Each file must have a module doc comment and minimal valid Rust content (either `// Module contents to be implemented in STEP XX` or actual type stubs that other modules reference).

Directory structure to create:
```
src/
├── main.rs
├── app.rs
├── tui/
│   ├── mod.rs
│   ├── app_layout.rs
│   ├── chat_view.rs
│   ├── input.rs
│   ├── thinking_view.rs
│   ├── tool_view.rs
│   ├── status_bar.rs
│   ├── settings_view.rs
│   └── event.rs
├── controller/
│   ├── mod.rs
│   ├── task.rs
│   └── messages.rs
├── provider/
│   ├── mod.rs
│   ├── anthropic.rs
│   ├── openai.rs
│   ├── gemini.rs
│   ├── openrouter.rs
│   └── common.rs
├── tool/
│   ├── mod.rs
│   ├── executor.rs
│   ├── definitions.rs
│   ├── handlers/
│   │   ├── mod.rs
│   │   ├── read_file.rs
│   │   ├── write_file.rs
│   │   ├── apply_patch.rs
│   │   ├── execute_command.rs
│   │   ├── search_files.rs
│   │   ├── list_files.rs
│   │   ├── ask_followup.rs
│   │   ├── attempt_completion.rs
│   │   ├── plan_mode_respond.rs
│   │   └── mcp_tool.rs
│   └── mcp/
│       ├── mod.rs
│       ├── client.rs
│       ├── transport.rs
│       └── types.rs
├── agent/
│   ├── mod.rs
│   ├── task_agent.rs
│   └── sub_agent.rs
├── permission/
│   ├── mod.rs
│   ├── command_perms.rs
│   ├── auto_approve.rs
│   └── yolo.rs
├── state/
│   ├── mod.rs
│   ├── config.rs
│   ├── history.rs
│   ├── secrets.rs
│   └── task_state.rs
├── prompt/
│   ├── mod.rs
│   ├── base.rs
│   ├── tools_section.rs
│   ├── rules.rs
│   └── context.rs
├── streaming/
│   ├── mod.rs
│   ├── tool_parser.rs
│   ├── thinking_parser.rs
│   └── chunk_batcher.rs
└── util/
    ├── mod.rs
    ├── fs.rs
    ├── process.rs
    ├── path.rs
    └── tokens.rs
```

### 1.4 main.rs content
```rust
//! Meh — Terminal-based AI coding assistant

mod app;
mod tui;
mod controller;
mod provider;
mod tool;
mod agent;
mod permission;
mod state;
mod prompt;
mod streaming;
mod util;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "meh", version, about = "Terminal-based AI coding assistant")]
pub struct Cli {
    /// Initial prompt to send
    #[arg(value_name = "PROMPT")]
    pub prompt: Option<String>,

    /// Override provider (anthropic|openai|gemini|openrouter)
    #[arg(short, long)]
    pub provider: Option<String>,

    /// Override model ID
    #[arg(short, long)]
    pub model: Option<String>,

    /// Start in mode (plan|act)
    #[arg(long)]
    pub mode: Option<String>,

    /// Enable YOLO mode (no approval prompts)
    #[arg(long)]
    pub yolo: bool,

    /// Config file path
    #[arg(short, long)]
    pub config: Option<std::path::PathBuf>,

    /// Resume a previous task
    #[arg(long)]
    pub resume: Option<String>,

    /// Verbose logging
    #[arg(short, long)]
    pub verbose: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Initialize tracing
    let filter = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter)),
        )
        .with_target(false)
        .init();

    // Build and run the tokio runtime
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async {
        let app = app::App::new(cli).await?;
        app.run().await
    })
}
```

### 1.5 app.rs stub
```rust
//! Application entry point — owns Controller and TUI, runs the main event loop.

use crate::Cli;

pub struct App {
    _cli: Cli,
}

impl App {
    pub async fn new(cli: Cli) -> anyhow::Result<Self> {
        Ok(Self { _cli: cli })
    }

    pub async fn run(self) -> anyhow::Result<()> {
        tracing::info!("meh starting up");
        Ok(())
    }
}
```

### 1.6 Module mod.rs stubs
Every mod.rs must:
1. Have a module-level doc comment `//! Description`
2. Declare all submodules with `pub mod submodule;`
3. Re-export key types (even if they're placeholder unit structs for now)

Example for `src/provider/mod.rs`:
```rust
//! LLM provider abstraction and implementations.

pub mod anthropic;
pub mod openai;
pub mod gemini;
pub mod openrouter;
pub mod common;
```

Each leaf file (e.g., `anthropic.rs`) should contain:
```rust
//! Anthropic (Claude) provider implementation.
```

And nothing else yet -- just the doc comment so clippy doesn't complain about empty files. Actually, empty files are fine in Rust since they compile. But add the doc comment for consistency.

### 1.7 Create config directory
```
config/
└── default_settings.toml    # Empty TOML file with comments for now
```

### 1.8 Create test directory structure
```
tests/
├── integration/
│   └── mod.rs   (empty — #[cfg(test)] placeholder)
└── unit/
    └── mod.rs   (empty)
```

## Tests for Step 1
```rust
// In src/main.rs or a tests file:
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_cli_no_args() {
        let cli = Cli::try_parse_from(["meh"]);
        assert!(cli.is_ok());
    }

    #[test]
    fn test_cli_with_prompt() {
        let cli = Cli::try_parse_from(["meh", "fix the bug"]).unwrap();
        assert_eq!(cli.prompt.as_deref(), Some("fix the bug"));
    }

    #[test]
    fn test_cli_with_all_flags() {
        let cli = Cli::try_parse_from([
            "meh", "--provider", "anthropic", "--model", "claude-sonnet-4-20250514",
            "--mode", "plan", "--yolo", "--verbose", "do something",
        ]).unwrap();
        assert_eq!(cli.provider.as_deref(), Some("anthropic"));
        assert_eq!(cli.model.as_deref(), Some("claude-sonnet-4-20250514"));
        assert_eq!(cli.mode.as_deref(), Some("plan"));
        assert!(cli.yolo);
        assert!(cli.verbose);
        assert_eq!(cli.prompt.as_deref(), Some("do something"));
    }

    #[test]
    fn test_cli_yolo_flag() {
        let cli = Cli::try_parse_from(["meh", "--yolo"]).unwrap();
        assert!(cli.yolo);
    }

    #[test]
    fn test_cli_resume() {
        let cli = Cli::try_parse_from(["meh", "--resume", "abc-123"]).unwrap();
        assert_eq!(cli.resume.as_deref(), Some("abc-123"));
    }
}
```

## Acceptance Criteria
- [x] `cargo build` succeeds with zero errors
- [x] `cargo build` produces zero warnings
- [x] `cargo clippy -- -D warnings` passes
- [x] `cargo test` passes (7 CLI parsing tests)
- [x] `cargo fmt --check` passes
- [x] All 71 source files exist with proper module declarations
- [x] Binary runs and exits cleanly: `cargo run -- --help`

**Completed**: PR #2
