# UI Control System And Live Follow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the confirmed UI control-system and adb live-follow spec, replacing system selects, removing source/drag ambiguity, adding command presets, and verifying the result.

**Architecture:** Keep `logcore` UI-independent by modeling/validating logcat commands in `logcore::adb`, expose normalized command config through Tauri DTOs, and implement all visual controls in React. Frontend pure helpers own path display, command UI parsing mirrors, and menu state; Tauri/Rust remains the authoritative guard before spawning adb.

**Tech Stack:** Rust `logcore` + Tauri v2 commands, Vite React TypeScript, zustand, Vitest, Tailwind v4 CSS-first, lucide icons.

---

## File Structure

- Create `src/components/ui/dropdown.tsx`: shared custom dropdown/menu/select/color controls.
- Create `src/lib/logcatCommand.ts`: frontend command parsing, preset normalization, and command labels.
- Create `src/lib/sourceDisplay.ts`: statusbar path compaction with `~/` and middle ellipsis.
- Modify `crates/logcore/src/adb.rs`: add `LogcatCommand`, parse/normalize command strings, default presets, preset normalization.
- Modify `crates/logcore/src/config.rs`: add `current_command` and `command_presets`, preserve old `command_buffers` compatibility.
- Modify `src-tauri/src/dto.rs`: expose new command fields and normalize old DTOs.
- Modify `src-tauri/src/commands.rs`: parse `StartLogcatRequest.command` into structured buffers before spawn.
- Modify `src/types.ts`, `src/lib/ipc.ts`, `src/store/session.ts`: update TS contract and tail-follow actions.
- Modify `src/components/Toolbar.tsx`: remove source dropdown, add device dropdown, command combobox, open/recent menu.
- Modify `src/components/ToolDialogs.tsx`: replace all visible selects with custom fields.
- Modify `src/components/StatusBar.tsx`: render `adb · serial` or `file · ~/path/.../file.log` at bottom right.
- Modify `src/components/LogTable.tsx`, `src/components/Minimap.tsx`, `src/App.tsx`: remove drag/drop and implement explicit live-follow pause/resume semantics.
- Modify `src/index.css`: menu/select/combobox styles and brighter light hover tokens.
- Add/extend tests in `src/lib/*.test.ts`, `src/store/session.test.ts`, `crates/logcore/src/{adb.rs,config.rs}`, and `src-tauri/src/dto.rs`.

---

### Task 1: Rust Logcat Command And Config Contract

**Files:**
- Modify: `crates/logcore/src/adb.rs`
- Modify: `crates/logcore/src/config.rs`
- Modify: `src-tauri/src/dto.rs`
- Modify: `src-tauri/src/commands.rs`
- Modify: `src/types.ts`

- [ ] **Step 1: Write failing Rust tests for command parsing**

Add tests in `crates/logcore/src/adb.rs`:

```rust
#[test]
fn parses_supported_threadtime_logcat_commands() {
    let command = LogcatCommand::parse("logcat -v threadtime -b radio").unwrap();
    assert_eq!(command.buffer, LogcatBuffer::Radio);
    assert_eq!(command.normalized(), "logcat -v threadtime -b radio");

    let default_buffer = LogcatCommand::parse("logcat -v threadtime").unwrap();
    assert_eq!(default_buffer.buffer, LogcatBuffer::Main);
    assert_eq!(default_buffer.normalized(), "logcat -v threadtime -b main");
}

#[test]
fn rejects_unsupported_or_shell_like_logcat_commands() {
    for input in [
        "logcat -v time",
        "logcat -v threadtime -b kernel",
        "adb logcat -v threadtime",
        "logcat -v threadtime && rm -rf /",
        "logcat -v threadtime | grep foo",
        "shell logcat -v threadtime",
    ] {
        assert!(LogcatCommand::parse(input).is_err(), "{input}");
    }
}

#[test]
fn normalizes_command_presets_with_defaults_and_limit() {
    let custom = vec![
        "logcat -v threadtime -b radio".to_string(),
        "logcat -v threadtime -b radio".to_string(),
        "logcat -v threadtime -b kernel".to_string(),
    ];
    let presets = normalize_command_presets(custom);
    assert!(presets.contains(&"logcat -v threadtime -b main".to_string()));
    assert!(presets.contains(&"logcat -v threadtime -b radio".to_string()));
    assert_eq!(
        presets
            .iter()
            .filter(|item| item.as_str() == "logcat -v threadtime -b radio")
            .count(),
        1
    );
    assert!(presets.len() <= 25);
}
```

- [ ] **Step 2: Run failing Rust test**

Run: `cargo test -p logcore adb::tests::parses_supported_threadtime_logcat_commands adb::tests::rejects_unsupported_or_shell_like_logcat_commands adb::tests::normalizes_command_presets_with_defaults_and_limit`

Expected: FAIL because `LogcatCommand` and preset helpers do not exist.

- [ ] **Step 3: Implement Rust command parsing and config fields**

Add in `crates/logcore/src/adb.rs`:

```rust
pub const DEFAULT_LOGCAT_COMMANDS: [&str; 5] = [
    "logcat -v threadtime -b main",
    "logcat -v threadtime -b system",
    "logcat -v threadtime -b radio",
    "logcat -v threadtime -b events",
    "logcat -v threadtime -b crash",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogcatCommand {
    pub buffer: LogcatBuffer,
}

impl LogcatCommand {
    pub fn parse(input: &str) -> Result<Self, String> {
        if input.contains('|')
            || input.contains('&')
            || input.contains(';')
            || input.contains('>')
            || input.contains('<')
        {
            return Err("compound shell commands are not supported".to_string());
        }
        let tokens: Vec<&str> = input.split_whitespace().collect();
        if tokens.is_empty() || tokens[0] != "logcat" {
            return Err("command must start with logcat".to_string());
        }
        let mut buffer = LogcatBuffer::Main;
        let mut saw_threadtime = false;
        let mut index = 1;
        while index < tokens.len() {
            match tokens[index] {
                "-v" => {
                    let value = tokens
                        .get(index + 1)
                        .ok_or_else(|| "-v requires a value".to_string())?;
                    if *value != "threadtime" {
                        return Err("only -v threadtime is supported".to_string());
                    }
                    saw_threadtime = true;
                    index += 2;
                }
                "-b" => {
                    let value = tokens
                        .get(index + 1)
                        .ok_or_else(|| "-b requires a buffer".to_string())?;
                    buffer = LogcatBuffer::try_from(*value)?;
                    index += 2;
                }
                other => return Err(format!("unsupported logcat argument: {other}")),
            }
        }
        if !saw_threadtime {
            return Err("only -v threadtime is supported".to_string());
        }
        Ok(Self { buffer })
    }

    pub fn normalized(&self) -> String {
        format!("logcat -v threadtime -b {}", self.buffer.as_arg())
    }
}

pub fn normalize_command_presets(presets: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = DEFAULT_LOGCAT_COMMANDS
        .iter()
        .map(|command| command.to_string())
        .collect();
    for preset in presets {
        if out.len() >= 25 {
            break;
        }
        if let Ok(command) = LogcatCommand::parse(&preset) {
            let normalized = command.normalized();
            if !out.contains(&normalized) {
                out.push(normalized);
            }
        }
    }
    out
}
```

Update `AppConfig` with:

```rust
#[serde(default = "default_current_command")]
pub current_command: String,
#[serde(default)]
pub command_presets: Vec<String>,
```

Normalize with `LogcatCommand::parse`, defaults, and old `command_buffers`.

- [ ] **Step 4: Run Rust tests green**

Run: `cargo test -p logcore adb::tests::parses_supported_threadtime_logcat_commands adb::tests::rejects_unsupported_or_shell_like_logcat_commands adb::tests::normalizes_command_presets_with_defaults_and_limit`

Expected: PASS.

- [ ] **Step 5: Add DTO tests for backward compatibility**

Extend `src-tauri/src/dto.rs` test `app_config_dto_rejects_unknown_theme_and_normalizes_numbers` or add:

```rust
#[test]
fn app_config_dto_round_trips_command_presets() {
    let config = logcore::config::AppConfig {
        current_command: "logcat -v threadtime -b radio".to_string(),
        command_presets: vec!["logcat -v threadtime -b radio".to_string()],
        ..Default::default()
    }
    .normalized();
    let dto = AppConfigDto::from_config(config.clone(), PathBuf::new());
    assert_eq!(dto.current_command, "logcat -v threadtime -b radio");
    assert!(dto
        .command_presets
        .contains(&"logcat -v threadtime -b radio".to_string()));

    let converted = logcore::config::AppConfig::try_from(dto).unwrap();
    assert_eq!(converted.current_command, config.current_command);
}
```

- [ ] **Step 6: Run Tauri DTO tests**

Run: `cargo test -p log-filter dto::tests::app_config_dto_round_trips_command_presets`

Expected: PASS after DTO fields are wired.

---

### Task 2: Frontend Pure Helpers And Store State

**Files:**
- Create: `src/lib/logcatCommand.ts`
- Test: `src/lib/logcatCommand.test.ts`
- Create: `src/lib/sourceDisplay.ts`
- Test: `src/lib/sourceDisplay.test.ts`
- Modify: `src/store/session.ts`
- Test: `src/store/session.test.ts`
- Modify: `src/types.ts`

- [ ] **Step 1: Write failing frontend helper tests**

Create `src/lib/logcatCommand.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import {
  DEFAULT_LOGCAT_COMMANDS,
  normalizeCommandPresets,
  parseLogcatCommand,
} from "@/lib/logcatCommand";

describe("logcat command helpers", () => {
  it("parses supported threadtime commands and defaults missing buffer to main", () => {
    expect(parseLogcatCommand("logcat -v threadtime -b radio")).toEqual({
      ok: true,
      buffer: "radio",
      normalized: "logcat -v threadtime -b radio",
    });
    expect(parseLogcatCommand("logcat -v threadtime")).toEqual({
      ok: true,
      buffer: "main",
      normalized: "logcat -v threadtime -b main",
    });
  });

  it("rejects unsupported shell-like commands", () => {
    for (const command of [
      "logcat -v time",
      "logcat -v threadtime -b kernel",
      "adb logcat -v threadtime",
      "logcat -v threadtime | grep foo",
      "logcat -v threadtime && rm -rf /",
    ]) {
      expect(parseLogcatCommand(command).ok).toBe(false);
    }
  });

  it("normalizes presets with defaults, de-duplication, and a custom limit", () => {
    const presets = normalizeCommandPresets([
      "logcat -v threadtime -b radio",
      "logcat -v threadtime -b radio",
      "logcat -v threadtime -b kernel",
    ]);
    expect(presets.slice(0, DEFAULT_LOGCAT_COMMANDS.length)).toEqual(DEFAULT_LOGCAT_COMMANDS);
    expect(presets.filter((item) => item === "logcat -v threadtime -b radio")).toHaveLength(1);
  });
});
```

Create `src/lib/sourceDisplay.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { compactSourcePath } from "@/lib/sourceDisplay";

describe("source display helpers", () => {
  it("uses tilde for home paths and preserves filename", () => {
    expect(
      compactSourcePath("/Users/alice/work_space_qa/log-filter/logs/demo.log", {
        homeDir: "/Users/alice",
        maxLength: 28,
      }).label,
    ).toBe("file · ~/.../logs/demo.log");
  });

  it("keeps full path in the title even when label is compacted", () => {
    const result = compactSourcePath("/var/tmp/logfilter/very/long/path/demo.log", {
      homeDir: "/Users/alice",
      maxLength: 24,
    });
    expect(result.label).toBe("file · /var/.../demo.log");
    expect(result.title).toBe("/var/tmp/logfilter/very/long/path/demo.log");
  });
});
```

- [ ] **Step 2: Run failing helper tests**

Run: `pnpm test src/lib/logcatCommand.test.ts src/lib/sourceDisplay.test.ts`

Expected: FAIL because helper files do not exist.

- [ ] **Step 3: Implement helpers and TS types**

Implement `src/lib/logcatCommand.ts` with parser/normalizer mirroring Rust. Implement `src/lib/sourceDisplay.ts` with `~/` conversion and middle ellipsis. Update `AppConfig` with `currentCommand` and `commandPresets`, and `StartLogcatRequest` with `command`.

- [ ] **Step 4: Run helper tests green**

Run: `pnpm test src/lib/logcatCommand.test.ts src/lib/sourceDisplay.test.ts`

Expected: PASS.

- [ ] **Step 5: Add store tests for tail-follow actions**

Extend `src/store/session.test.ts`:

```ts
it("pauses and restores tail following through explicit actions", () => {
  useSession.setState({ tailFollowing: true });
  useSession.getState().pauseTailFollowing("row");
  expect(useSession.getState().tailFollowing).toBe(false);

  useSession.getState().setTailFollowingFromViewport(false, "program");
  expect(useSession.getState().tailFollowing).toBe(false);

  useSession.getState().setTailFollowingFromViewport(true, "user");
  expect(useSession.getState().tailFollowing).toBe(true);
});

it("initializes adb sessions with tail following enabled and file sessions disabled", () => {
  useSession.getState().beginSession(status, "/tmp/logcat.log", "adb");
  expect(useSession.getState().tailFollowing).toBe(true);

  useSession.getState().beginSession(status, "/tmp/file.log", "file");
  expect(useSession.getState().tailFollowing).toBe(false);
});
```

- [ ] **Step 6: Run store tests green**

Run: `pnpm test src/store/session.test.ts`

Expected: PASS after store actions are implemented.

---

### Task 3: Custom Dropdown Components

**Files:**
- Create: `src/components/ui/dropdown.tsx`
- Modify: `src/index.css`

- [ ] **Step 1: Create reusable controls**

Add `DropdownMenu`, `DropdownItem`, `SelectField`, and `ColorSelect` in `src/components/ui/dropdown.tsx`. Controls must support click outside, Esc close, checked item, disabled item, group labels, icon/leading slot, and `aria-expanded`.

- [ ] **Step 2: Add CSS**

Add `.lf-dropdown-*`, `.lf-select-field-*`, and `.lf-color-*` classes in `src/index.css` using existing tokens. Include clearer light-mode hover via `--lf-hover-strong` or equivalent.

- [ ] **Step 3: Run typecheck**

Run: `pnpm typecheck`

Expected: PASS.

---

### Task 4: Toolbar Information Architecture And Command Combobox

**Files:**
- Modify: `src/components/Toolbar.tsx`
- Modify: `src/store/session.ts`
- Modify: `src/lib/ipc.ts`
- Modify: `src/types.ts`

- [ ] **Step 1: Replace source select and top native selects**

Remove source dropdown JSX. Use custom device dropdown and `CommandCombobox`. Keep open file as the only direct file opener in the toolbar; recent files live in an open-file dropdown/menu.

- [ ] **Step 2: Persist command presets**

When command changes or a new valid command is added, call `saveAppConfig` with `currentCommand` and normalized `commandPresets`. Continue selecting buffers from parsed command for backend compatibility until `StartLogcatRequest.command` is wired.

- [ ] **Step 3: Wire start request**

Call:

```ts
await startLogcat({
  deviceSerial: selectedDeviceSerial,
  command: currentCommand,
});
```

Do not send user input to shell; backend parses the command.

- [ ] **Step 4: Run toolbar-related typecheck**

Run: `pnpm typecheck`

Expected: PASS.

---

### Task 5: Dialog Select Replacement And Statusbar Source Display

**Files:**
- Modify: `src/components/ToolDialogs.tsx`
- Modify: `src/components/StatusBar.tsx`
- Modify: `src/components/LogTable.tsx`

- [ ] **Step 1: Replace dialog selects**

Use `SelectField` for export view, split mode, settings encoding. Use `ColorSelect` for highlight rule colors in `Toolbar.tsx`.

- [ ] **Step 2: Update statusbar**

Use `compactSourcePath(sourcePath, { homeDir })` for file mode. Use `adb · ${selectedDeviceSerial}` for adb mode. Put it on the far right with title tooltip.

- [ ] **Step 3: Update empty state copy**

Remove "拖入" copy. Keep buttons for open file and device capture.

- [ ] **Step 4: Run typecheck**

Run: `pnpm typecheck`

Expected: PASS.

---

### Task 6: Remove Drag-Drop And Tighten Tail-Follow

**Files:**
- Modify: `src/App.tsx`
- Modify: `src/components/LogTable.tsx`
- Modify: `src/components/Minimap.tsx`
- Modify: `src/store/session.ts`

- [ ] **Step 1: Remove global drag/drop open listeners**

Delete `dragover` and `drop` effect from `App.tsx`.

- [ ] **Step 2: Pause follow from user/navigation actions**

Call `pauseTailFollowing` on row click, search navigation, bookmark navigation, minimap navigation, and jump-to-line. `LogTable` should call `setTailFollowingFromViewport(isAtBottom, "user")` only for user scroll/wheel paths, not for programmatic scroll requests.

- [ ] **Step 3: Keep append behavior scoped to adb**

On `stream:append`, only auto-scroll when `sourceMode === "adb"` and `tailFollowing === true`.

- [ ] **Step 4: Run store and frontend tests**

Run: `pnpm test src/store/session.test.ts`

Expected: PASS.

---

### Task 7: Verification And Commit

**Files:**
- All modified files.

- [ ] **Step 1: Run required verification**

Run:

```bash
cargo test -p logcore
cargo build --workspace
pnpm typecheck
pnpm test
pnpm build
```

Expected: all commands exit 0.

- [ ] **Step 2: Run additional useful checks**

Run:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
pnpm lint
pnpm format
```

Expected: all commands exit 0.

- [ ] **Step 3: ADB dynamic smoke check if implementation touched live capture**

Run:

```bash
adb devices
serial=$(adb devices | awk 'NR > 1 && $2 == "device" { print $1; exit }')
test -n "$serial"
adb -s "$serial" logcat -d -t 500 -v threadtime >/tmp/logfilter-adb-threadtime-snapshot.log
wc -l /tmp/logfilter-adb-threadtime-snapshot.log
```

Expected: `serial` is dynamically discovered; snapshot command exits 0.

- [ ] **Step 4: Commit**

Run:

```bash
git status --short
git add docs/superpowers/plans/2026-07-02-ui-control-system-live-follow.md crates/logcore/src/adb.rs crates/logcore/src/config.rs src-tauri/src/dto.rs src-tauri/src/commands.rs src/types.ts src/lib/ipc.ts src/store/session.ts src/store/session.test.ts src/lib/logcatCommand.ts src/lib/logcatCommand.test.ts src/lib/sourceDisplay.ts src/lib/sourceDisplay.test.ts src/components/ui/dropdown.tsx src/components/Toolbar.tsx src/components/ToolDialogs.tsx src/components/StatusBar.tsx src/components/LogTable.tsx src/components/Minimap.tsx src/App.tsx src/index.css
git commit -m "feat: refine ui controls and live follow"
```

Expected: commit succeeds and working tree is clean.

---

## Self-Review

- Spec coverage: source dropdown removal, all visible selects, command combobox/presets, drag removal, hover, statusbar source display, and tail-follow are all assigned to tasks.
- Placeholder scan: no TBD/TODO placeholders remain; implementation steps name concrete files and commands.
- Type consistency: Rust uses `current_command`/`command_presets`; TS uses `currentCommand`/`commandPresets`; DTO bridges via camelCase.
