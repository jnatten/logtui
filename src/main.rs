use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, IsTerminal};
use std::process::Command;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat};
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use regex::Regex;
use serde_json::{json, Value};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Parser, Debug)]
#[command(author, version, about = "Interactive TUI log viewer")]
struct Args {
    /// Optional file to read logs from (defaults to stdin)
    #[arg(short, long)]
    file: Option<PathBuf>,

    /// Maximum number of log entries to keep in memory
    #[arg(long, default_value_t = 5000)]
    max_entries: usize,
}

impl App {
    fn clamp_offset(&mut self) {
        if self.max_row_width > self.last_list_width {
            let max_off = self.max_row_width.saturating_sub(self.last_list_width);
            self.horiz_offset = self.horiz_offset.min(max_off);
        } else {
            self.horiz_offset = 0;
        }
    }

    fn move_column(&mut self, delta: isize) {
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
}

#[derive(Clone, Debug)]
struct LogEntry {
    timestamp: String,
    level: String,
    message: String,
    raw: Value,
}

struct App {
    entries: Vec<LogEntry>,
    filtered_indices: Vec<usize>,
    columns: Vec<ColumnDef>,
    column_select_state: ListState,
    list_state: ListState,
    max_entries: usize,
    last_list_height: usize,
    last_list_width: usize,
    last_detail_height: usize,
    detail_scroll: u16,
    detail_total_lines: usize,
    focus: Focus,
    show_help: bool,
    zoom: Option<Focus>,
    filter_query: String,
    filter_regex: Option<Regex>,
    filter_error: Option<String>,
    input_mode: InputMode,
    filter_buffer: String,
    force_redraw: bool,
    max_row_width: usize,
    horiz_offset: usize,
}

impl App {
    fn new(max_entries: usize) -> Self {
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
        }
    }

    fn push(&mut self, entry: LogEntry) {
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

    fn next(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let next = (i + 1).min(self.filtered_indices.len() - 1);
        self.list_state.select(Some(next));
        self.detail_scroll = 0;
    }

    fn previous(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let prev = i.saturating_sub(1);
        self.list_state.select(Some(prev));
        self.detail_scroll = 0;
    }

    fn page_down(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let half = (self.last_list_height.max(1) / 2).max(1);
        let i = self.list_state.selected().unwrap_or(0);
        let next = (i + half).min(self.filtered_indices.len() - 1);
        self.list_state.select(Some(next));
        self.detail_scroll = 0;
    }

    fn page_up(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let half = (self.last_list_height.max(1) / 2).max(1);
        let i = self.list_state.selected().unwrap_or(0);
        let prev = i.saturating_sub(half);
        self.list_state.select(Some(prev));
        self.detail_scroll = 0;
    }

    fn select_last(&mut self) {
        if self.filtered_indices.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(self.filtered_indices.len() - 1));
        }
        self.detail_scroll = 0;
    }

    fn select_first(&mut self) {
        if self.filtered_indices.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(0));
        }
        self.detail_scroll = 0;
    }

    fn current_entry(&self) -> Option<LogEntry> {
        let idx = self.list_state.selected()?;
        let entry_idx = *self.filtered_indices.get(idx)?;
        self.entries.get(entry_idx).cloned()
    }

    fn detail_down(&mut self, lines: usize) {
        if self.detail_total_lines == 0 {
            return;
        }
        let max_offset = self
            .detail_total_lines
            .saturating_sub(self.last_detail_height.max(1));
        let new = (self.detail_scroll as usize + lines).min(max_offset);
        self.detail_scroll = new as u16;
    }

    fn detail_up(&mut self, lines: usize) {
        let new = self.detail_scroll.saturating_sub(lines as u16);
        self.detail_scroll = new;
    }

    fn detail_top(&mut self) {
        self.detail_scroll = 0;
    }

    fn detail_bottom(&mut self) {
        if self.detail_total_lines == 0 {
            self.detail_scroll = 0;
            return;
        }
        let max_offset = self
            .detail_total_lines
            .saturating_sub(self.last_detail_height.max(1));
        self.detail_scroll = max_offset as u16;
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

    fn apply_filter(&mut self, pattern: &str) {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    List,
    Detail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputMode {
    Normal,
    FilterInput,
    ColumnSelect,
}

#[derive(Clone, Copy)]
enum SelectStrategy {
    PreserveOrFirst,
    Last,
}

#[derive(Clone, Debug)]
struct ColumnDef {
    name: String,
    path: Vec<String>,
    enabled: bool,
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

enum InputSource {
    Stdin,
    File(PathBuf),
    StdinPipe(File),
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

fn main() -> Result<()> {
    let args = Args::parse();
    let input_source = if let Some(path) = args.file.clone() {
        InputSource::File(path)
    } else if io::stdin().is_terminal() {
        InputSource::Stdin
    } else {
        InputSource::StdinPipe(File::open("/dev/stdin").context("opening /dev/stdin")?)
    };

    let (tx, rx) = mpsc::channel();
    spawn_reader(input_source, tx);

    enable_raw_mode().context("enabling raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("entering alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("creating terminal")?;

    let mut app = App::new(args.max_entries);
    let res = run_app(&mut terminal, &mut app, rx);

    disable_raw_mode().context("disabling raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen).context("leaving alternate screen")?;
    terminal.show_cursor().ok();

    if let Err(err) = res {
        eprintln!("error: {err:?}");
    }

    Ok(())
}

fn run_app<B: Backend>(terminal: &mut Terminal<B>, app: &mut App, rx: mpsc::Receiver<LogEntry>) -> Result<()> {
    loop {
        // Drain any new log entries.
        for entry in rx.try_iter() {
            app.push(entry);
        }

        if app.force_redraw {
            terminal.clear().ok();
            app.force_redraw = false;
        }

        terminal.draw(|f| ui(f, app)).context("drawing frame")?;

        if event::poll(Duration::from_millis(100)).context("polling for events")? {
            match event::read().context("reading event")? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if key.code == KeyCode::Char('q') || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)) {
                        break;
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
                                // horizontal scroll right
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
                            KeyCode::Char('e') => {
                                if let Some(entry) = app.current_entry() {
                                    open_entry_in_editor(terminal, &entry)?;
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
                            KeyCode::Char('e') => {
                                if let Some(entry) = app.current_entry() {
                                    open_entry_in_editor(terminal, &entry)?;
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

fn ui(f: &mut Frame, app: &mut App) {
    let full_area = f.size();
    // Clear the full frame to avoid stray output from other streams (e.g., piped command stderr).
    f.render_widget(Clear, full_area);

    let show_status = matches!(app.input_mode, InputMode::FilterInput)
        || app.filter_error.is_some()
        || !app.filter_query.is_empty();

    let status_lines = if show_status {
        Some(status_lines(app))
    } else {
        None
    };

    let vertical = match &status_lines {
        Some(lines) => {
            let needed_height = (lines.len() as u16).saturating_add(2).max(3);
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(3), Constraint::Length(needed_height)])
                .split(full_area)
        }
        None => Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3)])
            .split(full_area),
    };

    let area = vertical[0];
    let chunks = match app.zoom {
        Some(Focus::List) => vec![area, Rect::new(0, 0, 0, 0)],
        Some(Focus::Detail) => vec![Rect::new(0, 0, 0, 0), area],
        None => Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(area)
            .to_vec(),
    };

    app.last_list_height = chunks[0].height.saturating_sub(2) as usize;
    let list_width = chunks[0].width.saturating_sub(2) as usize;
    app.last_list_width = list_width;
    app.last_detail_height = chunks[1].height.saturating_sub(2) as usize;
    let enabled_columns: Vec<&ColumnDef> = app.columns.iter().filter(|c| c.enabled).collect();
    let mut max_full_width = 0usize;
    let items: Vec<ListItem> = app
        .filtered_indices
        .iter()
        .filter_map(|&idx| app.entries.get(idx))
        .map(|entry| {
            let full = render_row(entry, &enabled_columns);
            max_full_width = max_full_width.max(full.width());
            let view = slice_row(&full, app.horiz_offset, list_width);
            ListItem::new(view).style(level_style(&entry.level))
        })
        .collect();
    app.max_row_width = max_full_width;
    app.clamp_offset();

    let list_title = if app.filter_query.is_empty() {
        "Logs".to_string()
    } else {
        format!("Logs [/{}]", app.filter_query)
    };

    let list_block = Block::default()
        .title(list_title)
        .borders(Borders::ALL)
        .border_style(match app.focus {
            Focus::List => Style::default().fg(Color::Cyan),
            Focus::Detail => Style::default(),
        });

    let list = List::new(items)
        .block(list_block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("▸ ");

    f.render_stateful_widget(list, chunks[0], &mut app.list_state);

    if chunks[1].width > 0 && chunks[1].height > 0 {
        let selected_entry = app
            .list_state
            .selected()
            .and_then(|i| app.filtered_indices.get(i))
            .and_then(|&idx| app.entries.get(idx))
            .cloned();
        let detail_text = selected_details(selected_entry);
        let inner_width = chunks[1].width.saturating_sub(2) as usize;
        app.detail_total_lines = wrapped_height(&detail_text, inner_width);
        let max_offset = app
            .detail_total_lines
            .saturating_sub(app.last_detail_height.max(1));
        if app.detail_scroll as usize > max_offset {
            app.detail_scroll = max_offset as u16;
        }

        let detail_block = Block::default()
            .title("Details")
            .borders(Borders::ALL)
            .border_style(match app.focus {
                Focus::Detail => Style::default().fg(Color::Cyan),
                Focus::List => Style::default(),
            });

        let detail = Paragraph::new(detail_text)
            .block(detail_block)
            .wrap(Wrap { trim: false })
            .scroll((app.detail_scroll, 0));
        f.render_widget(detail, chunks[1]);
    }

    if app.show_help {
        render_help(f, full_area);
    } else if let Some(lines) = status_lines {
        render_status(f, vertical[vertical.len() - 1], lines);
    }

    if matches!(app.input_mode, InputMode::ColumnSelect) {
        render_column_selector(f, full_area, app);
    }
}

fn selected_details(entry: Option<LogEntry>) -> Text<'static> {
    let Some(entry) = entry else {
        return Text::from("Waiting for logs...");
    };
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(format!("timestamp: {}", entry.timestamp)));
    lines.push(Line::from(vec![
        Span::raw("level: "),
        level_span(&entry.level),
    ]));
    lines.push(Line::from(format!("message: {}", entry.message)));
    lines.push(Line::from(""));
    render_value(&entry.raw, 0, false, &mut lines);
    Text::from(lines)
}

fn level_style(level: &str) -> Style {
    match level.to_ascii_uppercase().as_str() {
        "TRACE" => Style::default().fg(Color::LightGreen),
        "DEBUG" => Style::default().fg(Color::LightMagenta),
        "INFO" => Style::default().fg(Color::LightBlue),
        "WARN" | "WARNING" => Style::default().fg(Color::Yellow),
        "ERROR" => Style::default().fg(Color::Red),
        "CRITICAL" => Style::default().fg(Color::LightRed),
        "PARSE" => Style::default().fg(Color::Magenta),
        "TEXT" => Style::default().fg(Color::Gray),
        _ => Style::default(),
    }
}

fn level_span(level: &str) -> Span<'static> {
    Span::styled(level.to_ascii_uppercase(), level_style(level))
}

fn render_row(entry: &LogEntry, cols: &[&ColumnDef]) -> String {
    if cols.is_empty() {
        return "[no columns selected]".to_string();
    }
    let n = cols.len();
    let separator = " | ";
    let mut parts = Vec::with_capacity(n);
    for col in cols {
        let val = entry_field_or_raw(entry, &col.path).unwrap_or_default();
        parts.push(val);
    }
    parts.join(separator)
}

fn slice_row(s: &str, offset: usize, width: usize) -> String {
    let mut out = String::new();
    let mut current_width = 0usize;

    let mut skip_width = offset;

    for ch in s.chars() {
        let w = ch.width().unwrap_or(1);
        if skip_width > 0 {
            skip_width = skip_width.saturating_sub(w);
            continue;
        }
        if current_width + w > width {
            break;
        }
        out.push(ch);
        current_width += w;
    }

    if current_width < width {
        out.push_str(&" ".repeat(width.saturating_sub(current_width)));
    }

    out
}

fn extract_field_string<'a>(value: &'a Value, path: &[String]) -> Option<String> {
    if let Value::String(s) = value {
        if path.len() == 1 {
            match path[0].as_str() {
                "timestamp" => return Some("-".into()),
                "level" => return Some("TEXT".into()),
                "message" => return Some(s.clone()),
                _ => {}
            }
        }
    }

    let mut current = value;
    for key in path {
        current = current.get(key)?;
    }
    match current {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null => Some("null".into()),
        other => Some(other.to_string()),
    }
}

fn entry_field_or_raw(entry: &LogEntry, path: &[String]) -> Option<String> {
    if path.len() == 1 {
        match path[0].as_str() {
            "timestamp" => return Some(entry.timestamp.clone()),
            "level" => return Some(entry.level.clone()),
            "message" => return Some(entry.message.clone()),
            _ => {}
        }
    }
    extract_field_string(&entry.raw, path)
}

fn open_entry_in_editor<B: Backend>(terminal: &mut Terminal<B>, entry: &LogEntry) -> Result<()> {
    // Leave the TUI cleanly.
    disable_raw_mode().ok();
    let mut stdout = io::stdout();
    execute!(stdout, LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();

    let result = (|| -> Result<()> {
        let mut path = env::temp_dir();
        let sanitized_ts: String = entry
            .timestamp
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        path.push(format!("logtui-{}.json", sanitized_ts));

        let contents = serde_json::to_string_pretty(&entry.raw)?;
        fs::write(&path, contents)?;

        let editor = env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
        let status = Command::new(editor).arg(&path).status();
        match status {
            Ok(s) if !s.success() => {
                eprintln!("Editor exited with status: {s}");
            }
            Err(err) => {
                eprintln!("Failed to launch editor: {err}");
            }
            _ => {}
        }
        Ok(())
    })();

    // Restore the TUI.
    execute!(stdout, EnterAlternateScreen).ok();
    enable_raw_mode().ok();
    terminal.clear()?;

    result
}

#[derive(Clone, Copy)]
struct Shortcut {
    context: &'static str,
    keys: &'static str,
    description: &'static str,
}

fn all_shortcuts() -> Vec<Shortcut> {
    vec![
        Shortcut {
            context: "Global",
            keys: "q",
            description: "Quit",
        },
        Shortcut {
            context: "Global",
            keys: "Ctrl+C",
            description: "Quit",
        },
        Shortcut {
            context: "Global",
            keys: "?",
            description: "Toggle help",
        },
        Shortcut {
            context: "Global",
            keys: "/",
            description: "Filter logs (regex)",
        },
        Shortcut {
            context: "Global",
            keys: "Ctrl+L",
            description: "Force redraw",
        },
        Shortcut {
            context: "List",
            keys: "j/k, Up/Down, h/l",
            description: "Move selection",
        },
        Shortcut {
            context: "List",
            keys: "Ctrl+d / Ctrl+u",
            description: "Half-page down/up",
        },
        Shortcut {
            context: "List",
            keys: "g / G",
            description: "Jump to top/bottom",
        },
        Shortcut {
            context: "List",
            keys: "Enter, Tab, Right",
            description: "Focus details",
        },
        Shortcut {
            context: "List",
            keys: "z",
            description: "Toggle zoom (list)",
        },
        Shortcut {
            context: "List",
            keys: "e",
            description: "Open entry in $EDITOR",
        },
        Shortcut {
            context: "Detail",
            keys: "j/k, Up/Down, h/l",
            description: "Scroll details",
        },
        Shortcut {
            context: "Detail",
            keys: "Ctrl+d / Ctrl+u",
            description: "Half-page down/up",
        },
        Shortcut {
            context: "Detail",
            keys: "g / G",
            description: "Jump to top/bottom",
        },
        Shortcut {
            context: "Detail",
            keys: "z",
            description: "Toggle zoom (details)",
        },
        Shortcut {
            context: "Detail",
            keys: "Tab, Left, Esc",
            description: "Back to list",
        },
        Shortcut {
            context: "Detail",
            keys: "e",
            description: "Open entry in $EDITOR",
        },
        Shortcut {
            context: "Global",
            keys: "c",
            description: "Toggle column selector",
        },
    ]
}

fn render_help(f: &mut Frame, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    let mut entries = all_shortcuts();
    entries.sort_by(|a, b| a.context.cmp(b.context));
    let mut current_context: Option<&str> = None;
    for sc in entries {
        if current_context != Some(sc.context) {
            if current_context.is_some() {
                lines.push(Line::from(""));
            }
            current_context = Some(sc.context);
            lines.push(Line::styled(
                sc.context,
                Style::default().add_modifier(Modifier::BOLD),
            ));
        }
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:20}", sc.keys),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(sc.description),
        ]));
    }

    let width = (area.width.saturating_sub(10)).min(90).max(50);
    let needed_height = (lines.len() as u16).saturating_add(2);
    let max_allowed = area.height.saturating_sub(2);
    let height = needed_height.min(max_allowed).max(8);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup = Rect::new(x, y, width, height);

    let block = Block::default().title("Shortcuts").borders(Borders::ALL);
    let help = Paragraph::new(Text::from(lines)).block(block).wrap(Wrap { trim: false });
    f.render_widget(Clear, popup);
    f.render_widget(help, popup);
}

fn render_column_selector(f: &mut Frame, area: Rect, app: &mut App) {
    let width = (area.width.saturating_sub(10)).min(90).max(40);
    let height = (app.columns.len() as u16 + 4).min(area.height.saturating_sub(2)).max(6);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup = Rect::new(x, y, width, height);

    let items: Vec<ListItem> = app
        .columns
        .iter()
        .map(|c| {
            let prefix = if c.enabled { "[x]" } else { "[ ]" };
            let text = format!("{prefix} {}", c.name);
            ListItem::new(text)
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().title("Columns (space to toggle, Esc to close)").borders(Borders::ALL))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("▸ ");

    f.render_widget(Clear, popup);
    f.render_stateful_widget(list, popup, &mut app.column_select_state);
}

fn status_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();
    if matches!(app.input_mode, InputMode::ColumnSelect) {
        lines.push(Line::from(
            "Columns: j/k or arrows to move cursor, space/enter to toggle, J/K to move column, Esc to close",
        ));
        return lines;
    }
    if matches!(app.input_mode, InputMode::FilterInput) {
        lines.push(Line::from(format!("Filter (regex): {}_", app.filter_buffer)));
    } else if !app.filter_query.is_empty() {
        lines.push(Line::from(format!(
            "Filter: /{}/ ({})",
            app.filter_query,
            app.filtered_indices.len()
        )));
    } else {
        lines.push(Line::from("Filter: (none)"));
    }

    if let Some(err) = &app.filter_error {
        lines.push(Line::styled(
            format!("Filter error: {err}"),
            Style::default().fg(Color::Red),
        ));
    }
    lines
}

fn render_status(f: &mut Frame, area: Rect, lines: Vec<Line<'static>>) {
    let block = Block::default().borders(Borders::ALL);
    let status = Paragraph::new(Text::from(lines)).block(block).wrap(Wrap { trim: true });
    f.render_widget(Clear, area);
    f.render_widget(status, area);
}

fn indent_span(indent: usize) -> Span<'static> {
    Span::raw(" ".repeat(indent))
}

fn render_value(
    value: &Value,
    indent: usize,
    trailing_comma: bool,
    out: &mut Vec<Line<'static>>,
) {
    match value {
        Value::Object(map) => render_object(map, indent, trailing_comma, out),
        Value::Array(arr) => render_array(arr, indent, trailing_comma, out),
        _ => {
            let mut spans = vec![indent_span(indent)];
            spans.extend(render_primitive_spans(value));
            if trailing_comma {
                spans.push(Span::raw(","));
            }
            out.push(Line::from(spans));
        }
    }
}

fn render_object(
    map: &serde_json::Map<String, Value>,
    indent: usize,
    trailing_comma: bool,
    out: &mut Vec<Line<'static>>,
) {
    out.push(Line::from(vec![indent_span(indent), Span::raw("{")]));
    let len = map.len();
    for (idx, (key, value)) in map.iter().enumerate() {
        let is_last = idx + 1 == len;
        let mut spans = vec![
            indent_span(indent + 2),
            Span::styled(format!("\"{}\"", key), Style::default().fg(Color::Cyan)),
            Span::raw(": "),
        ];
        match value {
            Value::Object(_) | Value::Array(_) => {
                out.push(Line::from(spans));
                render_value(value, indent + 2, !is_last, out);
            }
            _ => {
                spans.extend(render_primitive_spans(value));
                if !is_last {
                    spans.push(Span::raw(","));
                }
                out.push(Line::from(spans));
            }
        }
    }
    let mut closing = vec![indent_span(indent), Span::raw("}")];
    if trailing_comma {
        closing.push(Span::raw(","));
    }
    out.push(Line::from(closing));
}

fn render_array(
    arr: &[Value],
    indent: usize,
    trailing_comma: bool,
    out: &mut Vec<Line<'static>>,
) {
    out.push(Line::from(vec![indent_span(indent), Span::raw("[")]));
    let len = arr.len();
    for (idx, value) in arr.iter().enumerate() {
        let is_last = idx + 1 == len;
        match value {
            Value::Object(_) | Value::Array(_) => {
                render_value(value, indent + 2, !is_last, out);
            }
            _ => {
                let mut spans = vec![indent_span(indent + 2)];
                spans.extend(render_primitive_spans(value));
                if !is_last {
                    spans.push(Span::raw(","));
                }
                out.push(Line::from(spans));
            }
        }
    }
    let mut closing = vec![indent_span(indent), Span::raw("]")];
    if trailing_comma {
        closing.push(Span::raw(","));
    }
    out.push(Line::from(closing));
}

fn render_primitive_spans(value: &Value) -> Vec<Span<'static>> {
    match value {
        Value::String(s) => vec![Span::styled(format!("\"{s}\""), Style::default().fg(Color::Green))],
        Value::Number(num) => vec![Span::styled(num.to_string(), Style::default().fg(Color::Yellow))],
        Value::Bool(b) => vec![Span::styled(b.to_string(), Style::default().fg(Color::Magenta))],
        Value::Null => vec![Span::styled("null", Style::default().fg(Color::Gray))],
        _ => vec![Span::raw(value.to_string())],
    }
}

fn wrapped_height(text: &Text<'_>, width: usize) -> usize {
    let effective_width = width.max(1);
    let mut total = 0usize;
    for line in &text.lines {
        let line_width: usize = line
            .spans
            .iter()
            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        let wrapped = if line_width == 0 {
            1
        } else {
            (line_width + effective_width - 1) / effective_width
        };
        total += wrapped.max(1);
    }
    if text.lines.is_empty() {
        0
    } else {
        total
    }
}

fn spawn_reader(input: InputSource, tx: mpsc::Sender<LogEntry>) {
    thread::spawn(move || {
        let reader: Box<dyn BufRead + Send> = match input {
            InputSource::Stdin => Box::new(BufReader::new(io::stdin())),
            InputSource::File(path) => match File::open(&path) {
                Ok(file) => Box::new(BufReader::new(file)),
                Err(err) => {
                    let _ = tx.send(LogEntry {
                        timestamp: "-".into(),
                        level: "PARSE".into(),
                        message: format!("Failed to open file {path:?}: {err}"),
                        raw: json!({"error": err.to_string(), "path": path}),
                    });
                    return;
                }
            },
            InputSource::StdinPipe(file) => Box::new(BufReader::new(file)),
        };

        for line in reader.lines() {
            match line {
                Ok(line) => match parse_log_line(&line) {
                    Ok(entry) => {
                        if tx.send(entry).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(LogEntry {
                            timestamp: "-".into(),
                            level: "PARSE".into(),
                            message: format!("Failed to parse line: {err}"),
                            raw: json!({ "error": err.to_string(), "line": line }),
                        });
                    }
                },
                Err(err) => {
                    let _ = tx.send(LogEntry {
                        timestamp: "-".into(),
                        level: "PARSE".into(),
                        message: format!("Failed to read line: {err}"),
                        raw: json!({ "error": err.to_string() }),
                    });
                }
            }
        }
    });
}

fn parse_log_line(line: &str) -> Result<LogEntry> {
    match serde_json::from_str::<Value>(line) {
        Ok(value) => {
            let timestamp = {
                let ts = extract_timestamp(&value);
                if ts == "-" {
                    if let Some(data) = value.get("data") {
                        extract_timestamp(data)
                    } else {
                        ts
                    }
                } else {
                    ts
                }
            };

            let level = find_str(&value, "level")
                .or_else(|| value.get("data").and_then(|d| find_str(d, "level")))
                .unwrap_or("UNKNOWN")
                .to_string();

            let message = find_str(&value, "message")
                .or_else(|| value.get("data").and_then(|d| find_str(d, "message")))
                .unwrap_or("")
                .to_string();

            Ok(LogEntry {
                timestamp,
                level,
                message,
                raw: value,
            })
        }
        Err(_) => Ok(LogEntry {
            timestamp: "-".into(),
            level: "TEXT".into(),
            message: line.to_string(),
            raw: Value::String(line.to_string()),
        }),
    }
}

fn find_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(|v| v.as_str())
}

fn extract_timestamp(value: &Value) -> String {
    if let Some(ts) = value.get("timestamp").and_then(|v| v.as_str()) {
        return ts.to_string();
    }

    if let Some(instant) = value.get("instant") {
        if let (Some(seconds), Some(nanos)) = (
            instant.get("epochSecond").and_then(|v| v.as_i64()),
            instant.get("nanoOfSecond").and_then(|v| v.as_u64()),
        ) {
            if let Some(dt) = DateTime::from_timestamp(seconds, nanos as u32) {
                return dt.to_rfc3339_opts(SecondsFormat::Millis, true);
            }
        }
    }

    "-".to_string()
}
