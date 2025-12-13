use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat};
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use serde_json::{json, Value};

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
}

impl App {
    fn new(max_entries: usize) -> Self {
        let mut list_state = ListState::default();
        list_state.select(None);
        Self {
            entries: Vec::new(),
            list_state,
            max_entries,
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
    }

    fn next(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let next = (i + 1).min(self.entries.len() - 1);
        self.list_state.select(Some(next));
    }

    fn previous(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let prev = i.saturating_sub(1);
        self.list_state.select(Some(prev));
    }

    fn select_last(&mut self) {
        if self.entries.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(self.entries.len() - 1));
        }
    }

    fn select_first(&mut self) {
        if self.entries.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(0));
        }
    }
}

enum InputSource {
    Stdin,
    File(PathBuf),
}

fn main() -> Result<()> {
    let args = Args::parse();
    let input_source = if let Some(path) = args.file {
        InputSource::File(path)
    } else {
        InputSource::Stdin
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
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('j') | KeyCode::Down => app.next(),
                    KeyCode::Char('k') | KeyCode::Up => app.previous(),
                    KeyCode::Char('g') => app.select_first(),
                    KeyCode::Char('G') => app.select_last(),
                    _ => {}
                },
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }

    Ok(())
}

fn ui(f: &mut Frame, app: &mut App) {
    let area = f.size();
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

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

    let list = List::new(items)
        .block(Block::default().title("Logs").borders(Borders::ALL))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("▸ ");

    f.render_stateful_widget(list, chunks[0], &mut app.list_state);

    let detail = selected_details(app);
    let detail = Paragraph::new(detail)
        .block(Block::default().title("Details").borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    f.render_widget(detail, chunks[1]);
}

fn selected_details(app: &App) -> String {
    let Some(idx) = app.list_state.selected() else {
        return "Waiting for logs...".to_string();
    };
    if let Some(entry) = app.entries.get(idx) {
        let mut header = format!(
            "timestamp: {}\nlevel: {}\nmessage: {}\n\n",
            entry.timestamp, entry.level, entry.message
        );
        let formatted = serde_json::to_string_pretty(&entry.raw).unwrap_or_else(|_| entry.raw.to_string());
        header.push_str(&formatted);
        header
    } else {
        "Waiting for logs...".to_string()
    }
}

fn level_style(level: &str) -> Style {
    match level.to_ascii_uppercase().as_str() {
        "ERROR" => Style::default().fg(Color::Red),
        "WARN" | "WARNING" => Style::default().fg(Color::Yellow),
        "INFO" => Style::default().fg(Color::Green),
        "DEBUG" => Style::default().fg(Color::Cyan),
        "TRACE" => Style::default().fg(Color::Gray),
        "PARSE" => Style::default().fg(Color::Magenta),
        _ => Style::default(),
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
