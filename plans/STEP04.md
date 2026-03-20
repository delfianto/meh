# STEP 04 — Controller Message Loop

## Objective
Implement the Controller as the central message router. It receives messages from TUI, agents, and tools via MPSC channels and dispatches actions. After this step, the user's input flows from TUI -> Controller -> echo response back to TUI (no LLM yet).

## Prerequisites
- STEP 01-03 complete

## Detailed Instructions

### 4.1 Define message types (`src/controller/messages.rs`)

```rust
//! All messages exchanged between components via channels.

use crate::state::task_state::Mode;
use std::path::PathBuf;

/// Messages sent TO the Controller from any component.
#[derive(Debug, Clone)]
pub enum ControllerMessage {
    // === From TUI ===
    /// User submitted text input.
    UserSubmit {
        text: String,
        images: Vec<PathBuf>,
    },
    /// User responded to a tool approval prompt.
    ApprovalResponse {
        tool_use_id: String,
        approved: bool,
        always_allow: bool,
    },
    /// User requested task cancellation.
    CancelTask,
    /// User switched mode via UI.
    SwitchMode(Mode),
    /// User toggled thinking visibility.
    ToggleThinking,
    /// User requested quit.
    Quit,

    // === From Agent ===
    /// Streaming chunk from LLM provider.
    StreamChunk(StreamChunkWrapper),
    /// Agent requests a tool to be executed (needs approval check).
    ToolCallRequest(ToolCallRequest),
    /// Tool execution completed.
    ToolCallResult(ToolCallResult),
    /// Agent finished its task loop.
    TaskComplete(TaskResult),
    /// Agent encountered an error.
    TaskError(String),
}
```

Note: `StreamChunk` from the provider module isn't `Clone`-friendly in all cases, so wrap it:

```rust
/// Wrapper for stream chunks that can be sent through channels.
/// This is a newtype around the provider's StreamChunk to keep
/// the controller message type clean.
#[derive(Debug, Clone)]
pub struct StreamChunkWrapper {
    pub chunk_type: String,
    pub content: String,
    pub tool_id: Option<String>,
    pub tool_name: Option<String>,
    pub usage_input: Option<u64>,
    pub usage_output: Option<u64>,
}

/// A tool call that needs approval and execution.
#[derive(Debug, Clone)]
pub struct ToolCallRequest {
    pub tool_use_id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
    /// Human-readable description for the approval prompt.
    pub description: String,
}

/// Result of a tool execution.
#[derive(Debug, Clone)]
pub struct ToolCallResult {
    pub tool_use_id: String,
    pub content: String,
    pub is_error: bool,
}

/// Final result when a task completes.
#[derive(Debug, Clone)]
pub struct TaskResult {
    pub task_id: String,
    pub completion_message: Option<String>,
    pub total_tokens: u64,
    pub total_cost: f64,
}

/// Messages sent FROM the Controller TO the TUI for rendering.
#[derive(Debug, Clone)]
pub enum UiUpdate {
    /// Append a new chat message.
    AppendMessage {
        role: crate::tui::chat_view::ChatRole,
        content: String,
    },
    /// Update the content of the last assistant message (streaming).
    StreamContent {
        delta: String,
    },
    /// Finalize the current streaming message.
    StreamEnd,
    /// Show a tool approval prompt.
    ToolApproval {
        tool_use_id: String,
        tool_name: String,
        description: String,
    },
    /// Update status bar fields (only Some fields are updated).
    StatusUpdate {
        mode: Option<String>,
        tokens: Option<u64>,
        cost: Option<f64>,
        is_streaming: Option<bool>,
    },
    /// Show thinking/reasoning content.
    ThinkingContent {
        delta: String,
    },
    /// Signal that the app should quit.
    Quit,
}
```

### 4.2 Implement Controller (`src/controller/mod.rs`)

```rust
//! Central orchestrator — routes messages between TUI, agents, and tools.

pub mod messages;
pub mod task;

use messages::{ControllerMessage, UiUpdate};
use tokio::sync::mpsc;

/// The Controller is the central message router.
///
/// Architecture:
/// ```text
///   TUI ──────┐
///              │     ┌──────────┐     ┌───────┐
///              ├────>│Controller│────>│  TUI   │ (via ui_tx)
///              │     └──────────┘     └───────┘
///   Agent ────┘           │
///   Tools ────┘           └──> Agent, Tools (via controller_tx clones)
/// ```
///
/// All components send messages to the controller via `controller_tx`.
/// The controller sends UI updates via `ui_tx`.
pub struct Controller {
    /// Receive messages from all components.
    rx: mpsc::UnboundedReceiver<ControllerMessage>,
    /// Send updates to the TUI.
    ui_tx: mpsc::UnboundedSender<UiUpdate>,
    /// Sender clone given to components that need to talk to controller.
    controller_tx: mpsc::UnboundedSender<ControllerMessage>,
    /// State manager reference.
    state: crate::state::StateManager,
    /// Whether the controller is running.
    running: bool,
}

impl Controller {
    /// Create a new Controller and return it along with the channel endpoints.
    ///
    /// Returns:
    /// - `Self` — the controller (call `.run()` to start)
    /// - `UnboundedSender<ControllerMessage>` — send messages TO the controller
    /// - `UnboundedReceiver<UiUpdate>` — receive UI updates FROM the controller
    pub fn new(
        state: crate::state::StateManager,
    ) -> (
        Self,
        mpsc::UnboundedSender<ControllerMessage>,
        mpsc::UnboundedReceiver<UiUpdate>,
    ) {
        let (controller_tx, rx) = mpsc::unbounded_channel();
        let (ui_tx, ui_rx) = mpsc::unbounded_channel();
        let ctrl = Self {
            rx,
            ui_tx,
            controller_tx: controller_tx.clone(),
            state,
            running: true,
        };
        (ctrl, controller_tx, ui_rx)
    }

    /// Returns a clone of the sender for components to send messages to controller.
    pub fn sender(&self) -> mpsc::UnboundedSender<ControllerMessage> {
        self.controller_tx.clone()
    }

    /// Main message loop — runs as a tokio task.
    ///
    /// Exits when:
    /// - A `Quit` message is received
    /// - All senders are dropped (channel closed)
    pub async fn run(mut self) -> anyhow::Result<()> {
        tracing::info!("Controller started");
        while self.running {
            match self.rx.recv().await {
                Some(msg) => self.handle_message(msg).await?,
                None => {
                    tracing::info!("All senders dropped, controller shutting down");
                    break;
                }
            }
        }
        tracing::info!("Controller stopped");
        Ok(())
    }

    async fn handle_message(&mut self, msg: ControllerMessage) -> anyhow::Result<()> {
        match msg {
            ControllerMessage::UserSubmit { text, .. } => {
                tracing::info!(len = text.len(), "User submitted message");
                // For now: echo back as assistant message (placeholder until agent exists)
                let _ = self.ui_tx.send(UiUpdate::AppendMessage {
                    role: crate::tui::chat_view::ChatRole::Assistant,
                    content: format!("[echo] {text}"),
                });
            }
            ControllerMessage::Quit => {
                tracing::info!("Quit requested");
                let _ = self.ui_tx.send(UiUpdate::Quit);
                self.running = false;
            }
            ControllerMessage::CancelTask => {
                tracing::info!("Task cancellation requested");
                // Will be implemented when agent is added (STEP 07+)
            }
            ControllerMessage::ToggleThinking => {
                tracing::info!("Toggle thinking visibility");
                // Will be implemented when thinking view is added (STEP 09)
            }
            ControllerMessage::SwitchMode(mode) => {
                tracing::info!(?mode, "Mode switch requested");
                let mode_str = match mode {
                    crate::state::task_state::Mode::Plan => "PLAN",
                    crate::state::task_state::Mode::Act => "ACT",
                };
                let _ = self.ui_tx.send(UiUpdate::StatusUpdate {
                    mode: Some(mode_str.to_string()),
                    tokens: None,
                    cost: None,
                    is_streaming: None,
                });
            }
            ControllerMessage::ApprovalResponse { tool_use_id, approved, .. } => {
                tracing::info!(tool_use_id, approved, "Approval response received");
                // Will be forwarded to agent when approval flow is implemented
            }
            ControllerMessage::StreamChunk(_chunk) => {
                // Will be implemented when streaming is wired (STEP 07)
            }
            ControllerMessage::ToolCallRequest(_req) => {
                // Will be implemented when tool execution is wired
            }
            ControllerMessage::ToolCallResult(_result) => {
                // Will be implemented when tool execution is wired
            }
            ControllerMessage::TaskComplete(result) => {
                tracing::info!(task_id = result.task_id, "Task completed");
                if let Some(msg) = result.completion_message {
                    let _ = self.ui_tx.send(UiUpdate::AppendMessage {
                        role: crate::tui::chat_view::ChatRole::System,
                        content: msg,
                    });
                }
                let _ = self.ui_tx.send(UiUpdate::StatusUpdate {
                    mode: None,
                    tokens: Some(result.total_tokens),
                    cost: Some(result.total_cost),
                    is_streaming: Some(false),
                });
            }
            ControllerMessage::TaskError(error) => {
                tracing::error!(%error, "Task error");
                let _ = self.ui_tx.send(UiUpdate::AppendMessage {
                    role: crate::tui::chat_view::ChatRole::System,
                    content: format!("Error: {error}"),
                });
            }
        }
        Ok(())
    }
}
```

### 4.3 Update `app.rs` — Split TUI and Controller

The App now spawns the Controller as a tokio task and runs the TUI on the main thread (or a blocking task). This is the critical architectural split:

```rust
use crate::Cli;
use crate::controller::Controller;
use crate::controller::messages::{ControllerMessage, UiUpdate};
use crate::state::StateManager;
use crate::tui::{
    self,
    chat_view::{ChatMessage, ChatRole, ChatViewState},
    event::{poll_event, TuiEvent},
    input::InputWidget,
    status_bar::StatusBarState,
};
use crossterm::event::{KeyCode, KeyModifiers};
use std::time::Duration;
use tokio::sync::mpsc;

pub struct App {
    cli: Cli,
    state: StateManager,
}

impl App {
    pub async fn new(cli: Cli) -> anyhow::Result<Self> {
        let state = StateManager::new(cli.config.clone()).await?;
        Ok(Self { cli, state })
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let (controller, ctrl_tx, ui_rx) = Controller::new(self.state.clone());

        // Spawn controller as a background tokio task
        let controller_handle = tokio::spawn(controller.run());

        // Run TUI on a blocking thread (crossterm requires sync I/O)
        let config = self.state.config().await;
        let tui_result = tokio::task::spawn_blocking(move || {
            Self::run_tui(ctrl_tx, ui_rx, &config.provider.default)
        })
        .await?;

        // Clean shutdown: abort controller if still running
        controller_handle.abort();
        tui_result
    }

    /// The TUI event loop. Runs on a blocking thread.
    ///
    /// This function owns the terminal and handles:
    /// - Rendering the UI
    /// - Polling for keyboard/mouse events
    /// - Draining UI updates from the controller
    fn run_tui(
        ctrl_tx: mpsc::UnboundedSender<ControllerMessage>,
        mut ui_rx: mpsc::UnboundedReceiver<UiUpdate>,
        default_provider: &str,
    ) -> anyhow::Result<()> {
        let mut tui = tui::Tui::new()?;
        let mut chat_state = ChatViewState::new();
        let mut input = InputWidget::new();
        let mut status = StatusBarState {
            mode: "ACT".to_string(),
            model_name: "claude-sonnet-4-20250514".to_string(),
            provider: default_provider.to_string(),
            total_tokens: 0,
            total_cost: 0.0,
            is_streaming: false,
        };

        // Welcome message
        chat_state.push_message(ChatMessage {
            role: ChatRole::System,
            content: "Welcome to meh. Type a message to begin.".to_string(),
            timestamp: chrono::Utc::now(),
            streaming: false,
        });

        loop {
            // 1. Drain all pending UI updates (non-blocking)
            while let Ok(update) = ui_rx.try_recv() {
                match update {
                    UiUpdate::AppendMessage { role, content } => {
                        chat_state.push_message(ChatMessage {
                            role,
                            content,
                            timestamp: chrono::Utc::now(),
                            streaming: false,
                        });
                    }
                    UiUpdate::StreamContent { delta } => {
                        if let Some(last) = chat_state.messages.last_mut() {
                            last.content.push_str(&delta);
                        }
                    }
                    UiUpdate::StreamEnd => {
                        if let Some(last) = chat_state.messages.last_mut() {
                            last.streaming = false;
                        }
                    }
                    UiUpdate::StatusUpdate {
                        mode,
                        tokens,
                        cost,
                        is_streaming,
                    } => {
                        if let Some(m) = mode {
                            status.mode = m;
                        }
                        if let Some(t) = tokens {
                            status.total_tokens = t;
                        }
                        if let Some(c) = cost {
                            status.total_cost = c;
                        }
                        if let Some(s) = is_streaming {
                            status.is_streaming = s;
                        }
                    }
                    UiUpdate::ThinkingContent { .. } => {
                        // Will be handled when thinking view is implemented
                    }
                    UiUpdate::ToolApproval { .. } => {
                        // Will be handled when approval UI is implemented
                    }
                    UiUpdate::Quit => {
                        tui.restore()?;
                        return Ok(());
                    }
                }
            }

            // 2. Render
            tui.draw(|frame| {
                tui::app_layout::render_app(frame, &chat_state, &input, &status);
            })?;

            // 3. Poll for terminal events
            if let Some(event) = poll_event(Duration::from_millis(16)) {
                match event {
                    TuiEvent::Key(key) => {
                        // Ctrl+C → quit
                        if key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            let _ = ctrl_tx.send(ControllerMessage::Quit);
                            continue;
                        }
                        // Forward to input widget
                        if let Some(text) = input.handle_key(key) {
                            // Show user message immediately in chat
                            chat_state.push_message(ChatMessage {
                                role: ChatRole::User,
                                content: text.clone(),
                                timestamp: chrono::Utc::now(),
                                streaming: false,
                            });
                            // Send to controller for processing
                            let _ = ctrl_tx.send(ControllerMessage::UserSubmit {
                                text,
                                images: vec![],
                            });
                        }
                    }
                    TuiEvent::Resize(_, _) => {} // Ratatui handles resize
                    TuiEvent::Tick => {}
                    TuiEvent::Mouse(_) => {} // Future: scroll
                }
            }
        }
    }
}
```

### 4.4 Controller task management stub (`src/controller/task.rs`)

```rust
//! Task lifecycle management.
//!
//! This module will be expanded in STEP 07 to manage:
//! - Spawning agent tasks
//! - Tracking active task handles
//! - Cancellation of in-flight tasks
//! - Task history recording

/// Placeholder for task lifecycle management.
pub struct TaskManager {
    // Will hold:
    // - active_task: Option<JoinHandle<()>>
    // - task_state: TaskState
    // - agent channels
}

impl TaskManager {
    pub fn new() -> Self {
        Self {}
    }
}
```

## Tests

### Controller message routing tests
```rust
#[cfg(test)]
mod controller_tests {
    use crate::controller::Controller;
    use crate::controller::messages::{ControllerMessage, UiUpdate, TaskResult};
    use crate::state::StateManager;
    use crate::tui::chat_view::ChatRole;

    #[tokio::test]
    async fn test_controller_echoes_user_input() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let state = StateManager::new(Some(path)).await.unwrap();
        let (controller, ctrl_tx, mut ui_rx) = Controller::new(state);

        let handle = tokio::spawn(controller.run());

        // Send user input
        ctrl_tx
            .send(ControllerMessage::UserSubmit {
                text: "hello".to_string(),
                images: vec![],
            })
            .unwrap();

        // Should receive echo response
        let update = ui_rx.recv().await.unwrap();
        match update {
            UiUpdate::AppendMessage { content, role } => {
                assert!(content.contains("hello"));
                assert_eq!(role, ChatRole::Assistant);
            }
            other => panic!("Expected AppendMessage, got {other:?}"),
        }

        // Quit
        ctrl_tx.send(ControllerMessage::Quit).unwrap();
        let quit = ui_rx.recv().await.unwrap();
        assert!(matches!(quit, UiUpdate::Quit));

        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn test_controller_shuts_down_on_quit() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let state = StateManager::new(Some(path)).await.unwrap();
        let (controller, ctrl_tx, mut ui_rx) = Controller::new(state);

        let handle = tokio::spawn(controller.run());
        ctrl_tx.send(ControllerMessage::Quit).unwrap();

        let update = ui_rx.recv().await.unwrap();
        assert!(matches!(update, UiUpdate::Quit));

        // Controller task should complete cleanly
        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_controller_handles_sender_drop() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let state = StateManager::new(Some(path)).await.unwrap();
        let (controller, ctrl_tx, _ui_rx) = Controller::new(state);

        let handle = tokio::spawn(controller.run());

        // Drop all senders — controller should exit cleanly
        drop(ctrl_tx);

        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_controller_mode_switch() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let state = StateManager::new(Some(path)).await.unwrap();
        let (controller, ctrl_tx, mut ui_rx) = Controller::new(state);

        let handle = tokio::spawn(controller.run());

        ctrl_tx
            .send(ControllerMessage::SwitchMode(
                crate::state::task_state::Mode::Plan,
            ))
            .unwrap();

        let update = ui_rx.recv().await.unwrap();
        match update {
            UiUpdate::StatusUpdate { mode, .. } => {
                assert_eq!(mode, Some("PLAN".to_string()));
            }
            other => panic!("Expected StatusUpdate, got {other:?}"),
        }

        ctrl_tx.send(ControllerMessage::Quit).unwrap();
        let _ = ui_rx.recv().await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn test_controller_task_complete() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let state = StateManager::new(Some(path)).await.unwrap();
        let (controller, ctrl_tx, mut ui_rx) = Controller::new(state);

        let handle = tokio::spawn(controller.run());

        ctrl_tx
            .send(ControllerMessage::TaskComplete(TaskResult {
                task_id: "task-1".to_string(),
                completion_message: Some("Done!".to_string()),
                total_tokens: 500,
                total_cost: 0.01,
            }))
            .unwrap();

        // Should receive the completion message
        let update1 = ui_rx.recv().await.unwrap();
        match update1 {
            UiUpdate::AppendMessage { content, role } => {
                assert_eq!(content, "Done!");
                assert_eq!(role, ChatRole::System);
            }
            other => panic!("Expected AppendMessage, got {other:?}"),
        }

        // Should receive status update
        let update2 = ui_rx.recv().await.unwrap();
        match update2 {
            UiUpdate::StatusUpdate {
                tokens, cost, is_streaming, ..
            } => {
                assert_eq!(tokens, Some(500));
                assert_eq!(cost, Some(0.01));
                assert_eq!(is_streaming, Some(false));
            }
            other => panic!("Expected StatusUpdate, got {other:?}"),
        }

        ctrl_tx.send(ControllerMessage::Quit).unwrap();
        let _ = ui_rx.recv().await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn test_controller_task_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let state = StateManager::new(Some(path)).await.unwrap();
        let (controller, ctrl_tx, mut ui_rx) = Controller::new(state);

        let handle = tokio::spawn(controller.run());

        ctrl_tx
            .send(ControllerMessage::TaskError("something broke".to_string()))
            .unwrap();

        let update = ui_rx.recv().await.unwrap();
        match update {
            UiUpdate::AppendMessage { content, role } => {
                assert!(content.contains("something broke"));
                assert_eq!(role, ChatRole::System);
            }
            other => panic!("Expected AppendMessage, got {other:?}"),
        }

        ctrl_tx.send(ControllerMessage::Quit).unwrap();
        let _ = ui_rx.recv().await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn test_controller_multiple_messages() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let state = StateManager::new(Some(path)).await.unwrap();
        let (controller, ctrl_tx, mut ui_rx) = Controller::new(state);

        let handle = tokio::spawn(controller.run());

        // Send multiple messages rapidly
        for i in 0..5 {
            ctrl_tx
                .send(ControllerMessage::UserSubmit {
                    text: format!("msg-{i}"),
                    images: vec![],
                })
                .unwrap();
        }

        // Should receive all 5 echoes
        for i in 0..5 {
            let update = ui_rx.recv().await.unwrap();
            match update {
                UiUpdate::AppendMessage { content, .. } => {
                    assert!(content.contains(&format!("msg-{i}")));
                }
                other => panic!("Expected AppendMessage, got {other:?}"),
            }
        }

        ctrl_tx.send(ControllerMessage::Quit).unwrap();
        let _ = ui_rx.recv().await;
        handle.await.unwrap().unwrap();
    }
}
```

### Message type tests
```rust
#[cfg(test)]
mod message_tests {
    use super::messages::*;
    use crate::tui::chat_view::ChatRole;

    #[test]
    fn test_tool_call_request_creation() {
        let req = ToolCallRequest {
            tool_use_id: "tc-123".to_string(),
            tool_name: "read_file".to_string(),
            arguments: serde_json::json!({"path": "/src/main.rs"}),
            description: "Read /src/main.rs".to_string(),
        };
        assert_eq!(req.tool_name, "read_file");
        assert_eq!(req.arguments["path"], "/src/main.rs");
    }

    #[test]
    fn test_tool_call_result_creation() {
        let result = ToolCallResult {
            tool_use_id: "tc-123".to_string(),
            content: "file contents here".to_string(),
            is_error: false,
        };
        assert!(!result.is_error);
    }

    #[test]
    fn test_task_result_creation() {
        let result = TaskResult {
            task_id: "task-1".to_string(),
            completion_message: Some("All done".to_string()),
            total_tokens: 1000,
            total_cost: 0.05,
        };
        assert_eq!(result.total_tokens, 1000);
    }

    #[test]
    fn test_ui_update_variants() {
        let updates = vec![
            UiUpdate::AppendMessage {
                role: ChatRole::User,
                content: "hello".to_string(),
            },
            UiUpdate::StreamContent {
                delta: "partial".to_string(),
            },
            UiUpdate::StreamEnd,
            UiUpdate::Quit,
        ];
        assert_eq!(updates.len(), 4);
    }
}
```

## Acceptance Criteria
- [ ] Controller runs as a separate tokio task
- [ ] TUI and Controller communicate via unbounded MPSC channels
- [ ] User input flows: TUI -> `ControllerMessage::UserSubmit` -> Controller -> `UiUpdate::AppendMessage` -> TUI
- [ ] Echo response appears in chat view (confirms full round-trip)
- [ ] Mode switch sends `StatusUpdate` to TUI
- [ ] Task completion sends both message and status update
- [ ] Task error displays error message in chat
- [ ] Ctrl+C sends `Quit` message and app exits cleanly
- [ ] Terminal is always restored on exit (even on panic via `Drop`)
- [ ] Controller exits cleanly when all senders are dropped
- [ ] No deadlocks or channel starvation under rapid message sending
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo test` passes all controller message routing tests
