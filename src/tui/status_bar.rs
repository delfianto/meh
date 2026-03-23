//! Status bar showing mode, model, token count, context utilization, and cost.

use crate::util::cost::{CostLevel, cost_level, format_cost};
use crate::util::tokens::{context_utilization, format_tokens};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

/// State for the status bar display.
pub struct StatusBarState {
    /// Current mode (ACT/PLAN).
    pub mode: String,
    /// Model name.
    pub model_name: String,
    /// Provider name.
    pub provider: String,
    /// Cumulative tokens across all API calls.
    pub total_tokens: u64,
    /// Cumulative cost in USD.
    pub total_cost: f64,
    /// Whether the agent is currently streaming.
    pub is_streaming: bool,
    /// Whether YOLO mode is active.
    pub is_yolo: bool,
    /// Estimated context window usage (tokens for next call).
    pub context_tokens: u64,
    /// Model's context window size.
    pub context_window: u32,
}

/// Render the status bar into the given area.
pub fn render_status_bar(frame: &mut Frame, area: Rect, state: &StatusBarState) {
    let mode_style = match state.mode.as_str() {
        "ACT" => Style::default().fg(Color::Black).bg(Color::Green).bold(),
        "PLAN" => Style::default().fg(Color::Black).bg(Color::Yellow).bold(),
        _ => Style::default().fg(Color::White).bold(),
    };

    let streaming_indicator = if state.is_streaming { " ⟳" } else { "" };

    let mut spans = vec![
        Span::styled(format!(" {} ", state.mode), mode_style),
        Span::raw(" "),
    ];

    if state.is_yolo {
        spans.push(Span::styled(
            " YOLO ",
            Style::default().fg(Color::White).bg(Color::Red).bold(),
        ));
        spans.push(Span::raw(" "));
    }

    spans.push(Span::styled(
        format!("{}/{}", state.provider, state.model_name),
        Style::default().fg(Color::Gray),
    ));

    if state.context_window > 0 {
        let pct = context_utilization(state.context_tokens, state.context_window);
        let ctx_color = if pct > 90.0 {
            Color::Red
        } else if pct > 75.0 {
            Color::Yellow
        } else {
            Color::Gray
        };
        spans.push(Span::styled("  ·  ", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(
            format!(
                "ctx: {}/{} ({:.0}%)",
                format_tokens(state.context_tokens),
                format_tokens(u64::from(state.context_window)),
                pct,
            ),
            Style::default().fg(ctx_color),
        ));
    }

    spans.extend([
        Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("total: {}", format_tokens(state.total_tokens)),
            Style::default().fg(Color::Gray),
        ),
        Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format_cost(state.total_cost),
            Style::default().fg(match cost_level(state.total_cost) {
                CostLevel::Normal => Color::Green,
                CostLevel::Moderate => Color::Yellow,
                CostLevel::Expensive => Color::Red,
            }),
        ),
        Span::styled(
            streaming_indicator.to_string(),
            Style::default().fg(Color::Cyan),
        ),
    ]);

    let line = Line::from(spans);
    let paragraph = Paragraph::new(line).style(Style::default().bg(Color::Rgb(30, 30, 30)));
    frame.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_tokens_display() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(500), "500");
        assert_eq!(format_tokens(1_000), "1.0k");
        assert_eq!(format_tokens(1_500), "1.5k");
        assert_eq!(format_tokens(1_000_000), "1.0M");
        assert_eq!(format_tokens(2_500_000), "2.5M");
    }

    #[test]
    fn context_utilization_percentage() {
        let pct = context_utilization(10_000, 200_000);
        assert!((pct - 5.0).abs() < 0.01);
    }
}
