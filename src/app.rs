use std::{sync::mpsc, time::Duration};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{backend::Backend, Terminal, widgets::ListState};
use regex::Regex;
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
    pub max_entries: usize,
    pub last_list_height: usize,
    pub last_list_width: usize,
    pub last_detail_height: usize,
    pub detail_scroll: u16,
    pub detail_total_lines: usize,
    pub focus: Focus,
    pub show_help: bool,
    pub zoom: Option<Focus>,
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
            max_entries,
            last_list_height: 0,
            last_list_width: 0,
            last_detail_height: 0,
            detail_scroll: 0,
            detail_total_lines: 0,
            focus: Focus::List,
            show_help: false,
            zoom: None,
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
        }
        self.discover_columns(&entry.raw);
        self.entries.push(entry);
        self.rebuild_filtered(Some(SelectStrategy::Last));
        self.horiz_offset = 0;
    }

    pub fn next(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let next = (i + 1).min(self.filtered_indices.len() - 1);
        self.list_state.select(Some(next));
        self.detail_scroll = 0;
    }

    pub fn previous(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let prev = i.saturating_sub(1);
        self.list_state.select(Some(prev));
        self.detail_scroll = 0;
    }

    pub fn page_down(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let half = (self.last_list_height.max(1) / 2).max(1);
        let i = self.list_state.selected().unwrap_or(0);
        let next = (i + half).min(self.filtered_indices.len() - 1);
        self.list_state.select(Some(next));
        self.detail_scroll = 0;
    }

    pub fn page_up(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let half = (self.last_list_height.max(1) / 2).max(1);
        let i = self.list_state.selected().unwrap_or(0);
        let prev = i.saturating_sub(half);
        self.list_state.select(Some(prev));
        self.detail_scroll = 0;
    }

    pub fn select_last(&mut self) {
        if self.filtered_indices.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(self.filtered_indices.len() - 1));
        }
        self.detail_scroll = 0;
    }

    pub fn select_first(&mut self) {
        if self.filtered_indices.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(0));
        }
        self.detail_scroll = 0;
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
            self.rebuild_filtered(Some(SelectStrategy::PreserveOrFirst));
            return;
        }

        match Regex::new(pattern) {
            Ok(re) => {
                self.filter_query = pattern.to_string();
                self.filter_regex = Some(re);
                self.filter_error = None;
                self.rebuild_filtered(Some(SelectStrategy::PreserveOrFirst));
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
        self.field_detail_scroll = 0;
        self.field_detail_total_lines = 0;
        self.last_field_detail_height = 0;
        self.field_zoom = None;
        self.input_mode = InputMode::FieldView;
    }

    pub fn exit_field_view(&mut self) {
        self.field_view = None;
        self.field_detail_scroll = 0;
        self.field_detail_total_lines = 0;
        self.last_field_detail_height = 0;
        self.field_zoom = None;
        self.input_mode = InputMode::Normal;
    }

    fn matches_filter(&self, entry: &LogEntry) -> bool {
        if let Some(re) = &self.filter_regex {
            let hay = format!(
                "{} {} {} {}",
                entry.timestamp,
                entry.level,
                entry.message,
                entry.raw
            );
            re.is_match(&hay)
        } else {
            true
        }
    }

    fn rebuild_filtered(&mut self, strategy: Option<SelectStrategy>) {
        let prev_selected_entry = self
            .list_state
            .selected()
            .and_then(|idx| self.filtered_indices.get(idx))
            .copied();

        let mut filtered = Vec::with_capacity(self.entries.len());
        for (idx, entry) in self.entries.iter().enumerate() {
            if self.matches_filter(entry) {
                filtered.push(idx);
            }
        }

        self.filtered_indices = filtered;

        if self.filtered_indices.is_empty() {
            self.list_state.select(None);
            self.detail_scroll = 0;
            return;
        }

        match strategy.unwrap_or(SelectStrategy::PreserveOrFirst) {
            SelectStrategy::Last => {
                self.list_state
                    .select(Some(self.filtered_indices.len().saturating_sub(1)));
                self.detail_scroll = 0;
                self.horiz_offset = 0;
            }
            SelectStrategy::PreserveOrFirst => {
                if let Some(prev_entry_idx) = prev_selected_entry {
                    if let Some(new_pos) = self
                        .filtered_indices
                        .iter()
                        .position(|&idx| idx == prev_entry_idx)
                    {
                        self.list_state.select(Some(new_pos));
                        self.detail_scroll = 0;
                        self.horiz_offset = 0;
                        return;
                    }
                }
                self.list_state.select(Some(0));
                self.detail_scroll = 0;
                self.horiz_offset = 0;
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

        terminal.draw(|f| ui::render(f, app)).context("drawing frame")?;

        if event::poll(Duration::from_millis(100)).context("polling for events")? {
            match event::read().context("reading event")? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if key.code == KeyCode::Char('q') || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)) {
                        break;
                    }
                    if key.modifiers.contains(KeyModifiers::CONTROL) {
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
                                    if let Some(fv) = app.field_view.as_ref() {
                                        if let Some(sel) = fv
                                            .list_state
                                            .selected()
                                            .and_then(|i| fv.filtered_indices.get(i))
                                            .and_then(|&idx| fv.fields.get(idx))
                                        {
                                            open_value_in_editor(terminal, &sel.path, &sel.value)?;
                                        }
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
                            || (key.code == KeyCode::Char('t') && key.modifiers.contains(KeyModifiers::CONTROL))
                        {
                            app.exit_field_view();
                            continue;
                        }
                        if let Some(fv) = app.field_view.as_mut() {
                            match key.code {
                                KeyCode::Backspace => {
                                    if !fv.filter.is_empty() {
                                        fv.filter.pop();
                                        let changed = fv.rebuild_filter();
                                        if changed {
                                            app.field_detail_scroll = 0;
                                        }
                                    }
                                }
                                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    if !fv.filter.is_empty() {
                                        fv.filter.clear();
                                        let changed = fv.rebuild_filter();
                                        if changed {
                                            app.field_detail_scroll = 0;
                                        }
                                    }
                                }
                                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    fv.filter.push(c);
                                    let changed = fv.rebuild_filter();
                                    if changed {
                                        app.field_detail_scroll = 0;
                                    }
                                }
                                KeyCode::Down => {
                                    if fv.filtered_indices.is_empty() {
                                        continue;
                                    }
                                    let len = fv.filtered_indices.len();
                                    let next = fv
                                        .list_state
                                        .selected()
                                        .map(|i| (i + 1).min(len.saturating_sub(1)))
                                        .or(Some(0));
                                    fv.list_state.select(next);
                                    app.field_detail_scroll = 0;
                                }
                                KeyCode::Up => {
                                    if fv.filtered_indices.is_empty() {
                                        continue;
                                    }
                                    let prev = fv
                                        .list_state
                                        .selected()
                                        .map(|i| i.saturating_sub(1))
                                        .or(Some(0));
                                    fv.list_state.select(prev);
                                    app.field_detail_scroll = 0;
                                }
                                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    let half = (app.last_field_detail_height.max(1) / 2).max(1);
                                    let max_offset = app
                                        .field_detail_total_lines
                                        .saturating_sub(app.last_field_detail_height.max(1));
                                    let new = (app.field_detail_scroll as usize + half).min(max_offset);
                                    app.field_detail_scroll = new as u16;
                                }
                                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    let half = (app.last_field_detail_height.max(1) / 2).max(1);
                                    app.field_detail_scroll =
                                        app.field_detail_scroll.saturating_sub(half as u16);
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
                                    app.horiz_offset = app
                                        .max_row_width
                                        .saturating_sub(app.last_list_width);
                                } else {
                                    app.horiz_offset = 0;
                                }
                                app.clamp_offset();
                            }
                            KeyCode::Char('c') => {
                                app.input_mode = InputMode::ColumnSelect;
                                if app.column_select_state.selected().is_none() && !app.columns.is_empty() {
                                    app.column_select_state.select(Some(0));
                                }
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
                            KeyCode::Char('h') => app.detail_up(1),
                            KeyCode::Char('l') => app.detail_down(1),
                            KeyCode::Char('c') => {
                                app.input_mode = InputMode::ColumnSelect;
                                if app.column_select_state.selected().is_none() && !app.columns.is_empty() {
                                    app.column_select_state.select(Some(0));
                                }
                            }
                            KeyCode::Char('j') | KeyCode::Down => {
                                app.detail_down(1)
                            }
                            KeyCode::Char('k') | KeyCode::Up => {
                                app.detail_up(1)
                            }
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
