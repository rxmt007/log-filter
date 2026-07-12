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
