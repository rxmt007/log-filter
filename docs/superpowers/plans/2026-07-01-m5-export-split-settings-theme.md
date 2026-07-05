# M5 Export, Split, Settings, Theme Plan

## Scope

M5 completes the remaining file-only v1 tools:

- Export the current filtered/bookmark/error/all view, or a selected 1-based line range, to a file.
- Split large log files by byte budget or line count.
- Persist user-editable settings as TOML in the platform config directory, with a configurable storage directory field.
- Add GUI controls for export, split, settings, and light/dark theme.

ADB live streaming, packaging, CI, and platform-specific installer validation remain outside this autonomous pass.

## Architecture

- Keep `logcore` independent of Tauri/UI.
- Add pure, unit-tested `export`, `split`, and `config` modules.
- Export writes on the backend from indexed source rows; no exported row text is sent to the frontend.
- Split streams from disk using bounded buffers; no whole-file load.
- Tauri commands stay thin:
  - `export_logs(request) -> ExportSummary`
  - `split_file(request) -> SplitSummary`
  - `get_config() -> ConfigDto`
  - `set_config(config) -> ConfigDto`
- Frontend invokes commands from dialogs and stores only status/config metadata.

## TDD Plan

1. `config.rs`
   - Defaults use light theme, UTF-8, sensible row/font sizes, and no adb/storage override.
   - Round-trip TOML preserves editable fields.
   - Platform config path is derived without touching UI code.
2. `split.rs`
   - Reject zero byte/line limits.
   - Split by line count preserves input bytes and produces deterministic part names.
   - Split by byte count preserves input bytes and never creates oversized non-final parts.
3. `export.rs` + `session.rs`
   - Export selected inclusive line range writes the original source bytes.
   - Export filtered view writes only matched source lines and does not materialize all rows.
   - Export all/bookmark/error views reuse the same bounded writer path.

## Frontend Plan

- Add compact dialogs for export, split, and settings using the current LogFilter visual language.
- Use Tauri dialog plugin for open/save/directory selection.
- Toolbar actions:
  - Export opens a dialog with view/range mode and output path.
  - Split opens a dialog with source path, output directory, byte/line mode, and value.
  - Settings opens persisted config fields and storage location.
  - Theme button toggles light/dark and persists it.
- Apply theme through the app root class (`lf-theme-light`/`lf-theme-dark`) and CSS variables in `src/index.css`.

## Verification

Before declaring M5 complete:

- `cargo test -p logcore`
- `cargo build --workspace`
- `pnpm build`
- Adversarial self-review for:
  - unbounded IPC row/text transfer,
  - full-file reads in export/split,
  - config path and TOML persistence bugs,
  - UI actions that can run without required paths,
  - theme persistence mismatches.
