use std::{
    fs::File,
    io::{self, BufRead, BufReader, IsTerminal},
    path::PathBuf,
    sync::mpsc,
    thread,
};

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat};
use serde_json::{Value, json};

use crate::{args::Args, model::LogEntry};

pub enum InputSource {
    Stdin,
    File(PathBuf),
    StdinPipe(File),
}

pub fn resolve_input_source(args: &Args) -> Result<InputSource> {
    if let Some(path) = args.file.clone() {
        Ok(InputSource::File(path))
    } else if io::stdin().is_terminal() {
        Ok(InputSource::Stdin)
    } else {
        let file = File::open("/dev/stdin").context("opening /dev/stdin")?;
        Ok(InputSource::StdinPipe(file))
    }
}

pub fn spawn_reader(input: InputSource, tx: mpsc::Sender<LogEntry>) {
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
