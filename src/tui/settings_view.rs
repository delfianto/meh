//! Interactive settings panel — tabbed UI for provider, keys, models, permissions.
//!
//! Rendered as an overlay on the bottom portion of the screen when
//! `/settings` is invoked. Three tabs: API, Permissions, Features.
//! Keyboard-driven: arrows navigate, Enter edits, Space toggles, Esc closes.

use crate::state::config::AppConfig;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs};

/// Active tab in the settings panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    Api,
    Permissions,
    Features,
}

const TAB_COUNT: usize = 3;

/// A single editable setting row.
#[derive(Debug, Clone)]
pub struct SettingRow {
    pub key: String,
    pub label: String,
    pub value: SettingValue,
    pub description: String,
}

/// Types of setting values.
#[derive(Debug, Clone)]
pub enum SettingValue {
    /// Plain text field.
    Text(String),
    /// Password-masked field (API keys).
    Secret(String),
    /// Boolean toggle.
    Toggle(bool),
    /// Selection from a fixed list.
    Select {
        options: Vec<String>,
        selected: usize,
    },
}

/// State when a row is being edited.
#[derive(Debug, Clone)]
pub enum EditState {
    /// Text/secret input mode.
    TextInput {
        buffer: String,
        cursor: usize,
        masked: bool,
    },
    /// List picker mode.
    ListPicker {
        options: Vec<String>,
        selected: usize,
    },
}

/// Actions returned from settings key handling.
pub enum SettingsAction {
    /// Stay in settings view.
    Continue,
    /// Close settings, return to chat.
    Close,
    /// Apply a config change.
    Apply(SettingsChange),
}

/// A config change to persist.
#[derive(Debug, Clone)]
pub struct SettingsChange {
    pub key: String,
    pub value: String,
}

/// The full settings view state.
pub struct SettingsView {
    pub tab: SettingsTab,
    pub rows: Vec<SettingRow>,
    pub selected_row: usize,
    pub scroll_offset: usize,
    pub editing: Option<EditState>,
}

impl SettingsView {
    /// Create a new settings view populated from the given config.
    pub fn new(config: &AppConfig) -> Self {
        let mut view = Self {
            tab: SettingsTab::Api,
            rows: Vec::new(),
            selected_row: 0,
            scroll_offset: 0,
            editing: None,
        };
        view.rebuild_rows(config);
        view
    }

    /// Rebuild rows for the current tab from config.
    pub fn rebuild_rows(&mut self, config: &AppConfig) {
        self.rows = match self.tab {
            SettingsTab::Api => build_api_rows(config),
            SettingsTab::Permissions => build_permissions_rows(config),
            SettingsTab::Features => build_features_rows(config),
        };
        self.selected_row = 0;
        self.scroll_offset = 0;
    }

    /// Handle a key event. Returns the action to take.
    pub fn handle_key(&mut self, key: KeyEvent, config: &AppConfig) -> SettingsAction {
        if let Some(ref mut edit) = self.editing {
            return handle_edit_key(key, edit, &self.rows[self.selected_row]);
        }

        match key.code {
            KeyCode::Left => {
                self.prev_tab();
                self.rebuild_rows(config);
                SettingsAction::Continue
            }
            KeyCode::Right => {
                self.next_tab();
                self.rebuild_rows(config);
                SettingsAction::Continue
            }
            KeyCode::Char('1') => {
                self.tab = SettingsTab::Api;
                self.rebuild_rows(config);
                SettingsAction::Continue
            }
            KeyCode::Char('2') => {
                self.tab = SettingsTab::Permissions;
                self.rebuild_rows(config);
                SettingsAction::Continue
            }
            KeyCode::Char('3') => {
                self.tab = SettingsTab::Features;
                self.rebuild_rows(config);
                SettingsAction::Continue
            }
            KeyCode::Up => {
                if self.selected_row > 0 {
                    self.selected_row -= 1;
                    if self.selected_row < self.scroll_offset {
                        self.scroll_offset = self.selected_row;
                    }
                }
                SettingsAction::Continue
            }
            KeyCode::Down => {
                if self.selected_row + 1 < self.rows.len() {
                    self.selected_row += 1;
                }
                SettingsAction::Continue
            }
            KeyCode::Enter => {
                self.start_editing();
                SettingsAction::Continue
            }
            KeyCode::Char(' ') => {
                if let Some(row) = self.rows.get_mut(self.selected_row) {
                    if let SettingValue::Toggle(ref mut v) = row.value {
                        *v = !*v;
                        return SettingsAction::Apply(SettingsChange {
                            key: row.key.clone(),
                            value: v.to_string(),
                        });
                    }
                }
                SettingsAction::Continue
            }
            KeyCode::Esc => SettingsAction::Close,
            _ => SettingsAction::Continue,
        }
    }

    /// Switch to previous tab (wraps).
    const fn prev_tab(&mut self) {
        self.tab = match self.tab {
            SettingsTab::Api => SettingsTab::Features,
            SettingsTab::Permissions => SettingsTab::Api,
            SettingsTab::Features => SettingsTab::Permissions,
        };
    }

    /// Switch to next tab (wraps).
    const fn next_tab(&mut self) {
        self.tab = match self.tab {
            SettingsTab::Api => SettingsTab::Permissions,
            SettingsTab::Permissions => SettingsTab::Features,
            SettingsTab::Features => SettingsTab::Api,
        };
    }

    /// Start editing the currently selected row.
    fn start_editing(&mut self) {
        let Some(row) = self.rows.get(self.selected_row) else {
            return;
        };
        self.editing = match &row.value {
            SettingValue::Text(s) => Some(EditState::TextInput {
                buffer: s.clone(),
                cursor: s.len(),
                masked: false,
            }),
            SettingValue::Secret(s) => Some(EditState::TextInput {
                buffer: s.clone(),
                cursor: s.len(),
                masked: true,
            }),
            SettingValue::Select { options, selected } => Some(EditState::ListPicker {
                options: options.clone(),
                selected: *selected,
            }),
            SettingValue::Toggle(_) => None,
        };
    }
}

/// Handle key events when in edit mode.
fn handle_edit_key(key: KeyEvent, edit: &mut EditState, row: &SettingRow) -> SettingsAction {
    match edit {
        EditState::TextInput { buffer, cursor, .. } => match key.code {
            KeyCode::Char(c) => {
                buffer.insert(*cursor, c);
                *cursor += 1;
                SettingsAction::Continue
            }
            KeyCode::Backspace => {
                if *cursor > 0 {
                    *cursor -= 1;
                    buffer.remove(*cursor);
                }
                SettingsAction::Continue
            }
            KeyCode::Enter => {
                let change = SettingsChange {
                    key: row.key.clone(),
                    value: buffer.clone(),
                };
                SettingsAction::Apply(change)
            }
            _ => SettingsAction::Continue,
        },
        EditState::ListPicker { options, selected } => match key.code {
            KeyCode::Up => {
                if *selected > 0 {
                    *selected -= 1;
                }
                SettingsAction::Continue
            }
            KeyCode::Down => {
                if *selected + 1 < options.len() {
                    *selected += 1;
                }
                SettingsAction::Continue
            }
            KeyCode::Enter => {
                let value = options.get(*selected).cloned().unwrap_or_default();
                let change = SettingsChange {
                    key: row.key.clone(),
                    value,
                };
                SettingsAction::Apply(change)
            }
            _ => SettingsAction::Continue,
        },
    }
}

/// Mask an API key for display.
pub fn mask_secret(s: &str) -> String {
    if s.is_empty() {
        "(not set)".to_string()
    } else if s.starts_with('$') {
        s.to_string()
    } else if s.len() <= 7 {
        "\u{2022}".repeat(s.len())
    } else {
        format!("{}{}", "\u{2022}".repeat(s.len() - 7), &s[s.len() - 7..])
    }
}

/// Build rows for the API tab.
#[allow(clippy::too_many_lines)]
pub fn build_api_rows(config: &AppConfig) -> Vec<SettingRow> {
    let providers = vec![
        "anthropic".to_string(),
        "openai".to_string(),
        "gemini".to_string(),
        "openrouter".to_string(),
    ];
    let provider_idx = providers
        .iter()
        .position(|p| p == &config.provider.default)
        .unwrap_or(0);

    vec![
        SettingRow {
            key: "provider.default".to_string(),
            label: "Provider".to_string(),
            value: SettingValue::Select {
                options: providers,
                selected: provider_idx,
            },
            description: "Default LLM provider".to_string(),
        },
        SettingRow {
            key: "provider.anthropic.api_key".to_string(),
            label: "Anthropic API Key".to_string(),
            value: SettingValue::Secret(
                config
                    .provider
                    .anthropic
                    .api_key
                    .clone()
                    .or_else(|| {
                        config
                            .provider
                            .anthropic
                            .api_key_env
                            .as_ref()
                            .map(|e| format!("${e}"))
                    })
                    .unwrap_or_default(),
            ),
            description: "Direct key or $ENV_VAR".to_string(),
        },
        SettingRow {
            key: "provider.anthropic.model".to_string(),
            label: "Anthropic Model".to_string(),
            value: SettingValue::Select {
                options: vec![
                    "claude-sonnet-4-6".to_string(),
                    "claude-opus-4-6".to_string(),
                    "claude-haiku-4-5".to_string(),
                    "claude-sonnet-4-5".to_string(),
                ],
                selected: 0,
            },
            description: "Model ID".to_string(),
        },
        SettingRow {
            key: "provider.openai.api_key".to_string(),
            label: "OpenAI API Key".to_string(),
            value: SettingValue::Secret(
                config
                    .provider
                    .openai
                    .api_key
                    .clone()
                    .or_else(|| {
                        config
                            .provider
                            .openai
                            .api_key_env
                            .as_ref()
                            .map(|e| format!("${e}"))
                    })
                    .unwrap_or_default(),
            ),
            description: "Direct key or $ENV_VAR".to_string(),
        },
        SettingRow {
            key: "provider.openai.model".to_string(),
            label: "OpenAI Model".to_string(),
            value: SettingValue::Select {
                options: vec![
                    "gpt-5.4".to_string(),
                    "gpt-5.4-mini".to_string(),
                    "gpt-5.4-nano".to_string(),
                ],
                selected: 0,
            },
            description: "Model ID".to_string(),
        },
        SettingRow {
            key: "provider.gemini.api_key".to_string(),
            label: "Gemini API Key".to_string(),
            value: SettingValue::Secret(
                config
                    .provider
                    .gemini
                    .api_key
                    .clone()
                    .or_else(|| {
                        config
                            .provider
                            .gemini
                            .api_key_env
                            .as_ref()
                            .map(|e| format!("${e}"))
                    })
                    .unwrap_or_default(),
            ),
            description: "Direct key or $ENV_VAR".to_string(),
        },
        SettingRow {
            key: "provider.gemini.model".to_string(),
            label: "Gemini Model".to_string(),
            value: SettingValue::Select {
                options: vec![
                    "gemini-3.1-pro-preview".to_string(),
                    "gemini-3-flash-preview".to_string(),
                    "gemini-3.1-flash-lite-preview".to_string(),
                    "gemini-2.5-pro".to_string(),
                    "gemini-2.5-flash".to_string(),
                ],
                selected: 0,
            },
            description: "Model ID".to_string(),
        },
        SettingRow {
            key: "provider.openrouter.api_key".to_string(),
            label: "OpenRouter API Key".to_string(),
            value: SettingValue::Secret(
                config
                    .provider
                    .openrouter
                    .api_key
                    .clone()
                    .or_else(|| {
                        config
                            .provider
                            .openrouter
                            .api_key_env
                            .as_ref()
                            .map(|e| format!("${e}"))
                    })
                    .unwrap_or_default(),
            ),
            description: "Direct key or $ENV_VAR".to_string(),
        },
    ]
}

/// Build rows for the Permissions tab.
pub fn build_permissions_rows(config: &AppConfig) -> Vec<SettingRow> {
    let mode_idx = match config.permissions.mode.as_str() {
        "auto" => 1,
        "yolo" => 2,
        _ => 0,
    };
    vec![
        SettingRow {
            key: "permissions.mode".to_string(),
            label: "Permission Mode".to_string(),
            value: SettingValue::Select {
                options: vec!["ask".into(), "auto".into(), "yolo".into()],
                selected: mode_idx,
            },
            description: "How tool calls are approved".to_string(),
        },
        SettingRow {
            key: "permissions.auto_approve.read_files".to_string(),
            label: "Auto-approve file reads".to_string(),
            value: SettingValue::Toggle(config.permissions.auto_approve.read_files),
            description: "Skip approval for read_file, list_files, search_files".to_string(),
        },
        SettingRow {
            key: "permissions.auto_approve.edit_files".to_string(),
            label: "Auto-approve file edits".to_string(),
            value: SettingValue::Toggle(config.permissions.auto_approve.edit_files),
            description: "Skip approval for write_file, apply_patch".to_string(),
        },
        SettingRow {
            key: "permissions.auto_approve.execute_safe_commands".to_string(),
            label: "Auto-approve safe commands".to_string(),
            value: SettingValue::Toggle(config.permissions.auto_approve.execute_safe_commands),
            description: "Commands matching allow patterns".to_string(),
        },
        SettingRow {
            key: "permissions.auto_approve.execute_all_commands".to_string(),
            label: "Auto-approve ALL commands".to_string(),
            value: SettingValue::Toggle(config.permissions.auto_approve.execute_all_commands),
            description: "Dangerous — skips all command approval".to_string(),
        },
    ]
}

/// Build rows for the Features tab.
pub fn build_features_rows(config: &AppConfig) -> Vec<SettingRow> {
    let mode_idx = usize::from(config.mode.default == "plan");
    vec![
        SettingRow {
            key: "mode.default".to_string(),
            label: "Default Mode".to_string(),
            value: SettingValue::Select {
                options: vec!["act".into(), "plan".into()],
                selected: mode_idx,
            },
            description: "Starting mode for new tasks".to_string(),
        },
        SettingRow {
            key: "mode.strict_plan".to_string(),
            label: "Strict Plan Mode".to_string(),
            value: SettingValue::Toggle(config.mode.strict_plan),
            description: "Require plan approval before acting".to_string(),
        },
    ]
}

/// Render the settings panel into the given area.
pub fn render_settings(frame: &mut Frame, area: Rect, view: &SettingsView) {
    // Fill entire area with a solid background block first.
    // This overwrites EVERY cell, preventing stale content from previous
    // frames bleeding through ratatui's double-buffer diff.
    let bg = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::DarkGray))
        .style(Style::default().bg(Color::Black));
    frame.render_widget(bg, area);

    let inner = Block::default().borders(Borders::TOP).inner(area);
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(inner);

    let tab_titles = vec!["1:API", "2:Permissions", "3:Features"];
    let tab_idx = match view.tab {
        SettingsTab::Api => 0,
        SettingsTab::Permissions => 1,
        SettingsTab::Features => 2,
    };
    let tabs = Tabs::new(tab_titles)
        .select(tab_idx)
        .highlight_style(Style::default().bold().fg(Color::White).bg(Color::DarkGray))
        .style(Style::default().fg(Color::Gray).bg(Color::Black))
        .divider(" | ");
    frame.render_widget(tabs, chunks[0]);

    let max_visible = chunks[1].height.saturating_sub(2) as usize;
    let items: Vec<ListItem> = view
        .rows
        .iter()
        .enumerate()
        .skip(view.scroll_offset)
        .take(max_visible)
        .map(|(i, row)| {
            let is_selected = i == view.selected_row;
            let cursor = if is_selected { "\u{276f} " } else { "  " };
            let val = match &row.value {
                SettingValue::Text(s) => s.clone(),
                SettingValue::Secret(s) => mask_secret(s),
                SettingValue::Toggle(b) => if *b { "[x]" } else { "[ ]" }.to_string(),
                SettingValue::Select { options, selected } => {
                    options.get(*selected).cloned().unwrap_or_default()
                }
            };
            let line = format!("{cursor}{:<32} {val}", row.label);
            let style = if is_selected {
                Style::default().fg(Color::Cyan).bold().bg(Color::Black)
            } else {
                Style::default().fg(Color::Gray).bg(Color::Black)
            };
            ListItem::new(line).style(style)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(" Settings ")
                .style(Style::default().bg(Color::Black)),
        )
        .style(Style::default().bg(Color::Black));
    frame.render_widget(list, chunks[1]);

    let help = if view.editing.is_some() {
        "Enter: save  Esc: cancel"
    } else {
        "\u{2190}\u{2192}: tabs  \u{2191}\u{2193}: navigate  Enter: edit  Space: toggle  Esc: close"
    };
    let help_bar =
        Paragraph::new(help).style(Style::default().fg(Color::DarkGray).bg(Color::Black));
    frame.render_widget(help_bar, chunks[2]);

    if let Some(ref edit) = view.editing {
        render_edit_overlay(frame, area, edit, &view.rows[view.selected_row]);
    }
}

/// Render an edit overlay popup.
fn render_edit_overlay(frame: &mut Frame, area: Rect, edit: &EditState, row: &SettingRow) {
    let popup = centered_rect(50, 30, area);
    frame.render_widget(Clear, popup);

    match edit {
        EditState::TextInput { buffer, masked, .. } => {
            let display = if *masked {
                mask_secret(buffer)
            } else {
                buffer.clone()
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .title(format!(" Edit: {} ", row.label))
                .border_style(Style::default().fg(Color::Cyan));
            let text = Paragraph::new(vec![
                Line::from(row.description.clone()).style(Style::default().fg(Color::DarkGray)),
                Line::from(""),
                Line::from(display).style(Style::default().fg(Color::White)),
            ])
            .block(block);
            frame.render_widget(text, popup);
        }
        EditState::ListPicker { options, selected } => {
            let block = Block::default()
                .borders(Borders::ALL)
                .title(format!(" Select: {} ", row.label))
                .border_style(Style::default().fg(Color::Cyan));
            let mut lines = vec![
                Line::from(row.description.clone()).style(Style::default().fg(Color::DarkGray)),
            ];
            lines.push(Line::from(""));
            for (i, opt) in options.iter().enumerate().take(10) {
                let cursor = if i == *selected { "\u{276f} " } else { "  " };
                let style = if i == *selected {
                    Style::default().fg(Color::Cyan).bold()
                } else {
                    Style::default().fg(Color::Gray)
                };
                lines.push(Line::from(format!("{cursor}{opt}")).style(style));
            }
            let text = Paragraph::new(lines).block(block);
            frame.render_widget(text, popup);
        }
    }
}

/// Calculate a centered rectangle within the given area.
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_navigation_wraps() {
        let config = AppConfig::default();
        let mut view = SettingsView::new(&config);
        assert_eq!(view.tab, SettingsTab::Api);
        view.next_tab();
        assert_eq!(view.tab, SettingsTab::Permissions);
        view.next_tab();
        assert_eq!(view.tab, SettingsTab::Features);
        view.next_tab();
        assert_eq!(view.tab, SettingsTab::Api);
    }

    #[test]
    fn prev_tab_wraps() {
        let config = AppConfig::default();
        let mut view = SettingsView::new(&config);
        view.prev_tab();
        assert_eq!(view.tab, SettingsTab::Features);
    }

    #[test]
    fn row_navigation() {
        let config = AppConfig::default();
        let mut view = SettingsView::new(&config);
        assert_eq!(view.selected_row, 0);
        let key_down = KeyEvent::from(KeyCode::Down);
        view.handle_key(key_down, &config);
        assert_eq!(view.selected_row, 1);
        let key_up = KeyEvent::from(KeyCode::Up);
        view.handle_key(key_up, &config);
        assert_eq!(view.selected_row, 0);
        view.handle_key(key_up, &config);
        assert_eq!(view.selected_row, 0);
    }

    #[test]
    fn mask_secret_empty() {
        assert_eq!(mask_secret(""), "(not set)");
    }

    #[test]
    fn mask_secret_env_var() {
        assert_eq!(mask_secret("$ANTHROPIC_API_KEY"), "$ANTHROPIC_API_KEY");
    }

    #[test]
    fn mask_secret_long_key() {
        let masked = mask_secret("sk-ant-abc1234567");
        assert!(masked.contains("1234567"));
        assert!(masked.contains('\u{2022}'));
    }

    #[test]
    fn mask_secret_short_key() {
        let masked = mask_secret("short");
        assert_eq!(masked.chars().filter(|c| *c == '\u{2022}').count(), 5);
    }

    #[test]
    fn api_tab_has_provider_row() {
        let rows = build_api_rows(&AppConfig::default());
        assert!(rows.iter().any(|r| r.key == "provider.default"));
    }

    #[test]
    fn api_tab_has_key_rows() {
        let rows = build_api_rows(&AppConfig::default());
        assert!(rows.iter().any(|r| r.key.contains("api_key")));
    }

    #[test]
    fn permissions_tab_has_toggles() {
        let rows = build_permissions_rows(&AppConfig::default());
        assert!(
            rows.iter()
                .any(|r| matches!(r.value, SettingValue::Toggle(_)))
        );
    }

    #[test]
    fn features_tab_has_mode() {
        let rows = build_features_rows(&AppConfig::default());
        assert!(rows.iter().any(|r| r.key == "mode.default"));
    }

    #[test]
    fn esc_closes_settings() {
        let config = AppConfig::default();
        let mut view = SettingsView::new(&config);
        let action = view.handle_key(KeyEvent::from(KeyCode::Esc), &config);
        assert!(matches!(action, SettingsAction::Close));
    }

    #[test]
    fn space_toggles_boolean() {
        let config = AppConfig::default();
        let mut view = SettingsView::new(&config);
        view.tab = SettingsTab::Permissions;
        view.rebuild_rows(&config);
        let toggle_idx = view
            .rows
            .iter()
            .position(|r| matches!(r.value, SettingValue::Toggle(_)))
            .unwrap();
        view.selected_row = toggle_idx;
        let action = view.handle_key(KeyEvent::from(KeyCode::Char(' ')), &config);
        assert!(matches!(action, SettingsAction::Apply(_)));
    }
}
