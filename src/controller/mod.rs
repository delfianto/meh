//! Central orchestrator — routes messages between TUI, agents, and tools.

pub mod messages;
pub mod task;

use messages::{ControllerMessage, UiUpdate};
use tokio::sync::mpsc;

/// The Controller is the central message router.
///
/// All components send messages to the controller via clones of the
/// `UnboundedSender<ControllerMessage>` returned by [`Controller::new`].
/// The controller sends UI updates via `ui_tx`.
pub struct Controller {
    /// Receives messages from all components.
    rx: mpsc::UnboundedReceiver<ControllerMessage>,
    /// Sends updates to the TUI.
    ui_tx: mpsc::UnboundedSender<UiUpdate>,
    /// Whether the controller is running.
    running: bool,
}

impl Controller {
    /// Create a new Controller and return it along with the channel endpoints.
    ///
    /// Returns `(controller, ctrl_tx, ui_rx)`:
    /// - `controller` — call `.run()` to start the message loop
    /// - `ctrl_tx` — clone and share with components that need to send messages
    /// - `ui_rx` — the TUI drains this for rendering updates
    pub fn new() -> (
        Self,
        mpsc::UnboundedSender<ControllerMessage>,
        mpsc::UnboundedReceiver<UiUpdate>,
    ) {
        let (ctrl_tx, rx) = mpsc::unbounded_channel();
        let (ui_tx, ui_rx) = mpsc::unbounded_channel();
        let ctrl = Self {
            rx,
            ui_tx,
            running: true,
        };
        (ctrl, ctrl_tx, ui_rx)
    }

    /// Main message loop — runs as a tokio task.
    ///
    /// Exits when a `Quit` message is received or all senders are dropped.
    pub async fn run(mut self) -> anyhow::Result<()> {
        tracing::info!("Controller started");
        while self.running {
            if let Some(msg) = self.rx.recv().await {
                self.handle_message(msg);
            } else {
                tracing::info!("All senders dropped, controller shutting down");
                break;
            }
        }
        tracing::info!("Controller stopped");
        Ok(())
    }

    /// Dispatch a single message to the appropriate handler.
    fn handle_message(&mut self, msg: ControllerMessage) {
        match msg {
            ControllerMessage::UserSubmit { text, .. } => {
                tracing::info!(len = text.len(), "User submitted message");
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
            }
            ControllerMessage::ToggleThinking => {
                tracing::info!("Toggle thinking visibility");
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
            ControllerMessage::ApprovalResponse {
                tool_use_id,
                approved,
                ..
            } => {
                tracing::info!(tool_use_id, approved, "Approval response received");
            }
            ControllerMessage::StreamChunk(_)
            | ControllerMessage::ToolCallRequest(_)
            | ControllerMessage::ToolCallResult(_) => {}
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
    }
}

#[cfg(test)]
mod controller_tests {
    use super::Controller;
    use super::messages::{ControllerMessage, TaskResult, UiUpdate};
    use crate::tui::chat_view::ChatRole;

    #[tokio::test]
    async fn controller_echoes_user_input() {
        let (controller, ctrl_tx, mut ui_rx) = Controller::new();
        let handle = tokio::spawn(controller.run());

        ctrl_tx
            .send(ControllerMessage::UserSubmit {
                text: "hello".to_string(),
                images: vec![],
            })
            .unwrap();

        let update = ui_rx.recv().await.unwrap();
        match update {
            UiUpdate::AppendMessage { content, role } => {
                assert!(content.contains("hello"));
                assert_eq!(role, ChatRole::Assistant);
            }
            other => panic!("Expected AppendMessage, got {other:?}"),
        }

        ctrl_tx.send(ControllerMessage::Quit).unwrap();
        let quit = ui_rx.recv().await.unwrap();
        assert!(matches!(quit, UiUpdate::Quit));

        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn controller_shuts_down_on_quit() {
        let (controller, ctrl_tx, mut ui_rx) = Controller::new();
        let handle = tokio::spawn(controller.run());

        ctrl_tx.send(ControllerMessage::Quit).unwrap();

        let update = ui_rx.recv().await.unwrap();
        assert!(matches!(update, UiUpdate::Quit));

        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn controller_handles_sender_drop() {
        let (controller, ctrl_tx, _ui_rx) = Controller::new();
        let handle = tokio::spawn(controller.run());

        drop(ctrl_tx);

        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn controller_mode_switch() {
        let (controller, ctrl_tx, mut ui_rx) = Controller::new();
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
    async fn controller_task_complete() {
        let (controller, ctrl_tx, mut ui_rx) = Controller::new();
        let handle = tokio::spawn(controller.run());

        ctrl_tx
            .send(ControllerMessage::TaskComplete(TaskResult {
                task_id: "task-1".to_string(),
                completion_message: Some("Done!".to_string()),
                total_tokens: 500,
                total_cost: 0.01,
            }))
            .unwrap();

        let update1 = ui_rx.recv().await.unwrap();
        match update1 {
            UiUpdate::AppendMessage { content, role } => {
                assert_eq!(content, "Done!");
                assert_eq!(role, ChatRole::System);
            }
            other => panic!("Expected AppendMessage, got {other:?}"),
        }

        let update2 = ui_rx.recv().await.unwrap();
        match update2 {
            UiUpdate::StatusUpdate {
                tokens,
                cost,
                is_streaming,
                ..
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
    async fn controller_task_error() {
        let (controller, ctrl_tx, mut ui_rx) = Controller::new();
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
    async fn controller_multiple_messages() {
        let (controller, ctrl_tx, mut ui_rx) = Controller::new();
        let handle = tokio::spawn(controller.run());

        for i in 0..5 {
            ctrl_tx
                .send(ControllerMessage::UserSubmit {
                    text: format!("msg-{i}"),
                    images: vec![],
                })
                .unwrap();
        }

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
