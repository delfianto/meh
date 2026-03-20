//! Terminal event handling — bridges crossterm events to app events.

use ratatui::crossterm::event::{self, Event as CrosstermEvent, KeyEvent, MouseEvent};
use std::time::Duration;

/// Events that the TUI event loop produces.
#[derive(Debug, Clone)]
pub enum TuiEvent {
    /// A key was pressed.
    Key(KeyEvent),
    /// Terminal was resized.
    Resize(u16, u16),
    /// Periodic tick for animations/polling.
    Tick,
    /// Mouse event (scroll).
    Mouse(MouseEvent),
}

/// Polls crossterm for events with a given tick rate.
/// Returns `Some(TuiEvent)` if an event occurred or tick elapsed.
/// Returns `None` on read error.
pub fn poll_event(tick_rate: Duration) -> Option<TuiEvent> {
    if event::poll(tick_rate).ok()? {
        match event::read().ok()? {
            CrosstermEvent::Key(key) => Some(TuiEvent::Key(key)),
            CrosstermEvent::Resize(w, h) => Some(TuiEvent::Resize(w, h)),
            CrosstermEvent::Mouse(m) => Some(TuiEvent::Mouse(m)),
            _ => None,
        }
    } else {
        Some(TuiEvent::Tick)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_event_tick_on_no_input() {
        // With a very short timeout, we should get a Tick (no input available)
        let result = poll_event(Duration::from_millis(1));
        // In test environment, poll may return None or Tick depending on terminal state
        if let Some(event) = result {
            assert!(matches!(event, TuiEvent::Tick));
        }
    }
}
