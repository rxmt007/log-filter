# M2 Filter, Highlight, Search Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add filter views, level styling, keyword/search highlighting, global search navigation, and the named M1 hardening fixes without violating the visible-window IPC rule.

**Architecture:** `logcore` stays UI-free: parser fixes, filter specs, and search specs are pure Rust and covered by unit tests. `Session` owns only line-number result vectors (`Vec<u64>`) for filtered/search views and still returns text only through bounded `get_rows(view,start,count)`. Tauri remains a thin command/event layer; React mirrors the design file with toolbar search, compact filter controls, level chips, and virtual rows.

**Tech Stack:** Rust + `regex`, Tauri v2, React 19 + TypeScript, Tailwind v4 CSS-first, TanStack Virtual, zustand, lucide-react.

---

## File Structure

- Modify `crates/logcore/src/parser.rs`: preserve fields for threadtime-like lines that have no `Tag:` colon.
- Create `crates/logcore/src/filter.rs`: `FilterSpec`, enabled field matchers, level masks, `|` multi-value substring/regex matching, pure filtering to zero-based line numbers.
- Create `crates/logcore/src/search.rs`: global search spec, count, first/next/previous line navigation over zero-based line matches.
- Modify `crates/logcore/src/session.rs`: store filtered/search line-number vectors, expose filtered rows/count and search navigation while keeping `get_rows` bounded.
- Modify `crates/logcore/src/lib.rs` and `crates/logcore/Cargo.toml`: export modules and add `regex`.
- Modify `src-tauri/src/{dto.rs,state.rs,commands.rs,lib.rs}`: DTOs and commands for `set_filter`, `get_filtered_count`, `search`, `search_next`; add status generation and lock-poison recovery.
- Modify `src/{types.ts,lib/ipc.ts,store/session.ts,components/*,App.tsx,index.css}`: design-aligned M2 UI, level coloring, keyword/search highlight, filter controls, search count and navigation.

## Task 1: Parser Hardening

- [ ] Add a failing test in `parser.rs` for `04-20 ... E NoColonTag message without colon`, expecting date/time/pid/tid/level/tag/message to be preserved.
- [ ] Run `cargo test -p logcore parser::tests::parses_threadtime_without_colon`.
- [ ] Implement the no-colon fallback by splitting the tail once at whitespace after the tag token.
- [ ] Re-run the parser test and commit `fix(logcore): preserve threadtime fields without colon`.

## Task 2: Pure Filter Engine

- [ ] Add failing tests in `filter.rs` for level masks, PID/TID exact matching, tag include/exclude, keyword include/exclude, `|` multi-values, regex matching, invalid regex rejection, and "all levels keeps raw lines".
- [ ] Run `cargo test -p logcore filter::tests::`.
- [ ] Implement `FilterSpec`, `FilterField`, `LevelMask`, matcher compilation, and `filter_entries`.
- [ ] Re-run filter tests and commit `feat(logcore): add pure filter engine`.

## Task 3: Pure Search Engine

- [ ] Add failing tests in `search.rs` for substring count/first, case sensitivity, regex search, invalid regex rejection, and previous/next wrap-around.
- [ ] Run `cargo test -p logcore search::tests::`.
- [ ] Implement `SearchSpec`, `SearchSummary`, `search_entries`, and `next_match`.
- [ ] Re-run search tests and commit `feat(logcore): add global search engine`.

## Task 4: Session Integration

- [ ] Add failing tests in `session.rs` proving filtered rows address original source lines, `filtered_count` returns only line numbers not copied text, and search next returns 1-based row numbers for IPC-friendly navigation.
- [ ] Run `cargo test -p logcore session::tests::`.
- [ ] Implement `set_filter`, `filtered_count`, `search`, `search_next`, and `get_rows("filtered")` behavior inside `Session`.
- [ ] Re-run logcore tests and commit `feat(logcore): wire filtered and search views into session`.

## Task 5: Tauri Commands And M1 State Fixes

- [ ] Add a failing unit test in `state.rs` that poisons the session mutex and then verifies `lock_session()` recovers the guard.
- [ ] Run `cargo test --workspace state::tests::recovers_poisoned_session_lock`.
- [ ] Add generation to `Status`, replace every `lock().unwrap()` with `AppState::lock_session()`, and expose M2 commands/events.
- [ ] Re-run `cargo build --workspace` and commit `feat(tauri): expose filter and search commands`.

## Task 6: Design-Aligned M2 Frontend

- [ ] Update TS types and IPC wrappers for filter/search/status generation.
- [ ] Extend zustand state with filter spec, active view, search query/options, current match, and stale-event generation checks.
- [ ] Replace the M1 toolbar/table/status shell with the design baseline: two-row toolbar, resident search box, level chips, collapsible filter bar, 9-column virtual table with hidden bookmark column by default, level-tinted rows, clipped long Tag, and highlighted query spans.
- [ ] Keep row fetching windowed (`WINDOW <= 512`) for both `all` and `filtered`.
- [ ] Run `pnpm build` and commit `feat(ui): add filter/search log viewer UI`.

## Task 7: M2 Verification And Review

- [ ] Run required verification: `cargo test -p logcore`, `cargo build --workspace`, `pnpm build`.
- [ ] Do adversarial self-review: search for full-file/full-filter-result IPC, `lock().unwrap()`, unbounded `get_rows`, copied Java, long Tag overflow, stale status generation, invalid regex handling, and UI/design conflicts.
- [ ] Fix Critical/Important findings, re-run required verification, and commit `milestone: complete M2 filter search UI`.

## Design Notes

- Follow `docs/design/LogWindow.dc.html`: compact toolbar, resident search, level chips, filter rows, level row tinting, yellow active search highlight, 154px Tag column with ellipsis.
- Intentional M2 limitation: minimap/bookmark interactions remain stubbed until M4; the left rail appears only when M4 implements its data source.
