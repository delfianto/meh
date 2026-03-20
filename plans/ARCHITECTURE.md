# Meh — Architecture & Module Specifications

## Crate & Module Structure

```
meh/
├── Cargo.toml
├── CLAUDE.md
├── src/
│   ├── main.rs                  # Entry point, arg parsing, tokio runtime
│   ├── app.rs                   # App struct — owns Controller, TUI, runs event loop
│   │
│   ├── tui/                     # Terminal UI (Ratatui)
│   │   ├── mod.rs
│   │   ├── app_layout.rs        # Main layout: chat pane, status bar, input
│   │   ├── chat_view.rs         # Renders conversation messages (markdown)
│   │   ├── input.rs             # User input widget (multiline, keybindings)
│   │   ├── thinking_view.rs     # Collapsible chain-of-thought panel
│   │   ├── tool_view.rs         # Tool call display (approval prompts)
│   │   ├── status_bar.rs        # Mode indicator, model name, token count
│   │   ├── settings_view.rs     # Provider/model/mode configuration screen
│   │   └── event.rs             # Crossterm event handling, key dispatch
│   │
│   ├── controller/              # Central orchestrator
│   │   ├── mod.rs               # Controller struct, message routing
│   │   ├── task.rs              # Task lifecycle (create, run, cancel, resume)
│   │   └── messages.rs          # ControllerMessage enum (all inter-component msgs)
│   │
│   ├── provider/                # LLM provider implementations
│   │   ├── mod.rs               # Provider trait, StreamChunk types, registry
│   │   ├── anthropic.rs         # Claude models (native tool use, thinking)
│   │   ├── openai.rs            # GPT/O-series (completion + responses API)
│   │   ├── gemini.rs            # Gemini models (native tool use, thinking)
│   │   ├── openrouter.rs        # OpenRouter (wraps OpenAI-compatible API)
│   │   └── common.rs            # Shared HTTP client, retry logic, error types
│   │
│   ├── tool/                    # Tool system
│   │   ├── mod.rs               # ToolHandler trait, ToolUse types, registry
│   │   ├── executor.rs          # Routes tool calls to handlers, manages approval
│   │   ├── definitions.rs       # Tool schemas for system prompt injection
│   │   ├── handlers/
│   │   │   ├── read_file.rs
│   │   │   ├── write_file.rs
│   │   │   ├── apply_patch.rs
│   │   │   ├── execute_command.rs
│   │   │   ├── search_files.rs
│   │   │   ├── list_files.rs
│   │   │   ├── ask_followup.rs
│   │   │   ├── attempt_completion.rs
│   │   │   ├── plan_mode_respond.rs
│   │   │   └── mcp_tool.rs
│   │   └── mcp/                 # MCP client integration
│   │       ├── mod.rs           # McpHub — manages server connections
│   │       ├── client.rs        # MCP protocol client (JSON-RPC over stdio/HTTP)
│   │       ├── transport.rs     # Stdio, SSE, StreamableHTTP transports
│   │       └── types.rs         # MCP protocol types
│   │
│   ├── agent/                   # Agent system
│   │   ├── mod.rs               # Agent trait, AgentMessage enum
│   │   ├── task_agent.rs        # Main task agent (conversation loop)
│   │   └── sub_agent.rs         # Nested agent for delegation
│   │
│   ├── permission/              # Permission & approval system
│   │   ├── mod.rs               # PermissionController, PermissionResult
│   │   ├── command_perms.rs     # Shell command validation (glob patterns)
│   │   ├── auto_approve.rs      # Auto-approval rules per tool category
│   │   └── yolo.rs              # YOLO mode — approve everything
│   │
│   ├── state/                   # State management
│   │   ├── mod.rs               # StateManager — in-memory cache + disk persistence
│   │   ├── config.rs            # App configuration (provider, model, mode, keys)
│   │   ├── history.rs           # Conversation/task history
│   │   ├── secrets.rs           # API key storage (keyring or encrypted file)
│   │   └── task_state.rs        # Per-task mutable state
│   │
│   ├── prompt/                  # System prompt construction
│   │   ├── mod.rs               # build_system_prompt() — assembles full prompt
│   │   ├── base.rs              # Base system prompt template
│   │   ├── tools_section.rs     # Tool definitions section
│   │   ├── rules.rs             # User rules (.meh/rules, .mehrules)
│   │   ├── context.rs           # Workspace context, file tree, environment info
│   │   └── environment.rs       # OS, shell, cwd, language detection
│   │
│   ├── context/                 # Context window management
│   │   ├── mod.rs               # ContextManager — tracks token budget
│   │   ├── summarizer.rs        # Conversation summarization when context full
│   │   └── truncation.rs        # Message truncation strategies
│   │
│   ├── ignore/                  # Path protection
│   │   ├── mod.rs               # IgnoreController — .mehignore filtering
│   │   └── rules.rs             # Rule file loading with YAML frontmatter
│   │
│   ├── streaming/               # Stream processing
│   │   ├── mod.rs               # StreamProcessor — parses streaming chunks
│   │   ├── tool_parser.rs       # Incremental JSON parsing for tool calls
│   │   ├── thinking_parser.rs   # Reasoning/thinking block accumulator
│   │   └── chunk_batcher.rs     # Batches rapid UI updates to prevent flicker
│   │
│   └── util/                    # Shared utilities
│       ├── mod.rs
│       ├── fs.rs                # File operations (read, write, diff, patch)
│       ├── process.rs           # Subprocess execution with timeout
│       ├── path.rs              # Path resolution, workspace detection
│       ├── tokens.rs            # Token counting (tiktoken-rs or estimate)
│       └── cost.rs              # Pricing database and cost calculation
│
├── config/                      # Default config files
│   └── default_settings.toml
│
└── tests/
    ├── integration/
    │   ├── provider_tests.rs
    │   ├── tool_tests.rs
    │   └── mcp_tests.rs
    └── unit/
        ├── stream_parser_test.rs
        ├── permission_test.rs
        └── command_perms_test.rs
```

---

## 1. TUI Layer (`src/tui/`)

**Framework**: Ratatui + Crossterm

**Layout** (top to bottom):
```
┌─────────────────────────────────────────────────┐
│ Status Bar: [PLAN|ACT] model_name  tokens: 1.2k │
├─────────────────────────────────────────────────┤
│                                                   │
│  Chat View (scrollable)                          │
│  ├── User message                                │
│  ├── Assistant message (streaming...)            │
│  ├── [Thinking] (collapsible)                    │
│  │   └── chain of thought text                   │
│  ├── Tool Call: read_file("src/main.rs")         │
│  │   └── [Approve] [Deny] [Always Allow]         │
│  ├── Tool Result: (file contents)                │
│  └── Assistant continues...                      │
│                                                   │
├─────────────────────────────────────────────────┤
│ > User input area (multiline)                    │
│   [Enter to send, Shift+Enter newline]           │
└─────────────────────────────────────────────────┘
```

**Key Behaviors**:
- The TUI runs on its own **dedicated thread** (not a tokio task) to ensure UI never blocks on async work.
- Communication: `mpsc::UnboundedSender<UiEvent>` from TUI → Controller, `mpsc::UnboundedSender<UiUpdate>` from Controller → TUI.
- **Thinking toggle**: `Ctrl+T` collapses/expands all thinking blocks. Thinking is shown by default during streaming, collapsed after completion.
- **Auto-scroll**: Chat view auto-scrolls to bottom during streaming unless user has scrolled up.
- **Streaming text**: Assistant text renders character-by-character as chunks arrive. Tool call JSON renders incrementally.
- **Approval prompts**: Inline in chat view. Keybindings: `y` approve, `n` deny, `a` always-allow for this tool type.

**Event Loop** (TUI thread):
```rust
loop {
    // 1. Poll crossterm events (16ms tick for ~60fps)
    // 2. Process any pending UiUpdate messages from controller
    // 3. Re-render frame via terminal.draw(|f| { ... })
}
```

---

## 2. Controller (`src/controller/`)

The Controller is the central message router. It owns no heavy state directly — it coordinates between components via channels.

**ControllerMessage enum**:
```rust
pub enum ControllerMessage {
    // From TUI
    UserSubmit { text: String, images: Vec<PathBuf> },
    ApprovalResponse { tool_use_id: String, approved: bool, always: bool },
    CancelTask,
    SwitchMode(Mode),
    UpdateConfig(ConfigUpdate),
    ToggleThinking,
    Quit,

    // From Agent/Task
    StreamChunk(StreamChunk),
    ToolCallRequest(ToolCallRequest),
    ToolCallResult(ToolCallResult),
    TaskComplete(TaskResult),
    TaskError(anyhow::Error),
    AgentSpawned { agent_id: String },

    // From Provider (via agent)
    ApiError(ApiError),

    // Internal
    Tick,
}
```

**Controller loop** (tokio task):
```rust
loop {
    tokio::select! {
        Some(msg) = self.rx.recv() => self.handle_message(msg).await,
        _ = shutdown.recv() => break,
    }
}
```

---

## 3. Provider System (`src/provider/`)

**Core trait**:
```rust
#[async_trait]
pub trait Provider: Send + Sync {
    async fn create_message(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &ModelConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>>;

    fn model_info(&self) -> &ModelInfo;
    fn abort(&self);
}
```

**StreamChunk enum** (normalized across all providers):
```rust
pub enum StreamChunk {
    Text { delta: String },
    Thinking { delta: String, signature: Option<String>, redacted: bool },
    ToolCall { id: String, name: String, arguments_delta: String },
    ToolCallComplete { id: String, name: String, arguments: serde_json::Value },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: Option<u64>,
        cache_write_tokens: Option<u64>,
        thinking_tokens: Option<u64>,
        total_cost: Option<f64>,
    },
    Done,
}
```

### Provider-Specific Notes

**Anthropic** (`anthropic.rs`):
- Uses `reqwest` with SSE streaming (Messages API).
- Maps Claude's content blocks: `text`, `thinking`, `tool_use`.
- Supports `thinking.budget_tokens` for extended thinking.
- Prompt caching via `cache_control: { type: "ephemeral" }` on system prompt.
- Thinking signatures preserved for multi-turn verification.

**OpenAI** (`openai.rs`):
- Dual path: standard Chat Completions API and Responses API (for newer models).
- WebSocket support via `tokio-tungstenite` for Responses API (lower latency).
- Reasoning effort mapping: "low"/"medium"/"high" → `reasoning_effort` param.
- O-series models may not support streaming — detect and handle.

**Gemini** (`gemini.rs`):
- Uses `reqwest` with SSE to `streamGenerateContent` endpoint.
- Parts array: each part can have `text`, `thought`, `functionCall`.
- Thinking budget: 0 = disabled, -1 = dynamic, >0 = fixed budget.
- Implicit caching (server-side, tracked by cost).

**OpenRouter** (`openrouter.rs`):
- OpenAI-compatible API format with extra fields.
- Usage may not arrive in stream — fallback: poll generation endpoint after 500ms.
- Reasoning details passthrough (model-specific format).
- Error detection: check `finish_reason === "error"` in chunk.

### Retry Logic

All providers share a common retry wrapper:
```rust
pub async fn with_retry<F, Fut, T>(f: F, max_retries: u32) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    // Exponential backoff: 1s, 2s, 4s, 8s...
    // Retry on: 429 (rate limit), 500+ (server error), connection errors
    // Do NOT retry on: 401 (auth), 400 (bad request), 404
}
```

---

## 4. Tool System (`src/tool/`)

**ToolHandler trait**:
```rust
#[async_trait]
pub trait ToolHandler: Send + Sync {
    fn name(&self) -> &str;
    async fn execute(&self, params: serde_json::Value, ctx: &ToolContext) -> Result<ToolResponse>;
    fn requires_approval(&self) -> bool { true }
    fn category(&self) -> ToolCategory;
}
```

**ToolCategory enum**:
```rust
pub enum ToolCategory {
    ReadOnly,       // read_file, list_files, search_files
    FileWrite,      // write_file, apply_patch
    Command,        // execute_command
    Browser,        // (future: browser_action)
    Mcp,            // mcp_tool
    Informational,  // ask_followup, attempt_completion, plan_mode_respond
}
```

**Implemented Tools**:

| Tool | Category | Description |
|------|----------|-------------|
| `read_file` | ReadOnly | Read file contents with line range support |
| `write_file` | FileWrite | Create or overwrite a file |
| `apply_patch` | FileWrite | Apply a unified diff patch to one or more files |
| `execute_command` | Command | Run shell command with timeout |
| `search_files` | ReadOnly | Regex search across files (uses `grep` or `ripgrep`) |
| `list_files` | ReadOnly | List directory contents (recursive optional) |
| `ask_followup_question` | Informational | Ask user for clarification |
| `attempt_completion` | Informational | Signal task completion with result |
| `plan_mode_respond` | Informational | Respond in plan mode / request switch to act |
| `mcp_tool` | Mcp | Call an MCP server tool |

**Tool Execution Flow**:
```
LLM emits tool_use → StreamProcessor parses it → Agent sends ToolCallRequest to Controller
  → Controller checks permissions (PermissionController)
    → If auto-approved: execute immediately
    → If needs approval: send to TUI, wait for ApprovalResponse
  → On approval: execute tool handler
  → Send ToolCallResult back to Agent
  → Agent includes result in next API call
```

**Command Execution** (`execute_command` handler):
- Spawns subprocess via `tokio::process::Command`.
- Default timeout: 30s. Long-running commands (npm, cargo, docker, etc.): 300s.
- Captures stdout + stderr, streams output to TUI in real-time.
- Validates command against permission rules before execution.

---

## 5. Agent System (`src/agent/`)

Agents are the autonomous execution units. Each agent runs as a tokio task and communicates with the Controller via MPSC channels.

**Agent lifecycle**:
```rust
pub struct TaskAgent {
    id: String,
    provider: Arc<dyn Provider>,
    tools: Arc<ToolRegistry>,
    tx: mpsc::Sender<ControllerMessage>,
    rx: mpsc::Receiver<AgentMessage>,
    messages: Vec<Message>,
    mode: Mode,
    system_prompt: String,
    config: ModelConfig,
}

impl TaskAgent {
    pub async fn run(&mut self) -> Result<TaskResult> {
        loop {
            // 1. Call provider.create_message() with current context
            // 2. Stream response, forwarding chunks to controller
            // 3. Parse tool calls from stream
            // 4. For each tool call: request → wait for result → append
            // 5. If attempt_completion received, return result
            // 6. If cancelled, return early
            // 7. Loop back to step 1
        }
    }
}
```

**AgentMessage enum** (Controller → Agent):
```rust
pub enum AgentMessage {
    ToolCallResult(ToolCallResult),
    Cancel,
    ModeSwitch(Mode),
    ConfigUpdate(ModelConfig),
}
```

**Sub-agents**: A TaskAgent can request spawning a sub-agent for delegation. The sub-agent gets its own conversation context but shares the same permission state.

---

## 6. Plan/Act Mode (`src/controller/task.rs`)

- **Plan Mode**: Read-only tools + `plan_mode_respond` only. LLM analyzes and proposes.
- **Act Mode**: Full tool access. LLM executes its plan.
- **Strict Plan Mode**: Must go through plan phase first.

```rust
pub struct ModeConfig {
    pub plan_model: ModelSelector,
    pub act_model: ModelSelector,
    pub plan_thinking_budget: Option<u32>,
    pub act_thinking_budget: Option<u32>,
    pub auto_act: bool,
}
```

**Mode Switching Flow**: Plan → `plan_mode_respond` → TUI review → approve → `ModeSwitch(Act)` → rebuild system prompt → continue in Act mode.

---

## 7. Permission System (`src/permission/`)

| Tier | Behavior |
|------|----------|
| **Ask** (default) | Every side-effecting tool call requires explicit user approval |
| **Auto-approve** | Granular rules per category (read=auto, write=ask, command=ask) |
| **YOLO** | Everything auto-approved |

```rust
pub struct PermissionController {
    mode: PermissionMode,
    auto_approve_rules: AutoApproveRules,
    command_permissions: CommandPermissions,
    always_allowed: HashSet<String>,
}
```

**Command Permission Validation**: Shell-quote tokenization, allow/deny glob patterns, operator splitting (`&&`, `||`, `|`, `;`), subshell recursion, deny takes precedence.

---

## 8. Streaming & Parsing (`src/streaming/`)

- **StreamProcessor**: Consumes raw `StreamChunk`s, produces structured events.
- **ToolParser**: Incremental JSON parsing for tool call arguments.
- **ThinkingParser**: Accumulates thinking chunks, tracks signatures.
- **ChunkBatcher**: Debounces rapid updates (16ms window) to prevent TUI flicker.

---

## 9. MCP Integration (`src/tool/mcp/`)

Uses `rmcp` (official Rust MCP SDK). McpHub manages server connections.

**Transport Types**: Stdio, SSE, StreamableHTTP.

**MCP Tool Flow**: Startup → connect servers → fetch tool lists → inject into system prompt → route `mcp_tool` calls → return results.

---

## 10. State Management (`src/state/`)

**Storage locations**:
```
~/.config/meh/
├── config.toml              # User configuration
├── mcp_settings.json        # MCP server configuration
├── history/                 # Task history (one JSON per task)
├── rules/                   # Global user rules
└── data/
    └── state.json           # Persisted global state
```

---

## Message & Conversation Format

```rust
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

pub enum ContentBlock {
    Text(String),
    Thinking { text: String, signature: Option<String>, redacted: bool },
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, content: String, is_error: bool },
    Image { media_type: String, data: Vec<u8> },
}

pub enum Role { User, Assistant, System }
```

Each provider module has `fn convert_messages(messages: &[Message]) -> ProviderMessages`.

---

## System Prompt Construction

Assembled dynamically per-request:
1. Base prompt (role definition, capabilities, behavioral rules)
2. Tool definitions (filtered by current mode — plan vs act)
3. MCP tool definitions (from connected servers)
4. User rules (~/.config/meh/rules/ and .mehrules in workspace)
5. Workspace context (cwd, file tree, OS/environment info)
6. Mode-specific instructions

---

## Concurrency Model

```
Main thread:        TUI event loop (crossterm + ratatui rendering)
Tokio runtime:
  Task 1:           Controller message loop
  Task 2:           Active TaskAgent (conversation loop)
  Task 3..N:        Tool executions (spawned per tool call)
  Task N+1..M:      MCP server connections
  Background:       State persistence, config file watchers
```

**Synchronization**: MPSC channels (no shared mutexes in hot paths), `RwLock` for config, `CancellationToken` for abort, `tokio::fs` for non-blocking file I/O.

---

## Error Handling Strategy

- **Provider errors**: Retriable (429, 500+, connection) with exponential backoff. Fatal (401, 400) surface immediately.
- **Tool errors**: Return error text to LLM for self-correction. Only catastrophic failures abort.
- **MCP errors**: Reconnect on disconnect. Tool call failures return error to LLM.
- **TUI errors**: Logged but don't crash.
- Use `anyhow::Result` at application level, `thiserror` for domain error enums.
