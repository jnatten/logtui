mod app;
mod args;
mod editor;
mod input;
mod model;
mod ui;

use std::io;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::{
    app::App,
    args::Args,
    input::{resolve_input_source, spawn_reader},
};

fn main() -> Result<()> {
    let args = Args::parse();
    let input_source = resolve_input_source(&args)?;

    let (tx, rx) = std::sync::mpsc::channel();
    spawn_reader(input_source, tx);

    enable_raw_mode().context("enabling raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("entering alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("creating terminal")?;

    let mut app = App::new(args.max_entries);
    let res = app::run_app(&mut terminal, &mut app, rx);

    disable_raw_mode().context("disabling raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen).context("leaving alternate screen")?;
    terminal.show_cursor().ok();

    if let Err(err) = res {
        eprintln!("error: {err:?}");
    }

    Ok(())
}
