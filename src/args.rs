use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about = "Interactive TUI log viewer")]
pub struct Args {
    /// Optional file to read logs from (defaults to stdin)
    #[arg(short, long)]
    pub file: Option<PathBuf>,

    /// Maximum number of log entries to keep in memory
    #[arg(long, default_value_t = 5000)]
    pub max_entries: usize,
}
