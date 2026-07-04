# M4 Bookmarks, Minimap, Sidecar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add persistent line bookmarks, bookmark/error navigation, bookmark/error-only views, and the design-baseline left minimap without sending full files or text results to the frontend.

**Architecture:** `logcore` owns bookmark state and sidecar TOML persistence next to the source file (`<file>.lfbookmarks.toml`). `Session` stores bookmark and error line-number vectors only; rows are still returned solely through bounded `get_rows(view,start,count)`. Tauri exposes thin commands for toggle/list/next/minimap; React renders a narrow left rail and keyboard/mouse controls.

**Tech Stack:** Rust + serde/toml, Tauri v2, React + TypeScript, Tailwind v4 CSS-first, TanStack Virtual, zustand, lucide-react.

---

## File Structure

- Create `crates/logcore/src/bookmarks.rs`: bookmark set, next/previous navigation, sidecar path/load/save tests.
- Modify `crates/logcore/src/session.rs`: load sidecar on open, persist bookmark toggles, maintain error line-number vector during indexing, add `RowsView::Bookmarks/Errors`, minimap buckets.
- Modify `crates/logcore/src/lib.rs` and `crates/logcore/Cargo.toml`: export module and add serde/toml.
- Modify `src-tauri/src/{dto.rs,commands.rs,lib.rs}`: bookmark/minimap DTOs and commands.
- Modify `src/{types.ts,lib/ipc.ts,store/session.ts,components/*,index.css}`: minimap rail, bookmark icon/left border, F2/F3 navigation, double-click toggle, only-bookmarks/only-errors view buttons.

## Task 1: Pure Bookmark Store

- [ ] Add failing tests for toggle add/remove, sorted list, next/previous wrap, and sidecar save/load.
- [ ] Run `cargo test -p logcore bookmarks::tests::`.
- [ ] Implement `BookmarkStore`, `BookmarkSidecar`, `sidecar_path_for`, `load_for_source`, and `save_for_source`.
- [ ] Re-run tests and commit `feat(logcore): add bookmark sidecar store`.

## Task 2: Session Bookmark/Error Views

- [ ] Add failing tests proving `toggle_bookmark` persists, `RowsView::Bookmarks` returns original line numbers, `RowsView::Errors` returns only E/F rows, `next_bookmark` wraps, and `minimap` returns bucket ticks.
- [ ] Run `cargo test -p logcore session::tests::`.
- [ ] Implement bookmark/error vectors and minimap generation in `Session`.
- [ ] Re-run `cargo test -p logcore` and commit `feat(logcore): add bookmark error views and minimap`.

## Task 3: Tauri Bookmark Commands

- [ ] Expose `toggle_bookmark`, `list_bookmarks`, `next_bookmark`, and `get_minimap`.
- [ ] Add `marked` row mapping from `Session::is_bookmarked`.
- [ ] Run `cargo build --workspace` and commit `feat(tauri): expose bookmark and minimap commands`.

## Task 4: M4 Frontend

- [ ] Extend TS types, IPC wrappers, and zustand state with `bookmarks`, `bookmarkRevision`, `minimap`, and views `bookmarks/errors`.
- [ ] Render left minimap rail with blue bookmark ticks, red error ticks, and a viewport box; click sets the target line.
- [ ] Support double-click row bookmark toggle, bookmark icon/left border, F2 previous bookmark and F3 next bookmark.
- [ ] Add toolbar view buttons for all/filtered/bookmarks/errors without changing the visible-window data path.
- [ ] Run `pnpm build` and commit `feat(ui): add bookmarks minimap interactions`.

## Task 5: M4 Verification And Review

- [ ] Run required verification: `cargo test -p logcore`, `cargo build --workspace`, `pnpm build`.
- [ ] Adversarial review: sidecar path safety, no full bookmark/error text IPC, row count cap intact, no Java copy, keyboard handlers cleaned up, and minimap bucket bounds.
- [ ] Fix Critical/Important findings, re-run verification, and commit `milestone: complete M4 bookmarks minimap`.

## Design Notes

- Follow `docs/design/LogWindow.dc.html`: minimap is a 26px left rail; bookmark ticks are blue, error ticks red, viewport is a blue outlined translucent box.
- Function beats design where needed: M4 adds actual sidecar persistence and keyboard navigation even though the design file shows mostly static interaction states.
