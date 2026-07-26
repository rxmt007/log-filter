import type { LogcatBuffer } from "@/types";

export const DEFAULT_LOGCAT_COMMANDS = [
  "logcat -v threadtime -b main",
  "logcat -v threadtime -b main -b system -b crash -b events",
  "logcat -v threadtime -b system",
  "logcat -v threadtime -b radio",
  "logcat -v threadtime -b events",
  "logcat -v threadtime -b crash",
] as const;

const LOGCAT_BUFFERS = new Set<LogcatBuffer>(["main", "system", "radio", "events", "crash"]);
const IMPLICIT_LOGCAT_BUFFERS: LogcatBuffer[] = ["main", "system", "crash"];

export type ParsedLogcatCommand =
  { ok: true; buffers: LogcatBuffer[]; normalized: string } | { ok: false; error: string };

export function parseLogcatCommand(input: string): ParsedLogcatCommand {
  if (/[|&;<>]/.test(input)) {
    return { ok: false, error: "不支持复合 shell 命令" };
  }
  const tokens = input.trim().split(/\s+/).filter(Boolean);
  if (tokens[0] !== "logcat") {
    return { ok: false, error: "命令必须以 logcat 开头" };
  }

  let sawThreadtime = false;
  const buffers: LogcatBuffer[] = [];
  for (let index = 1; index < tokens.length;) {
    const token = tokens[index];
    if (token === "-v") {
      const value = tokens[index + 1];
      if (value !== "threadtime") return { ok: false, error: "仅支持 -v threadtime" };
      sawThreadtime = true;
      index += 2;
      continue;
    }
    if (token === "-b") {
      const value = tokens[index + 1] as LogcatBuffer | undefined;
      if (!value || !LOGCAT_BUFFERS.has(value)) {
        return { ok: false, error: "缓冲区仅支持 main/system/radio/events/crash" };
      }
      if (!buffers.includes(value)) buffers.push(value);
      index += 2;
      continue;
    }
    return { ok: false, error: `不支持参数 ${token}` };
  }

  if (!sawThreadtime) return { ok: false, error: "仅支持 -v threadtime" };
  const normalizedBuffers = buffers.length ? buffers : [...IMPLICIT_LOGCAT_BUFFERS];
  return {
    ok: true,
    buffers: normalizedBuffers,
    normalized: `logcat -v threadtime ${normalizedBuffers
      .map((buffer) => `-b ${buffer}`)
      .join(" ")}`,
  };
}

export function normalizeCommandPresets(presets: string[]): string[] {
  const out: string[] = [...DEFAULT_LOGCAT_COMMANDS];
  for (const preset of presets) {
    if (out.length >= DEFAULT_LOGCAT_COMMANDS.length + 20) break;
    const parsed = parseLogcatCommand(preset);
    if (!parsed.ok || out.includes(parsed.normalized)) continue;
    out.push(parsed.normalized);
  }
  return out;
}
