use ratatui::{
    prelude::*,
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    app::{App, ColumnDef, FieldEntry, FieldViewState, Focus, InputMode},
    model::LogEntry,
};

pub fn render(f: &mut Frame, app: &mut App) {
    if matches!(app.input_mode, InputMode::FieldView) {
        render_field_view(f, app);
        return;
    }

    let full_area = f.size();
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

fn render_field_view(f: &mut Frame, app: &mut App) {
    let area = f.size();
    f.render_widget(Clear, area);
    let Some(field_view) = app.field_view.as_mut() else {
        let block = Block::default().title("Fields").borders(Borders::ALL);
        let empty = Paragraph::new("No fields to display").block(block);
        f.render_widget(empty, area);
        return;
    };

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    let items: Vec<ListItem> = field_view
        .filtered_indices
        .iter()
        .filter_map(|&idx| field_view.fields.get(idx))
        .map(|field| ListItem::new(field.path.clone()))
        .collect();

    let list = List::new(items)
        .block(Block::default().title(field_title(field_view)).borders(Borders::ALL))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("▸ ");

    f.render_stateful_widget(list, chunks[0], &mut field_view.list_state);

    let selected = field_view
        .list_state
        .selected()
        .and_then(|i| field_view.filtered_indices.get(i))
        .and_then(|&idx| field_view.fields.get(idx));
    let detail_text = match selected {
        Some(entry) => field_value_text(entry),
        None => Text::from("Select a field"),
    };

    let inner_width = chunks[1].width.saturating_sub(2) as usize;
    app.last_field_detail_height = chunks[1].height.saturating_sub(2) as usize;
    app.field_detail_total_lines = wrapped_height(&detail_text, inner_width);
    let max_offset = app
        .field_detail_total_lines
        .saturating_sub(app.last_field_detail_height.max(1));
    if app.field_detail_scroll as usize > max_offset {
        app.field_detail_scroll = max_offset as u16;
    }

    let title = selected
        .map(|s| format!("Field: {}", s.path))
        .unwrap_or_else(|| "Field".to_string());
    let detail_block = Block::default().title(title).borders(Borders::ALL);
    let detail = Paragraph::new(detail_text)
        .block(detail_block)
        .wrap(Wrap { trim: false })
        .scroll((app.field_detail_scroll, 0));
    f.render_widget(detail, chunks[1]);
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

fn field_value_text(entry: &FieldEntry) -> Text<'static> {
    match &entry.value {
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => render_json_text(&entry.value),
        serde_json::Value::String(s) => Text::from(s.clone()),
        serde_json::Value::Number(n) => Text::from(n.to_string()),
        serde_json::Value::Bool(b) => Text::from(b.to_string()),
        serde_json::Value::Null => Text::from("null"),
    }
}

fn render_json_text(value: &serde_json::Value) -> Text<'static> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    render_value(value, 0, false, &mut lines);
    Text::from(lines)
}

fn field_title(field_view: &FieldViewState) -> String {
    if field_view.filter.is_empty() {
        "Fields (Ctrl+T or Esc to close)".to_string()
    } else {
        format!("Fields (filter: {})", field_view.filter)
    }
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
    let separator = " | ";
    let mut parts = Vec::with_capacity(cols.len());
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

fn extract_field_string(value: &serde_json::Value, path: &[String]) -> Option<String> {
    if let serde_json::Value::String(s) = value {
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
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Null => Some("null".into()),
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
    value: &serde_json::Value,
    indent: usize,
    trailing_comma: bool,
    out: &mut Vec<Line<'static>>,
) {
    match value {
        serde_json::Value::Object(map) => render_object(map, indent, trailing_comma, out),
        serde_json::Value::Array(arr) => render_array(arr, indent, trailing_comma, out),
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
    map: &serde_json::Map<String, serde_json::Value>,
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
            serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
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
    arr: &[serde_json::Value],
    indent: usize,
    trailing_comma: bool,
    out: &mut Vec<Line<'static>>,
) {
    out.push(Line::from(vec![indent_span(indent), Span::raw("[")]));
    let len = arr.len();
    for (idx, value) in arr.iter().enumerate() {
        let is_last = idx + 1 == len;
        match value {
            serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
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

fn render_primitive_spans(value: &serde_json::Value) -> Vec<Span<'static>> {
    match value {
        serde_json::Value::String(s) => vec![Span::styled(format!("\"{s}\""), Style::default().fg(Color::Green))],
        serde_json::Value::Number(num) => vec![Span::styled(num.to_string(), Style::default().fg(Color::Yellow))],
        serde_json::Value::Bool(b) => vec![Span::styled(b.to_string(), Style::default().fg(Color::Magenta))],
        serde_json::Value::Null => vec![Span::styled("null", Style::default().fg(Color::Gray))],
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
            context: "Global",
            keys: "Ctrl+T",
            description: "Open field viewer",
        },
        Shortcut {
            context: "Global",
            keys: "Ctrl+N",
            description: "Next log (any pane)",
        },
        Shortcut {
            context: "Global",
            keys: "Ctrl+P",
            description: "Previous log (any pane)",
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
