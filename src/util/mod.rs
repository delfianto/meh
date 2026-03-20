//! Shared utilities used across multiple modules.
//!
//! ```text
//!   util/
//!     ├── fs.rs      ──► file read/write, diff generation, patch application
//!     ├── process.rs ──► subprocess execution with timeout and output capture
//!     ├── path.rs    ──► path resolution, workspace root detection
//!     ├── tokens.rs  ──► token counting (tiktoken-rs) and estimation
//!     └── cost.rs    ──► pricing database and cost calculation per model
//! ```

pub mod cost;
pub mod fs;
pub mod path;
pub mod process;
pub mod tokens;
