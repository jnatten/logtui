# Repository Guidelines

## Project Structure & Module Organization
- `src/main.rs`: CLI entry; wires argument parsing and launches the TUI app.
- `src/args.rs`, `input.rs`, `model.rs`, `editor.rs`, `ui.rs`: shared components for argument handling, input pipeline, data models, editor helpers, and rendering primitives.
- `src/app/`: feature modules (`state.rs` for core logic and tests, `columns.rs`/`field_view.rs` for layout/field rendering, `mod.rs` for exports). Add new screens or behaviors here.
- `target/`: build artifacts; safe to clean. `Cargo.lock` tracks dependency versions.
- `README.md`: quick usage; keep in sync when CLI flags or behavior change.

## Build, Test, and Development Commands
- `cargo fmt`: format the codebase (run before committing).
- `cargo clippy --all-targets --all-features`: lint for correctness and style; fix or explicitly allow warnings.
- `cargo test`: run unit tests (currently in `src/app/state.rs`; add more close to logic).
- `cargo run -- --help`: show available CLI options; use to validate flag docs.
- `cargo build --release`: produce an optimized binary at `target/release/logtui` for distribution.

## Coding Style & Naming Conventions
- Rust defaults: 4-space indent, `rustfmt` enforced; avoid trailing whitespace and unused imports.
- Naming: modules/files `snake_case`, types `PascalCase`, functions/vars `snake_case`, constants `SCREAMING_SNAKE_CASE`.
- Error handling: prefer `anyhow::Result` for app flow; use context (`with_context`) on fallible I/O.
- Keep UI/state updates small and testable; favor pure helpers in `app/state.rs` for behavior that can be unit-tested.

## Testing Guidelines
- Use `#[cfg(test)]` modules co-located with code; name tests after behavior (e.g., `updates_offset_on_scroll`).
- Aim for branch coverage on state transitions and parsing paths; add regression tests when fixing bugs.
- For TUI changes, assert on state/output model rather than terminal frames where possible.

## Commit & Pull Request Guidelines
- Follow Conventional Commit prefixes seen in history (`feat:`, `refactor:`, `docs:`, `test:`); keep subjects imperative and ~72 chars.
- PRs should include: summary of changes, testing performed (`cargo fmt`, `cargo clippy`, `cargo test`), and screenshots/GIFs of TUI changes when relevant.
- Link related issues; call out breaking changes or new CLI flags explicitly in the description.

## Security & Configuration Tips
- Do not commit sample logs with real secrets or PII; sanitize inputs used in docs or tests.
- If adding config files or integrations, default to least privilege and document required environment variables in `README.md`.
