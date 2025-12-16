use std::{sync::mpsc, time::Duration};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{backend::Backend, Terminal};
use regex::escape;

use crate::{
    editor::{open_entry_in_editor, open_value_in_editor},
    ui,
};

mod columns;
mod field_view;
mod state;

pub use columns::ColumnDef;
pub use field_view::{FieldEntry, FieldViewState, FieldZoom};
pub use state::{App, Focus, InputMode};

use field_view::field_value_for_filter;

fn move_field_selection(app: &mut App, delta: isize) {
    let Some(fv) = app.field_view.as_mut() else {
        return;
    };
    if fv.filtered_indices.is_empty() {
        return;
    }
    let len = fv.filtered_indices.len();
    let current_opt = fv.list_state.selected();
    let current = current_opt.unwrap_or(0).min(len.saturating_sub(1));
    let new_idx = (current as isize + delta).clamp(0, (len as isize) - 1) as usize;
    if current_opt.is_none() {
        fv.list_state.select(Some(new_idx));
        app.field_detail_scroll = 0;
        app.force_redraw = true;
        return;
    }
    if new_idx == current {
        return;
    }
    fv.list_state.select(Some(new_idx));
    app.reset_field_detail_position();
    app.force_redraw = true;
}

fn move_field_half_page(app: &mut App, delta: isize) {
    let step = (app.last_field_list_height.max(1) / 2).max(1) as isize;
    move_field_selection(app, delta * step);
}

fn move_field_value_to_filter(app: &mut App, entry: &FieldEntry) {
    let literal = escape(&field_value_for_filter(entry));
    app.exit_field_view();
    app.focus = Focus::List;
    app.filter_buffer = literal;
    app.filter_error = None;
    app.input_mode = InputMode::FilterInput;
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
    rx: mpsc::Receiver<crate::model::LogEntry>,
) -> Result<()> {
    loop {
        for entry in rx.try_iter() {
            app.push(entry);
        }

        if app.force_redraw {
            terminal.clear().ok();
            app.force_redraw = false;
        }

        terminal
            .draw(|f| ui::render(f, app))
            .context("drawing frame")?;

        if event::poll(Duration::from_millis(100)).context("polling for events")? {
            match event::read().context("reading event")? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if key.code == KeyCode::Char('q')
                        || (key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL))
                    {
                        break;
                    }
                    if key.modifiers.contains(KeyModifiers::CONTROL) {
                        if matches!(app.input_mode, InputMode::FieldView) {
                            match key.code {
                                KeyCode::Char('j') | KeyCode::Char('n') => {
                                    move_field_selection(app, 1);
                                    continue;
                                }
                                KeyCode::Char('k') | KeyCode::Char('p') => {
                                    move_field_selection(app, -1);
                                    continue;
                                }
                                _ => {}
                            }
                        }
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
                                    if let Some(sel) =
                                        app.field_view.as_ref().and_then(|fv| fv.selected_field())
                                    {
                                        open_value_in_editor(terminal, &sel.path, &sel.value)?;
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
                            || (key.code == KeyCode::Char('t')
                                && key.modifiers.contains(KeyModifiers::CONTROL))
                        {
                            app.exit_field_view();
                            continue;
                        }
                        if key.code == KeyCode::Char('/')
                            && !key.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            if let Some(selected) = app
                                .field_view
                                .as_ref()
                                .and_then(|fv| fv.selected_field())
                                .cloned()
                            {
                                move_field_value_to_filter(app, &selected);
                            }
                            continue;
                        }
                        if let Some(fv) = app.field_view.as_mut() {
                            match key.code {
                                KeyCode::Char('w') => {
                                    app.field_detail_wrap = !app.field_detail_wrap;
                                    app.reset_field_detail_position();
                                    app.force_redraw = true;
                                }
                                KeyCode::Char('h') => {
                                    if !app.field_detail_wrap {
                                        let step = (app.last_field_detail_width / 4).max(4);
                                        app.field_detail_horiz_offset =
                                            app.field_detail_horiz_offset.saturating_sub(step);
                                        app.clamp_field_detail_horiz_offset();
                                    }
                                }
                                KeyCode::Char('l') => {
                                    if !app.field_detail_wrap {
                                        let step = (app.last_field_detail_width / 4).max(4);
                                        app.field_detail_horiz_offset =
                                            app.field_detail_horiz_offset.saturating_add(step);
                                        app.clamp_field_detail_horiz_offset();
                                    }
                                }
                                KeyCode::Char('0') => {
                                    if !app.field_detail_wrap {
                                        app.field_detail_horiz_offset = 0;
                                    }
                                }
                                KeyCode::Char('$') => {
                                    if !app.field_detail_wrap
                                        && app.field_detail_max_line_width
                                            > app.last_field_detail_width
                                    {
                                        app.field_detail_horiz_offset = app
                                            .field_detail_max_line_width
                                            .saturating_sub(app.last_field_detail_width);
                                        app.clamp_field_detail_horiz_offset();
                                    }
                                }
                                KeyCode::Backspace => {
                                    if !fv.filter.is_empty() {
                                        fv.filter.pop();
                                        let changed = fv.rebuild_filter();
                                        if changed {
                                            app.reset_field_detail_position();
                                            app.force_redraw = true;
                                        }
                                    }
                                }
                                KeyCode::Char('u')
                                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                                {
                                    if !fv.filter.is_empty() {
                                        fv.filter.clear();
                                        let changed = fv.rebuild_filter();
                                        if changed {
                                            app.reset_field_detail_position();
                                            app.force_redraw = true;
                                        }
                                    }
                                }
                                KeyCode::Char(c)
                                    if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                                {
                                    fv.filter.push(c);
                                    let changed = fv.rebuild_filter();
                                    if changed {
                                        app.reset_field_detail_position();
                                        app.force_redraw = true;
                                    }
                                }
                                KeyCode::Down => {
                                    if fv.filtered_indices.is_empty() {
                                        continue;
                                    }
                                    move_field_selection(app, 1);
                                }
                                KeyCode::Up => {
                                    if fv.filtered_indices.is_empty() {
                                        continue;
                                    }
                                    move_field_selection(app, -1);
                                }
                                KeyCode::Char('d')
                                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                                {
                                    if matches!(app.field_zoom, Some(FieldZoom::Detail)) {
                                        let half = (app.last_field_detail_height.max(1) / 2).max(1);
                                        let max_offset = app
                                            .field_detail_total_lines
                                            .saturating_sub(app.last_field_detail_height.max(1));
                                        let new = (app.field_detail_scroll as usize + half)
                                            .min(max_offset);
                                        app.field_detail_scroll = new as u16;
                                    } else {
                                        move_field_half_page(app, 1);
                                    }
                                }
                                KeyCode::Char('u')
                                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                                {
                                    if matches!(app.field_zoom, Some(FieldZoom::Detail)) {
                                        let half = (app.last_field_detail_height.max(1) / 2).max(1);
                                        app.field_detail_scroll =
                                            app.field_detail_scroll.saturating_sub(half as u16);
                                    } else {
                                        move_field_half_page(app, -1);
                                    }
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
                                    app.horiz_offset =
                                        app.max_row_width.saturating_sub(app.last_list_width);
                                } else {
                                    app.horiz_offset = 0;
                                }
                                app.clamp_offset();
                            }
                            KeyCode::Char('c') => {
                                app.input_mode = InputMode::ColumnSelect;
                                if app.column_select_state.selected().is_none()
                                    && !app.columns.is_empty()
                                {
                                    app.column_select_state.select(Some(0));
                                }
                            }
                            KeyCode::Char('a') => {
                                app.toggle_autoscroll();
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
                            KeyCode::Char('w') => {
                                app.detail_wrap = !app.detail_wrap;
                                app.reset_detail_position();
                                app.force_redraw = true;
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
                            KeyCode::Char('h') => {
                                let step = (app.last_detail_width / 4).max(4);
                                app.detail_horiz_offset =
                                    app.detail_horiz_offset.saturating_sub(step);
                                app.clamp_detail_horiz_offset();
                            }
                            KeyCode::Char('l') => {
                                let step = (app.last_detail_width / 4).max(4);
                                app.detail_horiz_offset =
                                    app.detail_horiz_offset.saturating_add(step);
                                app.clamp_detail_horiz_offset();
                            }
                            KeyCode::Char('0') => {
                                app.detail_horiz_offset = 0;
                            }
                            KeyCode::Char('$') => {
                                if app.detail_max_line_width > app.last_detail_width {
                                    app.detail_horiz_offset = app
                                        .detail_max_line_width
                                        .saturating_sub(app.last_detail_width);
                                    app.clamp_detail_horiz_offset();
                                } else {
                                    app.detail_horiz_offset = 0;
                                }
                            }
                            KeyCode::Char('c') => {
                                app.input_mode = InputMode::ColumnSelect;
                                if app.column_select_state.selected().is_none()
                                    && !app.columns.is_empty()
                                {
                                    app.column_select_state.select(Some(0));
                                }
                            }
                            KeyCode::Char('j') | KeyCode::Down => app.detail_down(1),
                            KeyCode::Char('k') | KeyCode::Up => app.detail_up(1),
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
                            KeyCode::Char('w') => {
                                app.detail_wrap = !app.detail_wrap;
                                app.reset_detail_position();
                                app.force_redraw = true;
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
