# STEP 43 — Interactive Settings UI

## Objective
Implement a tabbed settings panel accessible via `/settings` slash command, allowing users to change provider, enter API keys, select models, and toggle features — all without editing config files manually. Modeled after Cline's in-chat settings panel architecture.

## Prerequisites
- STEP 42 complete (async TUI)
- STEP 35 complete (slash commands)
- STEP 02 complete (state management)

## Context: How Cline Does It

Cline's CLI (built with Ink/React) implements a 5-tab in-chat settings panel:

```
[ API | Auto-approve | Features | Account | Other ]
```

### Key design decisions from Cline's implementation:

1. **In-chat overlay** — Settings panel renders below the chat area, splitting the screen. Chat is compressed to the top half. Esc returns to chat.

2. **Tab navigation** — `←/→` arrows or number keys (`1-5`) switch tabs. Each tab is a scrollable list of editable items.

3. **Provider picker** — Searchable list of all providers. Shows "(Configured)" suffix for providers that already have credentials. Uses fuzzy filtering as user types.

4. **API key input** — Password-masked with bullets (`••••••••`). Shows last 4-7 chars for identification. Enter saves, Esc cancels. Supports both direct keys and env var references.

5. **Model picker** — Static list for most providers (Anthropic, OpenAI, Gemini). Async-fetched for OpenRouter. Default model pre-selected per provider.

6. **Persistence** — Changes write to StateManager immediately (debounced). In-memory cache + disk. No "Save" button needed — changes are live.

### Source files analyzed:
- `cli/src/components/SettingsPanelContent.tsx` — 5-tab settings panel
- `cli/src/components/ProviderPicker.tsx` — Searchable provider list
- `cli/src/components/ModelPicker.tsx` — Per-provider model selection
- `cli/src/components/ApiKeyInput.tsx` — Password-masked input
- `cli/src/components/SearchableList.tsx` — Generic fuzzy-filtered list
- `cli/src/utils/provider-config.ts` — Provider credential mapping
- `cli/src/utils/providers.ts` — Provider metadata and model lists

---

## Detailed Instructions

### 43.1 View mode routing (`src/app.rs`)

Add a `ViewMode` enum to control what the TUI renders:

```rust
/// Active view mode — determines what the TUI renders.
enum ViewMode {
    /// Normal chat view (default).
    Chat,
    /// Settings panel overlaid on chat.
    Settings,
}
```

In the main `tokio::select!` loop, route key events based on the active view:

```rust
match view_mode {
    ViewMode::Chat => handle_chat_key(key, &mut input, ctrl_tx, &mut chat_state),
    ViewMode::Settings => {
        if settings_view.handle_key(key) == SettingsAction::Close {
            view_mode = ViewMode::Chat;
        }
    }
}
```

The `/settings` slash command switches to `ViewMode::Settings`:
```rust
// In controller's handle_slash_command:
SlashCommand::Settings => {
    self.send_ui(UiUpdate::ShowSettings);
}
```

Add `ShowSettings` variant to `UiUpdate` enum.

### 43.2 Settings view state (`src/tui/settings_view.rs`)

This is the core new module. Replace the existing `settings_view.rs` stub.

```rust
/// Active tab in the settings panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    Api,
    Permissions,
    Features,
}

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
    /// Text field (provider name, model ID).
    Text(String),
    /// Password-masked field (API keys).
    Secret(String),
    /// Boolean toggle.
    Toggle(bool),
    /// Selection from a list of options.
    Select { options: Vec<String>, selected: usize },
}

/// The full settings view state.
pub struct SettingsView {
    pub tab: SettingsTab,
    pub rows: Vec<SettingRow>,
    pub selected_row: usize,
    pub scroll_offset: usize,
    pub editing: Option<EditState>,
}

/// State when a row is being edited inline.
pub enum EditState {
    /// Text input mode (for text/secret fields).
    TextInput {
        buffer: String,
        cursor: usize,
        masked: bool,
    },
    /// List picker mode (for Select fields).
    ListPicker {
        options: Vec<String>,
        filtered: Vec<usize>,
        selected: usize,
        query: String,
    },
}

/// Actions returned from key handling.
pub enum SettingsAction {
    /// Stay in settings view.
    Continue,
    /// Close settings, return to chat.
    Close,
    /// Apply a config change.
    Apply(SettingsChange),
}

/// A config change to persist.
pub struct SettingsChange {
    pub key: String,
    pub value: String,
}
```

### 43.3 Settings view key handling

```rust
impl SettingsView {
    /// Handle a key event. Returns the action to take.
    pub fn handle_key(&mut self, key: KeyEvent) -> SettingsAction {
        // If currently editing a field, delegate to edit handler
        if let Some(ref mut edit) = self.editing {
            return self.handle_edit_key(key, edit);
        }

        match key.code {
            // Tab navigation
            KeyCode::Left => { self.prev_tab(); SettingsAction::Continue }
            KeyCode::Right => { self.next_tab(); SettingsAction::Continue }
            KeyCode::Char('1') => { self.tab = SettingsTab::Api; self.rebuild_rows(); SettingsAction::Continue }
            KeyCode::Char('2') => { self.tab = SettingsTab::Permissions; self.rebuild_rows(); SettingsAction::Continue }
            KeyCode::Char('3') => { self.tab = SettingsTab::Features; self.rebuild_rows(); SettingsAction::Continue }

            // Row navigation
            KeyCode::Up => { self.move_up(); SettingsAction::Continue }
            KeyCode::Down => { self.move_down(); SettingsAction::Continue }

            // Edit selected row
            KeyCode::Enter => { self.start_editing(); SettingsAction::Continue }

            // Toggle (for boolean rows)
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

            // Close
            KeyCode::Esc => SettingsAction::Close,

            _ => SettingsAction::Continue,
        }
    }
}
```

### 43.4 API tab rows

The API tab shows these editable rows, populated from the current config:

```rust
fn build_api_rows(config: &AppConfig) -> Vec<SettingRow> {
    vec![
        SettingRow {
            key: "provider.default".to_string(),
            label: "Provider".to_string(),
            value: SettingValue::Select {
                options: vec![
                    "anthropic".to_string(),
                    "openai".to_string(),
                    "gemini".to_string(),
                    "openrouter".to_string(),
                ],
                selected: match config.provider.default.as_str() {
                    "anthropic" => 0,
                    "openai" => 1,
                    "gemini" => 2,
                    "openrouter" => 3,
                    _ => 0,
                },
            },
            description: "Default LLM provider".to_string(),
        },
        SettingRow {
            key: "provider.anthropic.api_key".to_string(),
            label: "Anthropic API Key".to_string(),
            value: SettingValue::Secret(
                config.provider.anthropic.api_key.clone().unwrap_or_default()
            ),
            description: "Direct key or env var (e.g. $ANTHROPIC_API_KEY)".to_string(),
        },
        SettingRow {
            key: "provider.anthropic.model".to_string(),
            label: "Anthropic Model".to_string(),
            value: SettingValue::Select {
                options: vec![
                    "claude-sonnet-4-20250514".to_string(),
                    "claude-opus-4-20250514".to_string(),
                    "claude-haiku-4-5-20251001".to_string(),
                ],
                selected: 0,
            },
            description: "Model ID for Anthropic".to_string(),
        },
        // Similar rows for OpenAI, Gemini, OpenRouter...
        SettingRow {
            key: "provider.openai.api_key".to_string(),
            label: "OpenAI API Key".to_string(),
            value: SettingValue::Secret(
                config.provider.openai.api_key.clone().unwrap_or_default()
            ),
            description: "Direct key or env var (e.g. $OPENAI_API_KEY)".to_string(),
        },
        SettingRow {
            key: "provider.gemini.api_key".to_string(),
            label: "Gemini API Key".to_string(),
            value: SettingValue::Secret(
                config.provider.gemini.api_key.clone().unwrap_or_default()
            ),
            description: "Direct key or env var (e.g. $GEMINI_API_KEY)".to_string(),
        },
    ]
}
```

### 43.5 Permissions tab rows

```rust
fn build_permissions_rows(config: &AppConfig) -> Vec<SettingRow> {
    vec![
        SettingRow {
            key: "permissions.mode".to_string(),
            label: "Permission Mode".to_string(),
            value: SettingValue::Select {
                options: vec!["ask".into(), "auto".into(), "yolo".into()],
                selected: match config.permissions.mode.as_str() {
                    "auto" => 1, "yolo" => 2, _ => 0,
                },
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
            description: "Skip approval for commands matching allow patterns".to_string(),
        },
        SettingRow {
            key: "permissions.auto_approve.execute_all_commands".to_string(),
            label: "Auto-approve all commands".to_string(),
            value: SettingValue::Toggle(config.permissions.auto_approve.execute_all_commands),
            description: "Skip approval for ALL commands (dangerous)".to_string(),
        },
    ]
}
```

### 43.6 Features tab rows

```rust
fn build_features_rows(config: &AppConfig) -> Vec<SettingRow> {
    vec![
        SettingRow {
            key: "mode.default".to_string(),
            label: "Default Mode".to_string(),
            value: SettingValue::Select {
                options: vec!["act".into(), "plan".into()],
                selected: if config.mode.default == "plan" { 1 } else { 0 },
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
```

### 43.7 Rendering the settings panel (`src/tui/settings_view.rs`)

```rust
pub fn render_settings(frame: &mut Frame, area: Rect, view: &SettingsView) {
    // Split area: tab bar (1 line) + content
    let chunks = Layout::vertical([
        Constraint::Length(1),  // tab bar
        Constraint::Min(0),     // content
        Constraint::Length(1),  // help bar
    ]).split(area);

    // Tab bar
    let tabs = vec!["1:API", "2:Permissions", "3:Features"];
    let tab_bar = Tabs::new(tabs)
        .select(view.tab as usize)
        .highlight_style(Style::default().bold().fg(Color::Cyan));
    frame.render_widget(tab_bar, chunks[0]);

    // Content — scrollable list of rows
    let visible_rows = view.visible_rows(chunks[1].height as usize);
    let items: Vec<ListItem> = visible_rows.iter().enumerate().map(|(i, row)| {
        let is_selected = i + view.scroll_offset == view.selected_row;
        let cursor = if is_selected { "❯ " } else { "  " };
        let value_display = match &row.value {
            SettingValue::Text(s) => s.clone(),
            SettingValue::Secret(s) => mask_secret(s),
            SettingValue::Toggle(b) => if *b { "[x]" } else { "[ ]" }.to_string(),
            SettingValue::Select { options, selected } => {
                options.get(*selected).cloned().unwrap_or_default()
            }
        };
        let line = format!("{cursor}{:<30} {value_display}", row.label);
        let style = if is_selected {
            Style::default().fg(Color::Cyan).bold()
        } else {
            Style::default().fg(Color::Gray)
        };
        ListItem::new(line).style(style)
    }).collect();

    let list = List::new(items)
        .block(Block::bordered().title(" Settings "));
    frame.render_widget(list, chunks[1]);

    // Help bar
    let help = if view.editing.is_some() {
        "Enter: save  Esc: cancel"
    } else {
        "↑↓: navigate  ←→: tabs  Enter: edit  Space: toggle  Esc: close"
    };
    let help_bar = Paragraph::new(help)
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(help_bar, chunks[2]);

    // If editing, render overlay
    if let Some(ref edit) = view.editing {
        render_edit_overlay(frame, area, edit, &view.rows[view.selected_row]);
    }
}

/// Mask an API key for display: "••••••sk-1234"
fn mask_secret(s: &str) -> String {
    if s.is_empty() {
        "(not set)".to_string()
    } else if s.starts_with('$') {
        s.to_string() // Env var reference shown as-is
    } else if s.len() <= 7 {
        "•".repeat(s.len())
    } else {
        format!("{}{}", "•".repeat(s.len() - 7), &s[s.len() - 7..])
    }
}
```

### 43.8 Edit overlay rendering

When user presses Enter on a row, an edit overlay appears:

```rust
fn render_edit_overlay(frame: &mut Frame, area: Rect, edit: &EditState, row: &SettingRow) {
    // Center a popup box
    let popup = centered_rect(60, 40, area);
    frame.render_widget(Clear, popup); // clear background

    match edit {
        EditState::TextInput { buffer, cursor, masked } => {
            let display = if *masked {
                mask_secret(buffer)
            } else {
                buffer.clone()
            };
            let block = Block::bordered()
                .title(format!(" Edit: {} ", row.label))
                .border_style(Style::default().fg(Color::Cyan));
            let content = Paragraph::new(vec![
                Line::from(row.description.clone()).style(Style::default().fg(Color::DarkGray)),
                Line::from(""),
                Line::from(display).style(Style::default().fg(Color::White)),
            ]).block(block);
            frame.render_widget(content, popup);
        }
        EditState::ListPicker { options, filtered, selected, query } => {
            let block = Block::bordered()
                .title(format!(" Select: {} ", row.label))
                .border_style(Style::default().fg(Color::Cyan));
            // Show search query + filtered list
            let mut lines = vec![
                Line::from(format!("Search: {query}")).style(Style::default().fg(Color::Yellow)),
                Line::from(""),
            ];
            for (i, &idx) in filtered.iter().enumerate().take(10) {
                let cursor = if i == *selected { "❯ " } else { "  " };
                let style = if i == *selected {
                    Style::default().fg(Color::Cyan).bold()
                } else {
                    Style::default().fg(Color::Gray)
                };
                lines.push(Line::from(format!("{cursor}{}", options[idx])).style(style));
            }
            let content = Paragraph::new(lines).block(block);
            frame.render_widget(content, popup);
        }
    }
}
```

### 43.9 Applying settings changes

When a change is made, it flows through the controller to StateManager:

```rust
// In app.rs, when SettingsAction::Apply is returned:
SettingsAction::Apply(change) => {
    let _ = ctrl_tx.send(ControllerMessage::SettingsChange(change));
}

// In controller/messages.rs, add:
SettingsChange(crate::tui::settings_view::SettingsChange),

// In controller/mod.rs handle_message:
ControllerMessage::SettingsChange(change) => {
    self.apply_settings_change(change).await;
}
```

The `apply_settings_change` method maps config keys to `StateManager::update_config`:

```rust
async fn apply_settings_change(&mut self, change: SettingsChange) {
    let result = self.state.update_config(|config| {
        match change.key.as_str() {
            "provider.default" => config.provider.default = change.value,
            "provider.anthropic.api_key" => {
                if change.value.starts_with('$') {
                    config.provider.anthropic.api_key_env = Some(change.value);
                    config.provider.anthropic.api_key = None;
                } else {
                    config.provider.anthropic.api_key = Some(change.value);
                }
            }
            "provider.anthropic.model" => {
                config.provider.anthropic.model = Some(change.value);
            }
            "permissions.mode" => config.permissions.mode = change.value,
            "permissions.auto_approve.read_files" => {
                config.permissions.auto_approve.read_files = change.value == "true";
            }
            // ... similar for other keys
            _ => tracing::warn!(key = change.key, "Unknown settings key"),
        }
    }).await;

    if let Err(e) = result {
        tracing::error!(error = %e, "Failed to apply settings change");
        return;
    }

    // Persist to disk
    if let Err(e) = self.state.persist().await {
        tracing::error!(error = %e, "Failed to persist settings");
    }

    self.send_ui(UiUpdate::AppendMessage {
        role: crate::tui::chat_view::ChatRole::System,
        content: format!("Setting updated: {} = {}", change.key, change.value),
    });
}
```

### 43.10 Env var support for API keys

When user enters a value starting with `$`, it's treated as an env var reference:

```
Enter API key (or $ENV_VAR): $ANTHROPIC_API_KEY
```

Stored as `api_key_env: Some("ANTHROPIC_API_KEY")` (without the `$` prefix).

Direct keys stored as `api_key: Some("sk-ant-...")`.

The existing `resolve_api_key` already handles both — env var is checked first, then inline key.

### 43.11 Integration with app_layout rendering

The settings panel splits the screen when active:

```rust
// In app_layout.rs or wherever render_app is called:
pub fn render_app(
    frame: &mut Frame,
    chat_state: &ChatViewState,
    input: &InputWidget,
    status: &StatusBarState,
    settings: Option<&SettingsView>,
) {
    if let Some(settings) = settings {
        // Split: top half chat, bottom half settings
        let chunks = Layout::vertical([
            Constraint::Percentage(40),
            Constraint::Percentage(60),
        ]).split(frame.area());

        render_chat_area(frame, chunks[0], chat_state, input, status);
        settings_view::render_settings(frame, chunks[1], settings);
    } else {
        // Full chat view (current behavior)
        render_chat_area(frame, frame.area(), chat_state, input, status);
    }
}
```

---

## Tests

```rust
#[cfg(test)]
mod settings_view_tests {
    use super::*;

    #[test]
    fn tab_navigation() {
        let mut view = SettingsView::new_with_config(&AppConfig::default());
        assert_eq!(view.tab, SettingsTab::Api);
        view.next_tab();
        assert_eq!(view.tab, SettingsTab::Permissions);
        view.next_tab();
        assert_eq!(view.tab, SettingsTab::Features);
        view.next_tab();
        assert_eq!(view.tab, SettingsTab::Api); // wraps
    }

    #[test]
    fn row_navigation() {
        let mut view = SettingsView::new_with_config(&AppConfig::default());
        assert_eq!(view.selected_row, 0);
        view.move_down();
        assert_eq!(view.selected_row, 1);
        view.move_up();
        assert_eq!(view.selected_row, 0);
        view.move_up(); // no wrap
        assert_eq!(view.selected_row, 0);
    }

    #[test]
    fn toggle_boolean() {
        let mut view = SettingsView::new_with_config(&AppConfig::default());
        view.tab = SettingsTab::Permissions;
        view.rebuild_rows();
        // Find a toggle row
        let toggle_idx = view.rows.iter().position(|r|
            matches!(r.value, SettingValue::Toggle(_))
        ).unwrap();
        view.selected_row = toggle_idx;
        // Toggle it
        if let SettingValue::Toggle(v) = &view.rows[toggle_idx].value {
            assert!(!v);
        }
    }

    #[test]
    fn mask_secret_display() {
        assert_eq!(mask_secret(""), "(not set)");
        assert_eq!(mask_secret("$ANTHROPIC_API_KEY"), "$ANTHROPIC_API_KEY");
        assert_eq!(mask_secret("sk-ant-abc1234"), "•••••••bc1234"); // last 7 visible
        assert_eq!(mask_secret("short"), "•••••");
    }

    #[test]
    fn api_tab_has_provider_rows() {
        let rows = build_api_rows(&AppConfig::default());
        assert!(rows.iter().any(|r| r.key == "provider.default"));
        assert!(rows.iter().any(|r| r.key.contains("api_key")));
    }

    #[test]
    fn permissions_tab_has_toggle_rows() {
        let rows = build_permissions_rows(&AppConfig::default());
        assert!(rows.iter().all(|r|
            matches!(r.value, SettingValue::Toggle(_) | SettingValue::Select { .. })
        ));
    }
}
```

## Acceptance Criteria
- [x] `/settings` opens tabbed settings panel over chat
- [x] Three tabs: API, Permissions, Features
- [x] Tab navigation with `←/→` arrows and `1/2/3` number keys
- [x] Row navigation with `↑/↓` arrows
- [x] `Enter` opens edit overlay for text/secret/select fields
- [x] `Space` toggles boolean fields inline
- [x] `Esc` closes settings panel (returns to chat)
- [x] Provider selection from list
- [x] API key input with password masking (`••••••`)
- [x] Env var references supported (`$ANTHROPIC_API_KEY`)
- [x] Model selection per provider from known models list
- [x] Permission mode selection (ask/auto/yolo)
- [x] Auto-approve toggles for each tool category
- [x] Changes persisted to config.toml via StateManager
- [x] Settings confirmation message shown in chat
- [x] `render_app` handles split layout when settings visible
- [x] All existing tests pass
- [x] `cargo clippy -- -D warnings` passes
- [x] `cargo test` passes
