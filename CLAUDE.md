# Meh — A Rust CLI AI Coding Assistant

A terminal-based AI coding assistant written in Rust, inspired by Cline. Interactive TUI (Ratatui) for conversing with LLMs, executing tool calls, and performing autonomous coding tasks — with async concurrency (tokio + MPSC) and streaming support.

---

## Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                              meh (binary)                            │
├──────────────────────────────────────────────────────────────────────┤
│  ┌─────────────┐   ┌──────────────┐   ┌──────────────────────────┐ │
│  │  TUI Layer  │   │  Controller  │   │     State Manager        │ │
│  │  (Ratatui)  │◄──┤  (Orchestr.) ├──►│  (Config, History,       │ │
│  │             │   │              │   │   Secrets, Task State)    │ │
│  └──────┬──────┘   └──────┬───────┘   └──────────────────────────┘ │
│         │    ┌────────────┼────────────────┐                        │
│         ▼    ▼            ▼                ▼                        │
│  ┌───────────────┐ ┌───────────┐  ┌──────────────┐                │
│  │  Agent/Task   │ │ Provider  │  │ Tool System  │                │
│  │  (MPSC-based) │ │ Registry  │  │ (Handlers +  │                │
│  │               │ │           │  │  MCP Client) │                │
│  └───────┬───────┘ └─────┬─────┘  └──────┬───────┘                │
│          │      ┌────────┴────────┐      │                         │
│          │      │   API Providers │      │                         │
│          │      │  Anthropic,     │      │                         │
│          │      │  OpenAI, Gemini,│      │                         │
│          │      │  OpenRouter     │      │                         │
│          │      └─────────────────┘      │                         │
│  ┌───────▼───────────────────────────────▼───────┐                │
│  │           Permission System                    │                │
│  │  (Per-tool approval, YOLO mode, command perms) │                │
│  └────────────────────────────────────────────────┘                │
└──────────────────────────────────────────────────────────────────────┘
```

For detailed module specs, types, and data flows see [plans/ARCHITECTURE.md](plans/ARCHITECTURE.md).

---

## Core Design Principles

1. **Zero-copy where possible** — `&str`, `Cow<'_, str>`, `bytes::Bytes` in hot paths.
2. **Async-first** — All I/O through tokio. TUI on its own thread.
3. **Channel-driven** — Components communicate via `tokio::sync::mpsc`, not shared mutable state.
4. **Trait-based providers** — Common `Provider` trait returning `Stream<Item = StreamChunk>`.
5. **Explicit permissions** — Every side-effecting tool call goes through the permission system.

---

## Reference Documents

| Document | Contents |
|----------|----------|
| [plans/ARCHITECTURE.md](plans/ARCHITECTURE.md) | Module structure, detailed type definitions, data flows, concurrency model |
| [plans/DEPENDENCIES.md](plans/DEPENDENCIES.md) | Full Cargo.toml, dependency decisions and rationale |
| [plans/CONFIG_REFERENCE.md](plans/CONFIG_REFERENCE.md) | config.toml format, MCP settings, CLI arguments, build commands |
| [plans/GIT_WORKFLOW.md](plans/GIT_WORKFLOW.md) | Branch naming, step lifecycle, commit/PR procedures |
| [plans/TRACKER.md](plans/TRACKER.md) | Implementation progress across all 37 steps |
| [plans/STEPXX.md](plans/) | Per-step specs with types, signatures, tests, acceptance criteria |

---

## Quality Requirements — NON-NEGOTIABLE

| Requirement | Enforcement |
|-------------|-------------|
| **ZERO compiler errors** | `cargo build` must succeed |
| **ZERO compiler warnings** | `cargo build 2>&1 \| grep warning` must produce nothing |
| **ZERO clippy lints** | `cargo clippy -- -D warnings` must pass |
| **ZERO fmt violations** | `cargo fmt --check` must pass |
| **Extensive test coverage** | Every public function, every error path, every edge case |
| **All tests pass** | `cargo test` must exit 0 |
| **No `unsafe` code** | `#![forbid(unsafe_code)]` in Cargo.toml lints |
| **No `unwrap()`/`expect()`** | In non-test code. Use `?` or return `Result` |

Run after EVERY step:
```bash
cargo fmt --check && cargo clippy -- -D warnings && cargo test
```

---

## Implementation Steps

Each step has a dedicated `plans/STEPXX.md` with exact types, signatures, tests, and acceptance criteria. See [plans/TRACKER.md](plans/TRACKER.md) for status.

### Phase 1: Foundation (Steps 01–04)
Scaffolding, state management, basic TUI, controller message loop.

### Phase 2: Provider Integration (Steps 05–10)
Provider trait, Anthropic/OpenAI/Gemini/OpenRouter, stream processing, end-to-end wiring.

### Phase 3: Tool System (Steps 11–16)
Tool handler trait, read-only tools, permissions, write tools, command execution, informational tools.

### Phase 4: Advanced Features (Steps 17–24)
Plan/Act modes, per-mode models, thinking view, YOLO mode, MCP client, sub-agents, task history.

### Phase 5: Polish (Steps 25–30)
Token counting, cost tracking, retry logic, chunk batching, config hot-reload, error messages.

### Phase 6: Critical Features (Steps 31–37)
Context window management, .mehignore, environment detection, user rules, slash commands, cancellation, system prompt builder.

---

## Code Conventions

- **Formatting**: `rustfmt` defaults. Run `cargo fmt` before every commit.
- **Linting**: `cargo clippy -- -D warnings` — all warnings are errors.
- **Error types**: `thiserror` for domain enums, `anyhow` for application boundaries.
- **Async**: All I/O is async. Use `tokio::runtime::Builder` for entry point (not `#[tokio::main]`).
- **Naming**: `snake_case` files/functions, `PascalCase` types, `SCREAMING_SNAKE` constants.
- **Tests**: Unit tests in same file (`#[cfg(test)]`), integration tests in `tests/`. Every public function tested. Error paths tested.
- **Logging**: `tracing` crate. `error` > `warn` > `info` > `debug` > `trace`.
- **Documentation**: `///` rustdoc on ALL functions (public and private). This is the ONLY form of documentation allowed in source code.
- **No inline comments**: Do not write numbered comments, step comments, TODO comments, or explanatory inline comments. If logic needs explanation, express it through rustdoc on the enclosing function or through clear naming. The code should be self-documenting.
- **Module docs in `mod.rs`**: Each `mod.rs` is the architectural home for its subsystem. Use `//!` doc comments to describe the module's purpose, responsibilities, data flows, and internal structure — including ASCII diagrams where they clarify relationships. `mod.rs` should never be a bare list of `pub mod` re-exports.
- **No panics**: Every `unwrap()`/`expect()` in non-test code is a bug.

---

## Security Notes

- API keys: Prefer env vars or OS keyring. Never log API keys.
- Command execution: Always validate against permission rules before spawning.
- File writes: Respect `.mehignore` for protected paths.
- MCP: Validate server binaries exist before spawning. Sanitize env var expansion.
- No network calls except to configured LLM provider endpoints and MCP servers.

---

## Key Differences from Cline

| Aspect | Cline (TS) | Meh (Rust) |
|--------|-----------|-------------|
| Runtime | Node.js single-threaded | Tokio multi-threaded |
| UI | VS Code Webview (React) | Terminal (Ratatui) |
| Concurrency | Promise-based | Tokio tasks, parallel tool execution |
| Streaming | AsyncIterableIterator | `Stream<Item = Result<StreamChunk>>` |
| IPC | VS Code postMessage | MPSC channels |
| Providers | 44+ | 4 (Anthropic, OpenAI, Gemini, OpenRouter) |
