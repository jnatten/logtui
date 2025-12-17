use ratatui::widgets::ListState;
use regex::Regex;
use serde_json::Value;

use crate::model::LogEntry;

use super::{
    columns::{ColumnDef, default_columns, is_reserved_column},
    field_view::{FieldViewState, FieldZoom, collect_fields},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    List,
    Detail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    FilterInput,
    ColumnSelect,
    FieldView,
}

#[derive(Clone, Copy)]
enum SelectStrategy {
    PreserveOrFirst,
    Last,
}

pub struct App {
    pub entries: Vec<LogEntry>,
    pub filtered_indices: Vec<usize>,
    pub columns: Vec<ColumnDef>,
    pub column_select_state: ListState,
    pub list_state: ListState,
    pub input_paused: bool,
    pub list_scroll_offset: usize,
    pub max_entries: usize,
    pub last_list_height: usize,
    pub last_list_width: usize,
    pub last_detail_height: usize,
    pub last_detail_width: usize,
    pub detail_scroll: u16,
    pub detail_total_lines: usize,
    pub detail_horiz_offset: usize,
    pub detail_max_line_width: usize,
    pub focus: Focus,
    pub show_help: bool,
    pub zoom: Option<Focus>,
    pub autoscroll: bool,
    pub filter_query: String,
    pub filter_regex: Option<Regex>,
    pub filter_error: Option<String>,
    pub input_mode: InputMode,
    pub filter_buffer: String,
    pub force_redraw: bool,
    pub max_row_width: usize,
    pub horiz_offset: usize,
    pub field_view: Option<FieldViewState>,
    pub field_detail_scroll: u16,
    pub field_detail_total_lines: usize,
    pub last_field_detail_height: usize,
    pub last_field_list_height: usize,
    pub last_field_detail_width: usize,
    pub field_detail_horiz_offset: usize,
    pub field_detail_max_line_width: usize,
    pub detail_wrap: bool,
    pub field_detail_wrap: bool,
    pub field_zoom: Option<FieldZoom>,
}

impl App {
    pub fn new(max_entries: usize) -> Self {
        let mut list_state = ListState::default();
        list_state.select(None);
        let mut column_select_state = ListState::default();
        column_select_state.select(Some(0));
        Self {
            entries: Vec::new(),
            filtered_indices: Vec::new(),
            columns: default_columns(),
            column_select_state,
            list_state,
            input_paused: false,
            list_scroll_offset: 0,
            max_entries,
            last_list_height: 0,
            last_list_width: 0,
            last_detail_height: 0,
            last_detail_width: 0,
            detail_scroll: 0,
            detail_total_lines: 0,
            detail_horiz_offset: 0,
            detail_max_line_width: 0,
            focus: Focus::List,
            show_help: false,
            zoom: None,
            autoscroll: true,
            filter_query: String::new(),
            filter_regex: None,
            filter_error: None,
            input_mode: InputMode::Normal,
            filter_buffer: String::new(),
            force_redraw: true,
            max_row_width: 0,
            horiz_offset: 0,
            field_view: None,
            field_detail_scroll: 0,
            field_detail_total_lines: 0,
            last_field_detail_height: 0,
            last_field_list_height: 0,
            last_field_detail_width: 0,
            field_detail_horiz_offset: 0,
            field_detail_max_line_width: 0,
            detail_wrap: true,
            field_detail_wrap: true,
            field_zoom: None,
        }
    }

    pub fn ingest(&mut self, entry: LogEntry) {
        if self.input_paused {
            return;
        }
        self.push(entry);
    }

    pub fn toggle_input_pause(&mut self) {
        self.input_paused = !self.input_paused;
        self.force_redraw = true;
    }

    pub fn push(&mut self, entry: LogEntry) {
        if self.entries.len() == self.max_entries {
            self.entries.remove(0);
            if let Some(sel) = self.list_state.selected() {
                if sel > 0 {
                    self.list_state.select(Some(sel - 1));
                } else {
                    self.list_state.select(Some(0));
                }
            }
            self.filtered_indices = self
                .filtered_indices
                .iter()
                .filter_map(|idx| idx.checked_sub(1))
                .collect();
            self.update_list_offset();
        }
        self.discover_columns(&entry.raw);
        self.entries.push(entry);
        let new_idx = self.entries.len().saturating_sub(1);
        if !self.autoscroll {
            if self.matches_filter(self.entries.last().expect("just pushed entry")) {
                self.filtered_indices.push(new_idx);
                if self.list_state.selected().is_none() {
                    self.list_state
                        .select(Some(self.filtered_indices.len().saturating_sub(1)));
                    self.reset_detail_position();
                    self.horiz_offset = 0;
                    self.update_list_offset();
                }
            }
        } else {
            let strategy = SelectStrategy::Last;
            self.rebuild_filtered(Some(strategy), false);
            self.horiz_offset = 0;
        }
    }

    pub fn next(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let next = (i + 1).min(self.filtered_indices.len() - 1);
        self.list_state.select(Some(next));
        self.reset_detail_position();
        self.force_redraw = true;
        self.update_list_offset();
    }

    pub fn previous(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let prev = i.saturating_sub(1);
        self.list_state.select(Some(prev));
        self.reset_detail_position();
        self.force_redraw = true;
        self.update_list_offset();
    }

    pub fn page_down(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let half = (self.last_list_height.max(1) / 2).max(1);
        let i = self.list_state.selected().unwrap_or(0);
        let next = (i + half).min(self.filtered_indices.len() - 1);
        self.list_state.select(Some(next));
        self.reset_detail_position();
        self.force_redraw = true;
        self.update_list_offset();
    }

    pub fn page_up(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let half = (self.last_list_height.max(1) / 2).max(1);
        let i = self.list_state.selected().unwrap_or(0);
        let prev = i.saturating_sub(half);
        self.list_state.select(Some(prev));
        self.reset_detail_position();
        self.force_redraw = true;
        self.update_list_offset();
    }

    pub fn select_last(&mut self) {
        if self.filtered_indices.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state
                .select(Some(self.filtered_indices.len() - 1));
        }
        self.reset_detail_position();
        self.force_redraw = true;
        self.update_list_offset();
    }

    pub fn select_first(&mut self) {
        if self.filtered_indices.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(0));
        }
        self.reset_detail_position();
        self.force_redraw = true;
        self.update_list_offset();
    }

    pub fn current_entry(&self) -> Option<LogEntry> {
        let idx = self.list_state.selected()?;
        let entry_idx = *self.filtered_indices.get(idx)?;
        self.entries.get(entry_idx).cloned()
    }

    pub fn detail_down(&mut self, lines: usize) {
        if self.detail_total_lines == 0 {
            return;
        }
        let max_offset = self
            .detail_total_lines
            .saturating_sub(self.last_detail_height.max(1));
        let new = (self.detail_scroll as usize + lines).min(max_offset);
        self.detail_scroll = new as u16;
    }

    pub fn detail_up(&mut self, lines: usize) {
        let new = self.detail_scroll.saturating_sub(lines as u16);
        self.detail_scroll = new;
    }

    pub fn detail_top(&mut self) {
        self.detail_scroll = 0;
        self.detail_horiz_offset = 0;
    }

    pub fn detail_bottom(&mut self) {
        if self.detail_total_lines == 0 {
            self.detail_scroll = 0;
            return;
        }
        let max_offset = self
            .detail_total_lines
            .saturating_sub(self.last_detail_height.max(1));
        self.detail_scroll = max_offset as u16;
    }

    pub(crate) fn reset_detail_position(&mut self) {
        self.detail_scroll = 0;
        self.detail_horiz_offset = 0;
    }

    pub(crate) fn reset_field_detail_position(&mut self) {
        self.field_detail_scroll = 0;
        self.field_detail_horiz_offset = 0;
    }

    pub(crate) fn clamp_detail_horiz_offset(&mut self) {
        if self.detail_wrap {
            self.detail_horiz_offset = 0;
            return;
        }
        let max_off = self
            .detail_max_line_width
            .saturating_sub(self.last_detail_width.max(1));
        self.detail_horiz_offset = self.detail_horiz_offset.min(max_off);
    }

    pub(crate) fn clamp_field_detail_horiz_offset(&mut self) {
        if self.field_detail_wrap {
            self.field_detail_horiz_offset = 0;
            return;
        }
        let max_off = self
            .field_detail_max_line_width
            .saturating_sub(self.last_field_detail_width.max(1));
        self.field_detail_horiz_offset = self.field_detail_horiz_offset.min(max_off);
    }

    pub(crate) fn clamp_offset(&mut self) {
        if self.max_row_width > self.last_list_width {
            let max_off = self.max_row_width.saturating_sub(self.last_list_width);
            self.horiz_offset = self.horiz_offset.min(max_off);
        } else {
            self.horiz_offset = 0;
        }
    }

    pub fn move_column(&mut self, delta: isize) {
        if self.columns.is_empty() {
            return;
        }
        let Some(idx) = self.column_select_state.selected() else {
            return;
        };
        let len = self.columns.len();
        let new_idx = (idx as isize + delta).clamp(0, (len as isize) - 1) as usize;
        if new_idx == idx {
            return;
        }
        self.columns.swap(idx, new_idx);
        self.column_select_state.select(Some(new_idx));
    }

    pub fn apply_filter(&mut self, pattern: &str) {
        if pattern.is_empty() {
            self.filter_query.clear();
            self.filter_regex = None;
            self.filter_error = None;
            self.rebuild_filtered(Some(SelectStrategy::PreserveOrFirst), false);
            return;
        }

        match Regex::new(pattern) {
            Ok(re) => {
                self.filter_query = pattern.to_string();
                self.filter_regex = Some(re);
                self.filter_error = None;
                self.rebuild_filtered(Some(SelectStrategy::PreserveOrFirst), false);
            }
            Err(err) => {
                self.filter_error = Some(err.to_string());
            }
        }
    }

    pub fn enter_field_view(&mut self) {
        let Some(entry) = self.current_entry() else {
            return;
        };
        let fields = collect_fields(&entry.raw);
        if fields.is_empty() {
            return;
        }
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        let filtered_indices = (0..fields.len()).collect();
        self.field_view = Some(FieldViewState {
            fields,
            filtered_indices,
            list_state,
            filter: String::new(),
        });
        self.reset_field_detail_position();
        self.field_detail_total_lines = 0;
        self.last_field_detail_height = 0;
        self.last_field_detail_width = 0;
        self.field_detail_max_line_width = 0;
        self.field_zoom = None;
        self.input_mode = InputMode::FieldView;
        self.force_redraw = true;
    }

    pub fn exit_field_view(&mut self) {
        self.field_view = None;
        self.reset_field_detail_position();
        self.field_detail_total_lines = 0;
        self.last_field_detail_height = 0;
        self.last_field_detail_width = 0;
        self.field_detail_max_line_width = 0;
        self.field_zoom = None;
        self.input_mode = InputMode::Normal;
    }

    pub fn toggle_autoscroll(&mut self) {
        self.autoscroll = !self.autoscroll;
        self.force_redraw = true;
        if self.autoscroll {
            self.select_last();
        } else {
            self.list_scroll_offset = self.list_state.offset();
            *self.list_state.offset_mut() = self.list_scroll_offset;
        }
    }

    fn matches_filter(&self, entry: &LogEntry) -> bool {
        if let Some(re) = &self.filter_regex {
            let hay = format!(
                "{} {} {} {}",
                entry.timestamp, entry.level, entry.message, entry.raw
            );
            re.is_match(&hay)
        } else {
            true
        }
    }

    fn selected_entry_index(&self) -> Option<usize> {
        let idx = self.list_state.selected()?;
        self.filtered_indices.get(idx).copied()
    }

    fn update_list_offset(&mut self) {
        if self.autoscroll {
            self.list_scroll_offset = self.list_state.offset();
            return;
        }
        let Some(selected) = self.list_state.selected() else {
            self.list_scroll_offset = self.list_state.offset();
            return;
        };
        let height = self.last_list_height.max(1);
        let offset = self.list_scroll_offset;
        let new_offset = if selected >= offset + height {
            selected + 1 - height
        } else if selected < offset {
            selected
        } else {
            offset
        };
        self.list_scroll_offset = new_offset;
        *self.list_state.offset_mut() = new_offset;
    }

    fn rebuild_filtered(&mut self, strategy: Option<SelectStrategy>, preserve_view: bool) {
        let prev_selected_entry = self.selected_entry_index();

        let mut filtered = Vec::with_capacity(self.entries.len());
        for (idx, entry) in self.entries.iter().enumerate() {
            if self.matches_filter(entry) {
                filtered.push(idx);
            }
        }

        self.filtered_indices = filtered;

        if self.filtered_indices.is_empty() {
            self.list_state.select(None);
            self.reset_detail_position();
            return;
        }

        if preserve_view
            && let Some(prev_entry_idx) = prev_selected_entry
            && let Some(new_pos) = self
                .filtered_indices
                .iter()
                .position(|&idx| idx == prev_entry_idx)
        {
            self.list_state.select(Some(new_pos));
            return;
        }

        match strategy.unwrap_or(SelectStrategy::PreserveOrFirst) {
            SelectStrategy::Last => {
                self.list_state
                    .select(Some(self.filtered_indices.len().saturating_sub(1)));
                self.reset_detail_position();
                self.horiz_offset = 0;
                self.update_list_offset();
            }
            SelectStrategy::PreserveOrFirst => {
                if let Some(prev_entry_idx) = prev_selected_entry
                    && let Some(new_pos) = self
                        .filtered_indices
                        .iter()
                        .position(|&idx| idx == prev_entry_idx)
                {
                    self.list_state.select(Some(new_pos));
                    self.reset_detail_position();
                    self.horiz_offset = 0;
                    self.update_list_offset();
                    return;
                }
                self.list_state.select(Some(0));
                self.reset_detail_position();
                self.horiz_offset = 0;
                self.update_list_offset();
            }
        }
    }

    fn discover_columns(&mut self, value: &Value) {
        let mut to_add: Vec<ColumnDef> = Vec::new();

        if let Some(obj) = value.as_object() {
            for key in obj.keys() {
                if is_reserved_column(key) {
                    continue;
                }
                let path = vec![key.clone()];
                if !self.columns.iter().any(|c| c.path == path) {
                    to_add.push(ColumnDef::new(key.clone(), path));
                }
            }
        }

        if let Some(data) = value.get("data").and_then(|v| v.as_object()) {
            for key in data.keys() {
                if is_reserved_column(key) {
                    continue;
                }
                let path = vec!["data".to_string(), key.clone()];
                if !self.columns.iter().any(|c| c.path == path) {
                    to_add.push(ColumnDef::new(format!("data.{key}"), path));
                }
            }
        }

        if !to_add.is_empty() {
            self.columns.extend(to_add);
            let len = self.columns.len();
            if self
                .column_select_state
                .selected()
                .map(|i| i >= len)
                .unwrap_or(true)
            {
                self.column_select_state.select(Some(0));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry_with_message(msg: &str) -> LogEntry {
        LogEntry {
            timestamp: "-".into(),
            level: "INFO".into(),
            message: msg.to_string(),
            raw: json!({ "message": msg }),
        }
    }

    #[test]
    fn autoscroll_off_preserves_viewport_on_push() {
        let mut app = App::new(100);
        app.autoscroll = false;
        app.last_list_height = 3;
        app.entries = vec![
            entry_with_message("one"),
            entry_with_message("two"),
            entry_with_message("three"),
        ];
        app.filtered_indices = vec![0, 1, 2];
        app.list_state.select(Some(1));
        app.list_scroll_offset = 1;
        *app.list_state.offset_mut() = 1;

        app.push(entry_with_message("four"));

        assert_eq!(app.list_scroll_offset, 1, "offset should stay fixed");
        assert_eq!(
            app.list_state.offset(),
            1,
            "list state offset should remain"
        );
        assert_eq!(
            app.list_state.selected(),
            Some(1),
            "selection should be unchanged"
        );
        assert_eq!(app.filtered_indices.len(), 4, "new entry still recorded");
    }

    #[test]
    fn filter_preserves_selection_when_entry_still_matches() {
        let mut app = App::new(100);
        app.entries = vec![
            entry_with_message("one"),
            entry_with_message("two"),
            entry_with_message("three"),
        ];
        app.filtered_indices = vec![0, 1, 2];
        app.list_state.select(Some(1));

        app.apply_filter("two|three");

        let current = app.current_entry().unwrap();
        assert_eq!(current.message, "two");
        assert_eq!(app.filtered_indices, vec![1, 2]);
        assert_eq!(app.list_state.selected(), Some(0));
    }

    #[test]
    fn filter_moves_selection_when_previous_is_filtered_out() {
        let mut app = App::new(100);
        app.entries = vec![
            entry_with_message("one"),
            entry_with_message("two"),
            entry_with_message("three"),
        ];
        app.filtered_indices = vec![0, 1, 2];
        app.list_state.select(Some(0));

        app.apply_filter("two");

        let current = app.current_entry().unwrap();
        assert_eq!(current.message, "two");
        assert_eq!(app.filtered_indices, vec![1]);
        assert_eq!(app.list_state.selected(), Some(0));
    }

    #[test]
    fn eviction_keeps_alignment_and_selection_on_tail() {
        let mut app = App::new(2);
        app.push(entry_with_message("one"));
        app.push(entry_with_message("two"));
        app.push(entry_with_message("three")); // triggers eviction of "one"

        let messages: Vec<String> = app.entries.iter().map(|e| e.message.clone()).collect();
        assert_eq!(messages, vec!["two", "three"]);
        assert_eq!(app.filtered_indices, vec![0, 1]);
        assert_eq!(app.list_state.selected(), Some(1));
        assert_eq!(app.current_entry().unwrap().message, "three");
    }

    #[test]
    fn toggle_autoscroll_jumps_to_latest() {
        let mut app = App::new(10);
        app.autoscroll = false;
        app.entries = vec![
            entry_with_message("one"),
            entry_with_message("two"),
            entry_with_message("three"),
        ];
        app.filtered_indices = vec![0, 1, 2];
        app.list_state.select(Some(0));

        app.toggle_autoscroll();

        assert!(app.autoscroll);
        assert_eq!(app.list_state.selected(), Some(2));
        assert_eq!(app.current_entry().unwrap().message, "three");
    }

    #[test]
    fn invalid_filter_does_not_change_active_regex() {
        let mut app = App::new(10);
        app.entries = vec![entry_with_message("one")];
        app.filtered_indices = vec![0];

        app.apply_filter("["); // invalid regex

        assert!(app.filter_regex.is_none());
        assert!(app.filter_error.is_some());
        assert_eq!(app.filtered_indices, vec![0]);
    }

    #[test]
    fn column_discovery_adds_new_fields_once() {
        let mut app = App::new(10);
        assert_eq!(app.columns.len(), 3); // default columns

        app.discover_columns(&json!({"foo": "bar", "data": { "baz": 1 }}));
        let names: Vec<String> = app.columns.iter().map(|c| c.name.clone()).collect();
        assert!(names.contains(&"foo".to_string()));
        assert!(names.contains(&"data.baz".to_string()));

        // Repeat discovery should not duplicate
        app.discover_columns(&json!({"foo": "again", "data": { "baz": 2 }}));
        let foo_count = app.columns.iter().filter(|c| c.name == "foo").count();
        let baz_count = app.columns.iter().filter(|c| c.name == "data.baz").count();
        assert_eq!(foo_count, 1);
        assert_eq!(baz_count, 1);
    }

    #[test]
    fn toggle_column_enabled_flag() {
        let mut app = App::new(10);
        app.column_select_state.select(Some(0));
        let before = app.columns[0].enabled;
        app.move_column(0); // no-op move; ensures selection exists
        if let Some(idx) = app.column_select_state.selected() {
            app.columns[idx].enabled = !before;
        }
        assert_ne!(app.columns[0].enabled, before);
    }

    #[test]
    fn eviction_rebases_filtered_indices_with_filter_active() {
        let mut app = App::new(2);
        app.apply_filter("two|three"); // set filter before data arrives

        app.push(entry_with_message("one")); // filtered out
        app.push(entry_with_message("two")); // kept
        app.push(entry_with_message("three")); // evicts "one"

        let msgs: Vec<_> = app
            .filtered_indices
            .iter()
            .filter_map(|&i| app.entries.get(i))
            .map(|e| e.message.clone())
            .collect();
        assert_eq!(msgs, vec!["two", "three"]);
        assert_eq!(app.entries.len(), 2);
        assert_eq!(app.filtered_indices, vec![0, 1]);
        assert_eq!(app.list_state.selected(), Some(1));
        assert_eq!(app.current_entry().unwrap().message, "three");
    }

    #[test]
    fn field_view_filter_rebuilds_indices_and_resets_selection() {
        let mut app = App::new(10);
        let entry = LogEntry {
            timestamp: "-".into(),
            level: "INFO".into(),
            message: "has data".into(),
            raw: json!({"a": 1, "b": 2, "nested": { "c": 3 }}),
        };
        app.entries.push(entry.clone());
        app.filtered_indices = vec![0];
        app.list_state.select(Some(0));

        app.enter_field_view();
        let fv = app.field_view.as_mut().unwrap();
        // initial selection
        assert_eq!(fv.list_state.selected(), Some(0));
        fv.filter = "nested".into();
        let _ = fv.rebuild_filter(); // may return false when selection stays valid
        assert_eq!(fv.filtered_indices.len(), 2); // nested and nested.c
        assert_eq!(fv.list_state.selected(), Some(0));
    }

    #[test]
    fn paused_input_discards_new_entries() {
        let mut app = App::new(5);
        app.input_paused = true;

        app.ingest(entry_with_message("ignored"));

        assert!(app.entries.is_empty());
        assert!(app.filtered_indices.is_empty());
    }

    #[test]
    fn ingest_resumes_after_toggle() {
        let mut app = App::new(5);
        app.input_paused = true;

        app.ingest(entry_with_message("ignored"));
        app.toggle_input_pause();
        app.ingest(entry_with_message("kept"));

        assert_eq!(app.entries.len(), 1);
        assert_eq!(app.current_entry().unwrap().message, "kept");
    }
}
