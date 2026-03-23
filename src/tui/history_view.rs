//! Task history browser — list, select, and resume saved conversations.
//!
//! Renders a scrollable list of `TaskSummary` entries. The user navigates
//! with Up/Down, presses Enter to resume, or `d` to delete.

use crate::state::history::TaskSummary;

/// Task history view state.
pub struct HistoryView {
    tasks: Vec<TaskSummary>,
    selected: usize,
    scroll_offset: usize,
}

impl HistoryView {
    /// Create a new history view from a list of task summaries.
    pub const fn new(tasks: Vec<TaskSummary>) -> Self {
        Self {
            tasks,
            selected: 0,
            scroll_offset: 0,
        }
    }

    /// Returns the task ID of the currently selected entry.
    pub fn selected_task_id(&self) -> Option<&str> {
        self.tasks.get(self.selected).map(|t| t.task_id.as_str())
    }

    /// Move selection up by one.
    pub const fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            if self.selected < self.scroll_offset {
                self.scroll_offset = self.selected;
            }
        }
    }

    /// Move selection down by one.
    pub fn move_down(&mut self) {
        if self.selected + 1 < self.tasks.len() {
            self.selected += 1;
        }
    }

    /// Returns the list of tasks.
    pub fn tasks(&self) -> &[TaskSummary] {
        &self.tasks
    }

    /// Returns the currently selected index.
    pub const fn selected_index(&self) -> usize {
        self.selected
    }

    /// Returns the current scroll offset.
    pub const fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Returns `true` if there are no tasks.
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_summary(task_id: &str, title: &str) -> TaskSummary {
        TaskSummary {
            task_id: task_id.to_string(),
            title: title.to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4".to_string(),
            message_count: 5,
            total_input_tokens: 100,
            total_output_tokens: 50,
            total_cost: 0.001,
            completed: false,
        }
    }

    #[test]
    fn history_view_navigation() {
        let tasks = vec![make_summary("t-1", "Task 1"), make_summary("t-2", "Task 2")];
        let mut view = HistoryView::new(tasks);
        assert_eq!(view.selected_task_id(), Some("t-1"));
        view.move_down();
        assert_eq!(view.selected_task_id(), Some("t-2"));
        view.move_down();
        assert_eq!(view.selected_task_id(), Some("t-2"));
        view.move_up();
        assert_eq!(view.selected_task_id(), Some("t-1"));
        view.move_up();
        assert_eq!(view.selected_task_id(), Some("t-1"));
    }

    #[test]
    fn history_view_empty() {
        let view = HistoryView::new(vec![]);
        assert_eq!(view.selected_task_id(), None);
        assert!(view.is_empty());
    }

    #[test]
    fn history_view_selected_index() {
        let tasks = vec![
            make_summary("t-1", "Task 1"),
            make_summary("t-2", "Task 2"),
            make_summary("t-3", "Task 3"),
        ];
        let mut view = HistoryView::new(tasks);
        assert_eq!(view.selected_index(), 0);
        view.move_down();
        view.move_down();
        assert_eq!(view.selected_index(), 2);
    }

    #[test]
    fn history_view_scroll_offset() {
        let tasks = vec![make_summary("t-1", "Task 1"), make_summary("t-2", "Task 2")];
        let view = HistoryView::new(tasks);
        assert_eq!(view.scroll_offset(), 0);
    }

    #[test]
    fn history_view_tasks_accessor() {
        let tasks = vec![make_summary("t-1", "Task 1")];
        let view = HistoryView::new(tasks);
        assert_eq!(view.tasks().len(), 1);
        assert_eq!(view.tasks()[0].task_id, "t-1");
    }
}
