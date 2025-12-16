use std::{sync::mpsc, time::Duration};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{Terminal, backend::Backend, widgets::ListState};
use regex::{Regex, escape};
use serde_json::Value;

use crate::{
    editor::{open_entry_in_editor, open_value_in_editor},
    model::LogEntry,
    ui,
};

#[derive(Clone, Debug)]
pub struct ColumnDef {
    pub name: String,
    pub path: Vec<String>,
    pub enabled: bool,
}

impl ColumnDef {
    fn new(name: String, path: Vec<String>) -> Self {
        Self {
            name,
            path,
            enabled: false,
        }
    }
}

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

    fn reset_detail_position(&mut self) {
        self.detail_scroll = 0;
        self.detail_horiz_offset = 0;
    }

    fn reset_field_detail_position(&mut self) {
        self.field_detail_scroll = 0;
        self.field_detail_horiz_offset = 0;
    }

    pub fn clamp_detail_horiz_offset(&mut self) {
        if self.detail_wrap {
            self.detail_horiz_offset = 0;
            return;
        }
        let max_off = self
            .detail_max_line_width
            .saturating_sub(self.last_detail_width.max(1));
        self.detail_horiz_offset = self.detail_horiz_offset.min(max_off);
    }

    pub fn clamp_field_detail_horiz_offset(&mut self) {
        if self.field_detail_wrap {
            self.field_detail_horiz_offset = 0;
            return;
        }
        let max_off = self
            .field_detail_max_line_width
            .saturating_sub(self.last_field_detail_width.max(1));
        self.field_detail_horiz_offset = self.field_detail_horiz_offset.min(max_off);
    }

    pub fn clamp_offset(&mut self) {
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

        if preserve_view {
            if let Some(prev_entry_idx) = prev_selected_entry {
                if let Some(new_pos) = self
                    .filtered_indices
                    .iter()
                    .position(|&idx| idx == prev_entry_idx)
                {
                    self.list_state.select(Some(new_pos));
                    return;
                }
            }
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
                if let Some(prev_entry_idx) = prev_selected_entry {
                    if let Some(new_pos) = self
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

fn default_columns() -> Vec<ColumnDef> {
    vec![
        ColumnDef {
            name: "timestamp".into(),
            path: vec!["timestamp".into()],
            enabled: true,
        },
        ColumnDef {
            name: "level".into(),
            path: vec!["level".into()],
            enabled: true,
        },
        ColumnDef {
            name: "message".into(),
            path: vec!["message".into()],
            enabled: true,
        },
    ]
}

fn is_reserved_column(key: &str) -> bool {
    matches!(key, "timestamp" | "level" | "message" | "instant" | "data")
}

#[derive(Clone)]
pub struct FieldEntry {
    pub path: String,
    pub value: Value,
}

pub struct FieldViewState {
    pub fields: Vec<FieldEntry>,
    pub filtered_indices: Vec<usize>,
    pub list_state: ListState,
    pub filter: String,
}

impl FieldViewState {
    fn selected_field(&self) -> Option<&FieldEntry> {
        let idx = self.list_state.selected()?;
        let field_idx = *self.filtered_indices.get(idx)?;
        self.fields.get(field_idx)
    }

    fn rebuild_filter(&mut self) -> bool {
        let old_selection = self.list_state.selected();
        let filter = self.filter.to_lowercase();
        self.filtered_indices.clear();
        for (idx, field) in self.fields.iter().enumerate() {
            if filter.is_empty() || field.path.to_lowercase().contains(&filter) {
                self.filtered_indices.push(idx);
            }
        }
        if self.filtered_indices.is_empty() {
            self.list_state.select(None);
        } else {
            match old_selection {
                Some(sel) if sel < self.filtered_indices.len() => {}
                _ => self.list_state.select(Some(0)),
            }
        }
        old_selection != self.list_state.selected()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldZoom {
    Detail,
}

fn collect_fields(value: &Value) -> Vec<FieldEntry> {
    fn walk(value: &Value, path: String, out: &mut Vec<FieldEntry>) {
        let display_path = if path.is_empty() {
            "(root)".to_string()
        } else {
            path.clone()
        };
        out.push(FieldEntry {
            path: display_path,
            value: value.clone(),
        });
        match value {
            Value::Object(map) => {
                for (key, v) in map {
                    let next = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    walk(v, next, out);
                }
            }
            Value::Array(arr) => {
                for (idx, v) in arr.iter().enumerate() {
                    let next = if path.is_empty() {
                        format!("[{idx}]")
                    } else {
                        format!("{path}[{idx}]")
                    };
                    walk(v, next, out);
                }
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    walk(value, String::new(), &mut out);
    out
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
        assert_eq!(app.list_state.offset(), 1, "list state offset should remain");
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
        let foo_count = app
            .columns
            .iter()
            .filter(|c| c.name == "foo")
            .count();
        let baz_count = app
            .columns
            .iter()
            .filter(|c| c.name == "data.baz")
            .count();
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

        app.push(entry_with_message("one"));   // filtered out
        app.push(entry_with_message("two"));   // kept
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
}

fn move_field_selection(app: &mut App, delta: isize) {
    let Some(fv) = app.field_view.as_mut() else {
        return;
    };
    if fv.filtered_indices.is_empty() {
        return;
    }
    let len = fv.filtered_indices.len();
    let current_opt = fv.list_state.selected();
    let current = current_opt.unwrap_or(0).min(len.saturating_sub(1));
    let new_idx = (current as isize + delta).clamp(0, (len as isize) - 1) as usize;
    if current_opt.is_none() {
        fv.list_state.select(Some(new_idx));
        app.field_detail_scroll = 0;
        app.force_redraw = true;
        return;
    }
    if new_idx == current {
        return;
    }
    fv.list_state.select(Some(new_idx));
    app.reset_field_detail_position();
    app.force_redraw = true;
}

fn move_field_half_page(app: &mut App, delta: isize) {
    let step = (app.last_field_list_height.max(1) / 2).max(1) as isize;
    move_field_selection(app, delta * step);
}

fn field_value_for_filter(entry: &FieldEntry) -> String {
    match &entry.value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

fn move_field_value_to_filter(app: &mut App, entry: &FieldEntry) {
    let literal = escape(&field_value_for_filter(entry));
    app.exit_field_view();
    app.focus = Focus::List;
    app.filter_buffer = literal;
    app.filter_error = None;
    app.input_mode = InputMode::FilterInput;
}

fn cycle_field_zoom(app: &mut App) {
    app.field_zoom = match app.field_zoom {
        None => Some(FieldZoom::Detail),
        Some(FieldZoom::Detail) => None,
    };
}

pub fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    rx: mpsc::Receiver<LogEntry>,
) -> Result<()> {
    loop {
        for entry in rx.try_iter() {
            app.push(entry);
        }

        if app.force_redraw {
            terminal.clear().ok();
            app.force_redraw = false;
        }

        terminal
            .draw(|f| ui::render(f, app))
            .context("drawing frame")?;

        if event::poll(Duration::from_millis(100)).context("polling for events")? {
            match event::read().context("reading event")? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if key.code == KeyCode::Char('q')
                        || (key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL))
                    {
                        break;
                    }
                    if key.modifiers.contains(KeyModifiers::CONTROL) {
                        if matches!(app.input_mode, InputMode::FieldView) {
                            match key.code {
                                KeyCode::Char('j') | KeyCode::Char('n') => {
                                    move_field_selection(app, 1);
                                    continue;
                                }
                                KeyCode::Char('k') | KeyCode::Char('p') => {
                                    move_field_selection(app, -1);
                                    continue;
                                }
                                _ => {}
                            }
                        }
                        match key.code {
                            KeyCode::Char('n') => {
                                app.next();
                                continue;
                            }
                            KeyCode::Char('p') => {
                                app.previous();
                                continue;
                            }
                            KeyCode::Char('t') => {
                                app.enter_field_view();
                                continue;
                            }
                            KeyCode::Char('z') => {
                                if matches!(app.input_mode, InputMode::FieldView) {
                                    cycle_field_zoom(app);
                                } else {
                                    app.zoom = match app.zoom {
                                        Some(Focus::List) => None,
                                        _ => Some(app.focus),
                                    };
                                }
                                continue;
                            }
                            KeyCode::Char('e') => {
                                if matches!(app.input_mode, InputMode::FieldView) {
                                    if let Some(sel) =
                                        app.field_view.as_ref().and_then(|fv| fv.selected_field())
                                    {
                                        open_value_in_editor(terminal, &sel.path, &sel.value)?;
                                    }
                                } else if let Some(entry) = app.current_entry() {
                                    open_entry_in_editor(terminal, &entry)?;
                                }
                                continue;
                            }
                            _ => {}
                        }
                    }
                    if key.code == KeyCode::Char('z')
                        && !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !matches!(app.input_mode, InputMode::FieldView)
                    {
                        app.zoom = match app.zoom {
                            Some(Focus::List) if matches!(app.focus, Focus::List) => None,
                            Some(Focus::Detail) if matches!(app.focus, Focus::Detail) => None,
                            _ => Some(app.focus),
                        };
                        continue;
                    }
                    if matches!(app.input_mode, InputMode::FieldView) {
                        if key.code == KeyCode::Esc
                            || (key.code == KeyCode::Char('t')
                                && key.modifiers.contains(KeyModifiers::CONTROL))
                        {
                            app.exit_field_view();
                            continue;
                        }
                        if key.code == KeyCode::Char('/')
                            && !key.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            if let Some(selected) = app
                                .field_view
                                .as_ref()
                                .and_then(|fv| fv.selected_field())
                                .cloned()
                            {
                                move_field_value_to_filter(app, &selected);
                            }
                            continue;
                        }
                        if let Some(fv) = app.field_view.as_mut() {
                            match key.code {
                                KeyCode::Char('w') => {
                                    app.field_detail_wrap = !app.field_detail_wrap;
                                    app.reset_field_detail_position();
                                    app.force_redraw = true;
                                }
                                KeyCode::Char('h') => {
                                    if !app.field_detail_wrap {
                                        let step = (app.last_field_detail_width / 4).max(4);
                                        app.field_detail_horiz_offset =
                                            app.field_detail_horiz_offset.saturating_sub(step);
                                        app.clamp_field_detail_horiz_offset();
                                    }
                                }
                                KeyCode::Char('l') => {
                                    if !app.field_detail_wrap {
                                        let step = (app.last_field_detail_width / 4).max(4);
                                        app.field_detail_horiz_offset =
                                            app.field_detail_horiz_offset.saturating_add(step);
                                        app.clamp_field_detail_horiz_offset();
                                    }
                                }
                                KeyCode::Char('0') => {
                                    if !app.field_detail_wrap {
                                        app.field_detail_horiz_offset = 0;
                                    }
                                }
                                KeyCode::Char('$') => {
                                    if !app.field_detail_wrap
                                        && app.field_detail_max_line_width
                                            > app.last_field_detail_width
                                    {
                                        app.field_detail_horiz_offset = app
                                            .field_detail_max_line_width
                                            .saturating_sub(app.last_field_detail_width);
                                        app.clamp_field_detail_horiz_offset();
                                    }
                                }
                                KeyCode::Backspace => {
                                    if !fv.filter.is_empty() {
                                        fv.filter.pop();
                                        let changed = fv.rebuild_filter();
                                        if changed {
                                            app.reset_field_detail_position();
                                            app.force_redraw = true;
                                        }
                                    }
                                }
                                KeyCode::Char('u')
                                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                                {
                                    if !fv.filter.is_empty() {
                                        fv.filter.clear();
                                        let changed = fv.rebuild_filter();
                                        if changed {
                                            app.reset_field_detail_position();
                                            app.force_redraw = true;
                                        }
                                    }
                                }
                                KeyCode::Char(c)
                                    if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                                {
                                    fv.filter.push(c);
                                    let changed = fv.rebuild_filter();
                                    if changed {
                                        app.reset_field_detail_position();
                                        app.force_redraw = true;
                                    }
                                }
                                KeyCode::Down => {
                                    if fv.filtered_indices.is_empty() {
                                        continue;
                                    }
                                    move_field_selection(app, 1);
                                }
                                KeyCode::Up => {
                                    if fv.filtered_indices.is_empty() {
                                        continue;
                                    }
                                    move_field_selection(app, -1);
                                }
                                KeyCode::Char('d')
                                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                                {
                                    if matches!(app.field_zoom, Some(FieldZoom::Detail)) {
                                        let half = (app.last_field_detail_height.max(1) / 2).max(1);
                                        let max_offset = app
                                            .field_detail_total_lines
                                            .saturating_sub(app.last_field_detail_height.max(1));
                                        let new = (app.field_detail_scroll as usize + half)
                                            .min(max_offset);
                                        app.field_detail_scroll = new as u16;
                                    } else {
                                        move_field_half_page(app, 1);
                                    }
                                }
                                KeyCode::Char('u')
                                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                                {
                                    if matches!(app.field_zoom, Some(FieldZoom::Detail)) {
                                        let half = (app.last_field_detail_height.max(1) / 2).max(1);
                                        app.field_detail_scroll =
                                            app.field_detail_scroll.saturating_sub(half as u16);
                                    } else {
                                        move_field_half_page(app, -1);
                                    }
                                }
                                _ => {}
                            }
                        }
                        continue;
                    }
                    if matches!(app.input_mode, InputMode::FilterInput) {
                        match key.code {
                            KeyCode::Esc => {
                                app.input_mode = InputMode::Normal;
                                app.filter_buffer.clear();
                                app.filter_error = None;
                            }
                            KeyCode::Enter => {
                                let pattern = app.filter_buffer.clone();
                                app.input_mode = InputMode::Normal;
                                app.apply_filter(&pattern);
                            }
                            KeyCode::Backspace => {
                                app.filter_buffer.pop();
                            }
                            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.filter_buffer.clear();
                            }
                            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.filter_buffer.push(c);
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if key.code == KeyCode::Char('?') {
                        app.show_help = !app.show_help;
                        continue;
                    }

                    if app.show_help {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('?') => app.show_help = false,
                            KeyCode::Char('q') => break,
                            _ => {}
                        }
                        continue;
                    }

                    if matches!(app.input_mode, InputMode::ColumnSelect) {
                        match key.code {
                            KeyCode::Char('q') => break,
                            KeyCode::Esc | KeyCode::Char('c') => {
                                app.input_mode = InputMode::Normal;
                            }
                            KeyCode::Char(' ') | KeyCode::Enter => {
                                if let Some(idx) = app.column_select_state.selected() {
                                    if let Some(col) = app.columns.get_mut(idx) {
                                        col.enabled = !col.enabled;
                                    }
                                }
                            }
                            KeyCode::Char('J') => {
                                app.move_column(1);
                            }
                            KeyCode::Char('K') => {
                                app.move_column(-1);
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                let len = app.columns.len();
                                let next = app
                                    .column_select_state
                                    .selected()
                                    .map(|i| (i + 1).min(len.saturating_sub(1)))
                                    .or(Some(0));
                                app.column_select_state.select(next);
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                let prev = app
                                    .column_select_state
                                    .selected()
                                    .map(|i| i.saturating_sub(1))
                                    .or(Some(0));
                                app.column_select_state.select(prev);
                            }
                            KeyCode::Char('g') => app.column_select_state.select(Some(0)),
                            KeyCode::Char('G') => {
                                if !app.columns.is_empty() {
                                    app.column_select_state
                                        .select(Some(app.columns.len().saturating_sub(1)));
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }

                    match app.focus {
                        Focus::List => match key.code {
                            KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.force_redraw = true;
                            }
                            KeyCode::Char('j') | KeyCode::Down => app.next(),
                            KeyCode::Char('k') | KeyCode::Up => app.previous(),
                            KeyCode::Char('h') => {
                                let step = (app.last_list_width / 4).max(4);
                                app.horiz_offset = app.horiz_offset.saturating_sub(step);
                                app.clamp_offset();
                            }
                            KeyCode::Char('l') => {
                                let step = (app.last_list_width / 4).max(4);
                                app.horiz_offset = app.horiz_offset.saturating_add(step);
                                app.clamp_offset();
                            }
                            KeyCode::Char('0') => {
                                app.horiz_offset = 0;
                            }
                            KeyCode::Char('$') => {
                                if app.max_row_width > app.last_list_width {
                                    app.horiz_offset =
                                        app.max_row_width.saturating_sub(app.last_list_width);
                                } else {
                                    app.horiz_offset = 0;
                                }
                                app.clamp_offset();
                            }
                            KeyCode::Char('c') => {
                                app.input_mode = InputMode::ColumnSelect;
                                if app.column_select_state.selected().is_none()
                                    && !app.columns.is_empty()
                                {
                                    app.column_select_state.select(Some(0));
                                }
                            }
                            KeyCode::Char('a') => {
                                app.toggle_autoscroll();
                            }
                            KeyCode::Char('/') => {
                                app.input_mode = InputMode::FilterInput;
                                app.filter_buffer = app.filter_query.clone();
                                app.filter_error = None;
                            }
                            KeyCode::Char('z') => {
                                app.zoom = match app.zoom {
                                    Some(Focus::List) => None,
                                    _ => Some(Focus::List),
                                }
                            }
                            KeyCode::Char('w') => {
                                app.detail_wrap = !app.detail_wrap;
                                app.reset_detail_position();
                                app.force_redraw = true;
                            }
                            KeyCode::Char('g') => app.select_first(),
                            KeyCode::Char('G') => app.select_last(),
                            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.page_down()
                            }
                            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.page_up()
                            }
                            KeyCode::Enter | KeyCode::Tab | KeyCode::Right => {
                                app.focus = Focus::Detail;
                            }
                            _ => {}
                        },
                        Focus::Detail => match key.code {
                            KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.force_redraw = true;
                            }
                            KeyCode::Char('h') => {
                                let step = (app.last_detail_width / 4).max(4);
                                app.detail_horiz_offset =
                                    app.detail_horiz_offset.saturating_sub(step);
                                app.clamp_detail_horiz_offset();
                            }
                            KeyCode::Char('l') => {
                                let step = (app.last_detail_width / 4).max(4);
                                app.detail_horiz_offset =
                                    app.detail_horiz_offset.saturating_add(step);
                                app.clamp_detail_horiz_offset();
                            }
                            KeyCode::Char('0') => {
                                app.detail_horiz_offset = 0;
                            }
                            KeyCode::Char('$') => {
                                if app.detail_max_line_width > app.last_detail_width {
                                    app.detail_horiz_offset = app
                                        .detail_max_line_width
                                        .saturating_sub(app.last_detail_width);
                                    app.clamp_detail_horiz_offset();
                                } else {
                                    app.detail_horiz_offset = 0;
                                }
                            }
                            KeyCode::Char('c') => {
                                app.input_mode = InputMode::ColumnSelect;
                                if app.column_select_state.selected().is_none()
                                    && !app.columns.is_empty()
                                {
                                    app.column_select_state.select(Some(0));
                                }
                            }
                            KeyCode::Char('j') | KeyCode::Down => app.detail_down(1),
                            KeyCode::Char('k') | KeyCode::Up => app.detail_up(1),
                            KeyCode::Char('/') => {
                                app.input_mode = InputMode::FilterInput;
                                app.filter_buffer = app.filter_query.clone();
                                app.filter_error = None;
                            }
                            KeyCode::Char('z') => {
                                app.zoom = match app.zoom {
                                    Some(Focus::Detail) => None,
                                    _ => Some(Focus::Detail),
                                }
                            }
                            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                let half = (app.last_detail_height.max(1) / 2).max(1);
                                app.detail_down(half);
                            }
                            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                let half = (app.last_detail_height.max(1) / 2).max(1);
                                app.detail_up(half);
                            }
                            KeyCode::Char('w') => {
                                app.detail_wrap = !app.detail_wrap;
                                app.reset_detail_position();
                                app.force_redraw = true;
                            }
                            KeyCode::Char('g') => app.detail_top(),
                            KeyCode::Char('G') => app.detail_bottom(),
                            KeyCode::Tab | KeyCode::Esc | KeyCode::Left => {
                                app.focus = Focus::List;
                            }
                            _ => {}
                        },
                    }
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }

    Ok(())
}
