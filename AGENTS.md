# Repository Guidelines

This is a small library.

## Project Structure & Module Organization
- `Cargo.toml` at the root defines dependencies (`chrono`, `fern`,
  `log`, `lazy_static`) and crate metadata.
- Library code lives in `src/lib.rs`; parsing helpers are isolated in
  `src/parse.rs` and re-exported through the main library module.
- Unit tests reside beside the code under `#[cfg(test)]`
  modules.
- Build artifacts accumulate in `target/`; clean it with `cargo clean`
  before packaging or benchmarking.

## Build, Test, and Development Commands

- `cargo build` — compile the crate and surface warnings early.

- `cargo test` — run the existing unit tests in `lib.rs` and `parse.rs`.

- `cargo fmt` — format code with `rustfmt`; configure editors to run
  it on save.

- `cargo clippy` — lint for idiomatic Rust issues; treat warnings as
  blockers before merging.

## Coding Style & Naming Conventions

- Follow default `rustfmt` (4-space indentation, trailing commas,
  module ordering).

- Use `snake_case` for functions/tests, `CamelCase` for types, and
  `SCREAMING_SNAKE_CASE` for constants like `TIMEPARSER_FORMATS`.

- Keep modules cohesive: parsing-specific logic belongs in
  `src/parse.rs`; expose only the minimal public API from `lib.rs`.

- Prefer `?` over manual error propagation and log parsing decisions
  with `log::trace!` when extending time formats.

## Testing Guidelines

- Mirror new logic with unit tests in the corresponding module,
  prefixing functions with `test_` for clarity.

- Use `Utc::with_ymd_and_hms` to keep timestamps deterministic; adjust
  offsets explicitly when testing `FixedOffset` behavior.

- Run `cargo test -- --nocapture` when debugging to surface trace logs during runs.

## Commit & Pull Request Guidelines

- Start commit subjects with an imperative verb (e.g., `Add parser
  fallback`); keep them under 72 characters.

- Group related changes: update code, formatter output, and tests in
  the same commit to avoid red builds.

- PRs should link any tracking issues, summarize time parsing
  behaviors touched, and include screenshots or logs if behavior
  changes.

- Confirm `cargo fmt`, `cargo clippy`, and `cargo test` succeed before
  requesting review.
