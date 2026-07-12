import { describe, expect, it } from "vitest";
import { fileNameFromPath, rememberRecentFile } from "@/lib/recent";

describe("recent file helpers", () => {
  it("moves opened files to the front and keeps at most ten entries", () => {
    const files = Array.from({ length: 11 }, (_, index) => `/tmp/${index}.log`);

    expect(rememberRecentFile(files, "/tmp/3.log")).toEqual([
      "/tmp/3.log",
      "/tmp/0.log",
      "/tmp/1.log",
      "/tmp/2.log",
      "/tmp/4.log",
      "/tmp/5.log",
      "/tmp/6.log",
      "/tmp/7.log",
      "/tmp/8.log",
      "/tmp/9.log",
    ]);
  });

  it("extracts names from unix and windows paths", () => {
    expect(fileNameFromPath("/tmp/app.log")).toBe("app.log");
    expect(fileNameFromPath("C:\\logs\\device.txt")).toBe("device.txt");
  });
});
