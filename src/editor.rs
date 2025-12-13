use std::{env, fs, process::Command};

use anyhow::Result;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::Backend, Terminal};

use crate::model::LogEntry;

pub fn open_entry_in_editor<B: Backend>(terminal: &mut Terminal<B>, entry: &LogEntry) -> Result<()> {
    // Leave the TUI cleanly.
    disable_raw_mode().ok();
    let mut stdout = std::io::stdout();
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
