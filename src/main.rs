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
use serde_json::{json, Value};
use unicode_width::UnicodeWidthStr;

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

#[derive(Clone, Debug)]
struct LogEntry {
    timestamp: String,
    level: String,
    message: String,
    raw: Value,
}

struct App {
    entries: Vec<LogEntry>,
    list_state: ListState,
    max_entries: usize,
    last_list_height: usize,
    last_detail_height: usize,
    detail_scroll: u16,
    detail_total_lines: usize,
    focus: Focus,
    show_help: bool,
}

impl App {
    fn new(max_entries: usize) -> Self {
        let mut list_state = ListState::default();
        list_state.select(None);
        Self {
            entries: Vec::new(),
            list_state,
            max_entries,
            last_list_height: 0,
            last_detail_height: 0,
            detail_scroll: 0,
            detail_total_lines: 0,
            focus: Focus::List,
            show_help: false,
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
        self.entries.push(entry);
        let last = self.entries.len().saturating_sub(1);
        self.list_state.select(Some(last));
        self.detail_scroll = 0;
    }

    fn next(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let next = (i + 1).min(self.entries.len() - 1);
        self.list_state.select(Some(next));
        self.detail_scroll = 0;
    }

    fn previous(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let prev = i.saturating_sub(1);
        self.list_state.select(Some(prev));
        self.detail_scroll = 0;
    }

    fn page_down(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let half = (self.last_list_height.max(1) / 2).max(1);
        let i = self.list_state.selected().unwrap_or(0);
        let next = (i + half).min(self.entries.len() - 1);
        self.list_state.select(Some(next));
        self.detail_scroll = 0;
    }

    fn page_up(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let half = (self.last_list_height.max(1) / 2).max(1);
        let i = self.list_state.selected().unwrap_or(0);
        let prev = i.saturating_sub(half);
        self.list_state.select(Some(prev));
        self.detail_scroll = 0;
    }

    fn select_last(&mut self) {
        if self.entries.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(self.entries.len() - 1));
        }
        self.detail_scroll = 0;
    }

    fn select_first(&mut self) {
        if self.entries.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(0));
        }
        self.detail_scroll = 0;
    }

    fn current_entry(&self) -> Option<LogEntry> {
        self.list_state
            .selected()
            .and_then(|i| self.entries.get(i))
            .cloned()
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    List,
    Detail,
}

enum InputSource {
    Stdin,
    File(PathBuf),
    StdinPipe(File),
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

        terminal
            .draw(|f| ui(f, app))
            .context("drawing frame")?;

        if event::poll(Duration::from_millis(100)).context("polling for events")? {
            match event::read().context("reading event")? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if key.code == KeyCode::Char('q') || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)) {
                        break;
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

                    match app.focus {
                        Focus::List => match key.code {
                            KeyCode::Char('j') | KeyCode::Down => app.next(),
                            KeyCode::Char('k') | KeyCode::Up => app.previous(),
                            KeyCode::Char('h') => app.previous(),
                            KeyCode::Char('l') => app.next(),
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
                            KeyCode::Char('j') | KeyCode::Down | KeyCode::Char('l') => {
                                app.detail_down(1)
                            }
                            KeyCode::Char('k') | KeyCode::Up | KeyCode::Char('h') => {
                                app.detail_up(1)
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
    let area = f.size();
    // Clear the full frame to avoid stray output from other streams (e.g., piped command stderr).
    f.render_widget(Clear, area);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

    app.last_list_height = chunks[0].height.saturating_sub(2) as usize;
    app.last_detail_height = chunks[1].height.saturating_sub(2) as usize;

    let items: Vec<ListItem> = app
        .entries
        .iter()
        .map(|entry| {
            let content = format!(
                "{}  {:<5} {}",
                entry.timestamp,
                entry.level.to_uppercase(),
                entry.message
            );
            ListItem::new(content).style(level_style(&entry.level))
        })
        .collect();

    let list_block = Block::default()
        .title("Logs")
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

    let selected_entry = app
        .list_state
        .selected()
        .and_then(|i| app.entries.get(i))
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

    if app.show_help {
        render_help(f, area);
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
        _ => Style::default(),
    }
}

fn level_span(level: &str) -> Span<'static> {
    Span::styled(level.to_ascii_uppercase(), level_style(level))
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
            keys: "Tab, Left, Esc",
            description: "Back to list",
        },
        Shortcut {
            context: "Detail",
            keys: "e",
            description: "Open entry in $EDITOR",
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
    let value: Value = serde_json::from_str(line).context("invalid JSON")?;

    let timestamp = extract_timestamp(&value);
    let level = value
        .get("level")
        .and_then(|v| v.as_str())
        .unwrap_or("UNKNOWN")
        .to_string();
    let message = value
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(LogEntry {
        timestamp,
        level,
        message,
        raw: value,
    })
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
