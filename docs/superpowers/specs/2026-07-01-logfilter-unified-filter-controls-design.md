# LogFilter Unified Filter Controls Design

- **Date**: 2026-07-01
- **Status**: User-approved design, ready for implementation planning
- **Scope**: Replace overlapping log level/view shortcuts with one unified filter expression. Preserve the existing large-file IPC rule: the frontend only requests visible row windows.

## 1. Problem

The current UI exposes overlapping concepts:

- View buttons: `全部 / 过滤 / 书签 / 错误 / 全级别`
- Level chips: `V / D / I / W / E / F`
- Field filters: Tag/PID/TID/keyword include/exclude

This creates ambiguity. For example, selecting or unselecting log levels is already filtering, so a separate `过滤` button is unnecessary. `错误` is also just a shortcut for `E + F`. `书签` is better modeled as another serial filter condition rather than a separate view mode.

The new design makes the main table always represent the current filter result.

## 2. UI Structure

The top toolbar keeps the visual language from `docs/design/LogFilter.dc.html` and `docs/design/LogWindow.dc.html`: compact icon buttons, subtle separators, level chips, low-radius borders, and a persistent search box.

The main filter strip becomes:

```text
级别 | 全部  V D I W E F  | [bookmark icon] 仅标记
```

Rules:

- `全部` moves next to the level chips and acts as a level all-select shortcut.
- Add a small visual gap or light separator between `全部` and `V/D/I/W/E/F`, so `全部` reads as a shortcut rather than another log level.
- Remove the lower filter title row buttons: `全部 / 过滤 / 书签 / 错误 / 全级别`.
- Remove the independent `错误` shortcut. Users select `E + F` for error/fatal logs.
- Add `仅标记` after the level chips. It uses a bookmark icon and compact chip styling matching the existing design.
- Keep the six field filters below the title: Tag include/exclude, PID, TID, word include/exclude.

## 3. Filter Semantics

There is one current result expression:

```text
level filter ∩ field filters ∩ optional marked-only filter
```

Definitions:

- `levels`: bitmask for `V/D/I/W/E/F`.
- `markedOnly`: boolean, true when `仅标记` is enabled.
- Field filters remain the existing Tag/PID/TID/keyword include/exclude fields.

Behavior:

- Level chips are independent toggles.
- Level selection may be empty. Empty level selection produces 0 current result rows.
- `全部` sets the level bitmask to all six levels.
- `全部` does not clear field filters and does not turn off `仅标记`.
- `E + F` is the replacement for the old `错误` shortcut.
- `仅标记` is serial: it returns only marked rows inside the current level and field-filter result.
- Double-clicking a row still toggles its marked state.
- If `仅标记` is enabled and a visible row is unmarked, that row disappears from the current result after the result refreshes.

## 4. Navigation And Status

The frontend no longer exposes manual `全部视图 / 过滤视图 / 书签视图 / 错误视图` buttons. The table always renders the current result.

Recommended status bar copy:

```text
已加载 N 行 | 当前结果 M 行 | 索引 P% | 当前 第 X 行 | UTF-8 | logcat · threadtime
```

When `markedOnly` is active, include a compact `仅标记` status hint.

Bookmark keyboard navigation:

- `F2` and `F3` navigate among marked rows inside the current result.
- If no marked row exists in the current result, navigation does not move.
- This behavior applies whether `仅标记` is on or off.

## 5. Minimap

The minimap follows the current result set, not the whole source file.

Coordinate rules:

- Minimap height represents the current result range.
- Clicking the minimap jumps to the corresponding relative position inside the current result.
- The viewport rectangle is also relative to the current result.

Color rules:

- Left blue marks represent marked rows inside the current result.
- Right red marks represent `E/F` rows inside the current result.
- Red/blue mark length is based on actual hit buckets or row counts.
- Dense adjacent hits visually merge into a continuous strip.
- Sparse hits remain short, discrete marks.
- If the current result is `E + F` and many rows are present, the right side should look like a near-continuous red strip.
- If `仅标记` is enabled and many rows are present, the left side should look like a near-continuous blue strip.
- For small result sets, strips remain proportional to actual content and must not be forced to full height.

## 6. Architecture

The backend remains UI-independent.

`logcore` should represent the current result as hit source line numbers, not copied row text. The marked-only condition is a filter stage over line numbers/bookmarks. The frontend still calls `get_rows(view, start, count)` or an equivalent bounded command to fetch only visible windows. The row count limit remains capped by the Tauri boundary.

Implementation may keep existing `RowsView::Bookmarks` and `RowsView::Errors` internally for compatibility during migration, but the UI should no longer expose them as primary modes. The primary table path is the current filter result.

Recommended state shape:

- `filter.levels`
- `filter.markedOnly`
- existing field filters
- a result revision that invalidates the virtual table cache when backend results change

## 7. Error Handling

- Invalid regex behavior remains unchanged: the backend returns an error and the previous valid result remains visible until the user fixes the filter.
- Empty level selection is valid and returns 0 rows.
- `仅标记` with no matching marked rows is valid and returns 0 rows.
- Minimap with 0 current result rows renders no marks and keeps an empty/neutral viewport state.

## 8. Testing

Engine tests:

- Empty level bitmask returns 0 rows.
- `全部` behavior restores all levels without clearing field filters or `markedOnly`.
- `E + F` returns the same rows as the previous error/fatal behavior.
- `markedOnly` intersects correctly with level and field filters.
- Unmarking a row removes it from a `markedOnly` result.

Frontend/state tests or headless checks:

- The lower view buttons are absent.
- `全部` and level chips are visually separated.
- `仅标记` includes a bookmark icon and toggles `markedOnly`.
- Filter result changes invalidate the virtual table cache.
- The table continues fetching only bounded visible windows.
- `F2/F3` navigate only within marked rows in the current result.

Minimap tests:

- Minimap buckets are computed from the current result, not the source file.
- `E + F` dense results render a near-continuous right red strip.
- `markedOnly` dense results render a near-continuous left blue strip.
- Sparse result sets render proportional short marks.
- Empty current result renders no marks.
