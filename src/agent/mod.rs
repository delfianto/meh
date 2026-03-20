//! Agent system — autonomous task execution and delegation.
//!
//! Agents are the execution units that drive the LLM conversation loop.
//! Each agent runs as a tokio task, calling the provider, parsing tool
//! calls from the stream, and communicating results back to the controller.
//!
//! ```text
//!                    ┌──────────────────────────┐
//!                    │       Controller         │
//!                    │  (ControllerMessage rx)  │
//!                    └────┬──────────────▲──────┘
//!      AgentMessage       │              │  ControllerMessage
//!      (tool results,     │              │  (stream chunks,
//!       cancel, mode)     ▼              │   tool requests)
//!                    ┌────────────────────┐
//!                    │     TaskAgent      │
//!                    │  ┌──────────────┐  │
//!                    │  │ Provider API │  │
//!                    │  │   (stream)   │  │
//!                    │  └──────────────┘  │
//!                    │  ┌──────────────┐  │
//!                    │  │  Messages[]  │  │
//!                    │  │  (context)   │  │
//!                    │  └──────────────┘  │
//!                    └────────┬───────────┘
//!                             │ spawn
//!                    ┌────────▼───────────┐
//!                    │     SubAgent       │
//!                    │  (own context,     │
//!                    │   shared perms)    │
//!                    └────────────────────┘
//! ```
//!
//! The `TaskAgent` owns the main conversation loop: call the provider,
//! stream the response, parse tool calls, request execution through
//! the controller, and loop until `attempt_completion` is received
//! or the task is cancelled.
//!
//! A `SubAgent` can be spawned by the main agent for delegated subtasks.
//! It gets its own conversation context but shares permission state
//! with the parent.

pub mod sub_agent;
pub mod task_agent;

pub use task_agent::{AgentMessage, TaskAgent};
