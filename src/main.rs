//! Meh — Terminal-based AI coding assistant
// Allow dead code for stub modules not yet wired into the app.
// Remove this as modules get implemented and connected.
#![allow(dead_code)]

mod agent;
mod app;
mod context;
mod controller;
mod ignore;
mod permission;
mod prompt;
mod provider;
mod state;
mod streaming;
mod tool;
mod tui;
mod util;

use clap::Parser;

/// Terminal-based AI coding assistant.
#[derive(Parser, Debug)]
#[command(name = "meh", version, about = "Terminal-based AI coding assistant")]
pub struct Cli {
    /// Initial prompt to send.
    #[arg(value_name = "PROMPT")]
    pub prompt: Option<String>,

    /// Override provider (anthropic|openai|gemini|openrouter).
    #[arg(short, long)]
    pub provider: Option<String>,

    /// Override model ID.
    #[arg(short, long)]
    pub model: Option<String>,

    /// Start in mode (plan|act).
    #[arg(long)]
    pub mode: Option<String>,

    /// Enable YOLO mode (no approval prompts).
    #[arg(long)]
    pub yolo: bool,

    /// Config file path.
    #[arg(short, long)]
    pub config: Option<std::path::PathBuf>,

    /// Resume a previous task.
    #[arg(long)]
    pub resume: Option<String>,

    /// Verbose logging.
    #[arg(short, long)]
    pub verbose: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let filter = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter)),
        )
        .with_target(false)
        .init();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async {
        let app = app::App::new(cli).await?;
        app.run().await
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn cli_no_args() {
        let cli = Cli::try_parse_from(["meh"]);
        assert!(cli.is_ok());
    }

    #[test]
    fn cli_with_prompt() {
        let cli = Cli::try_parse_from(["meh", "fix the bug"]).unwrap();
        assert_eq!(cli.prompt.as_deref(), Some("fix the bug"));
    }

    #[test]
    fn cli_with_all_flags() {
        let cli = Cli::try_parse_from([
            "meh",
            "--provider",
            "anthropic",
            "--model",
            "claude-sonnet-4-20250514",
            "--mode",
            "plan",
            "--yolo",
            "--verbose",
            "do something",
        ])
        .unwrap();
        assert_eq!(cli.provider.as_deref(), Some("anthropic"));
        assert_eq!(cli.model.as_deref(), Some("claude-sonnet-4-20250514"));
        assert_eq!(cli.mode.as_deref(), Some("plan"));
        assert!(cli.yolo);
        assert!(cli.verbose);
        assert_eq!(cli.prompt.as_deref(), Some("do something"));
    }

    #[test]
    fn cli_yolo_flag() {
        let cli = Cli::try_parse_from(["meh", "--yolo"]).unwrap();
        assert!(cli.yolo);
    }

    #[test]
    fn cli_resume() {
        let cli = Cli::try_parse_from(["meh", "--resume", "abc-123"]).unwrap();
        assert_eq!(cli.resume.as_deref(), Some("abc-123"));
    }

    #[test]
    fn cli_config_path() {
        let cli = Cli::try_parse_from(["meh", "--config", "/tmp/config.toml"]).unwrap();
        assert_eq!(
            cli.config.as_deref(),
            Some(std::path::Path::new("/tmp/config.toml"))
        );
    }
}
