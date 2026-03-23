//! Status bar showing mode, model, token count, and cost.

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

/// State for the status bar display.
pub struct StatusBarState {
    pub mode: String,
    pub model_name: String,
    pub provider: String,
    pub total_tokens: u64,
    pub total_cost: f64,
    pub is_streaming: bool,
    pub is_yolo: bool,
}

/// Render the status bar into the given area.
#[allow(clippy::cast_precision_loss)]
pub fn render_status_bar(frame: &mut Frame, area: Rect, state: &StatusBarState) {
    let mode_style = match state.mode.as_str() {
        "ACT" => Style::default().fg(Color::Black).bg(Color::Green).bold(),
        "PLAN" => Style::default().fg(Color::Black).bg(Color::Yellow).bold(),
        _ => Style::default().fg(Color::White).bold(),
    };

    let tokens_display = if state.total_tokens >= 1_000_000 {
        format!("{:.1}M", state.total_tokens as f64 / 1_000_000.0)
    } else if state.total_tokens >= 1_000 {
        format!("{:.1}k", state.total_tokens as f64 / 1_000.0)
    } else {
        state.total_tokens.to_string()
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

    spans.extend(vec![
        Span::styled(
            format!("{}/{}", state.provider, state.model_name),
            Style::default().fg(Color::Gray),
        ),
        Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("tokens: {tokens_display}"),
            Style::default().fg(Color::Gray),
        ),
        Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("cost: ${:.4}", state.total_cost),
            Style::default().fg(Color::Gray),
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
    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn tokens_display_format() {
        let format_tokens = |total: u64| -> String {
            if total >= 1_000_000 {
                format!("{:.1}M", total as f64 / 1_000_000.0)
            } else if total >= 1_000 {
                format!("{:.1}k", total as f64 / 1_000.0)
            } else {
                total.to_string()
            }
        };

        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(500), "500");
        assert_eq!(format_tokens(1_000), "1.0k");
        assert_eq!(format_tokens(1_500), "1.5k");
        assert_eq!(format_tokens(1_000_000), "1.0M");
        assert_eq!(format_tokens(2_500_000), "2.5M");
    }
}
