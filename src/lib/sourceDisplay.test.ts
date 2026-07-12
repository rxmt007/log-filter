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
