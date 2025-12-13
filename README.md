# 🪵 logtui

A fast, keyboard-driven TUI for exploring structured (JSON) logs from stdin or files. It shows a concise log list with selectable rows, a details pane for full JSON, live regex filtering, column toggling/reordering, and vi-like navigation—perfect for tailing services, piping from tools like `stern`, or inspecting static log files.

## ✨ Features

- **Streaming input**: Read from stdin (pipes) or `--file` without buffering the world.
- **Interactive list + details**: Summaries on the left, full JSON on the right; zoom either pane with `z`.
- **Regex filtering**: Hit `/`, type a regex, Enter to apply; status bar shows active filter/errors.
- **Column control**: Toggle and reorder columns (including dynamically discovered fields) via `c`; horizontal scroll with `h/l`, jump with `0/$`.
- **Nested field fallback**: Automatically picks `timestamp/level/message` from top-level or `data.*`.
- **Graceful for plain text**: Non-JSON lines render as `TEXT` with the raw content.
- **Colorized levels & JSON**: Levels are colored per severity; details JSON is syntax-highlighted.

## 🚀 Quick start

```bash
# Run with a file
logtui --file article-api.log

# Pipe logs from stdin
kubectl logs mypod | logtui
```

## ⌨️ Keys (essentials)

- Help overlay: `?`
- Quit: `q`, `Ctrl+C`
- Filter (regex): `/` (type, Enter to apply, Esc to cancel)
- Zoom: `z` (zoom focused pane)
- Redraw: `Ctrl+L` (clears stray artifacts)
- Open in `$EDITOR`: `e`

### List pane

- Move: `j/k`, `Up/Down`
- Half page: `Ctrl+d / Ctrl+u`
- Jump: `g` / `G`
- Horizontal scroll: `h` / `l`; jump: `0` (start), `$` (end)
- Focus details: `Enter`, `Tab`, `Right`
- Column selector: `c`

### Detail pane

- Scroll: `j/k`, `Up/Down`
- Half page: `Ctrl+d / Ctrl+u`
- Jump: `g` / `G`
- Back to list: `Tab`, `Left`, `Esc`
- Column selector: `c`

### Column selector (after `c`)

- Move cursor: `j/k`, arrows
- Toggle column: `Space` or `Enter`
- Reorder: `J` (down), `K` (up)
- Close: `Esc`

## 🧠 Behavior notes

- **Dynamic columns**: New fields discovered in logs (top-level and `data.*`) appear in the selector; defaults are `timestamp`, `level`, `message` (with `message` at the end).
- **Filtering**: Applies to timestamp, level, message, and full JSON string. Invalid regex leaves the previous filter active and shows an error.
- **Nested fields**: If `timestamp/level/message` are under `data.*`, they’re used automatically.

## 🛠️ Build & run

```bash
cargo check
cargo run -- --file logfile.txt
# or
cat logfile.txt | cargo run
```

## 🧭 Tips

- Keep an eye on the status bar: it shows filter input/errors and column selector hints.
- If the screen ever looks odd after a producer prints to stderr, `Ctrl+L` cleans it up.
- Wide log records? Use `h/l` or `0/$` to pan horizontally; reorder columns to bring important fields forward.

Enjoy tailing! 🧭
