use std::{env, fs, process::Command};

use anyhow::Result;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::Backend, Terminal};

use crate::model::LogEntry;
use serde_json::Value;

pub fn open_entry_in_editor<B: Backend>(terminal: &mut Terminal<B>, entry: &LogEntry) -> Result<()> {
    let sanitized_ts: String = entry
        .timestamp
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let filename = format!("logtui-{}.json", sanitized_ts);
    let contents = serde_json::to_string_pretty(&entry.raw)?;
    open_text_in_editor(terminal, &filename, &contents)
}

pub fn open_value_in_editor<B: Backend>(
    terminal: &mut Terminal<B>,
    path_label: &str,
    value: &Value,
) -> Result<()> {
    let contents = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        _ => serde_json::to_string_pretty(value)?,
    };
    let sanitized: String = path_label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let filename = if sanitized.is_empty() {
        "logtui-field.txt".to_string()
    } else {
        format!("logtui-field-{sanitized}.txt")
    };
    open_text_in_editor(terminal, &filename, &contents)
}

fn open_text_in_editor<B: Backend>(
    terminal: &mut Terminal<B>,
    filename: &str,
    contents: &str,
) -> Result<()> {
    // Leave the TUI cleanly.
    disable_raw_mode().ok();
    let mut stdout = std::io::stdout();
    execute!(stdout, LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();

    let result = (|| -> Result<()> {
        let mut path = env::temp_dir();
        path.push(filename);
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
