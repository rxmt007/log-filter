import { useRef, useState } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ProblemExportDialog } from "@/components/ProblemExportDialog";
import type { AnalysisToken, ProblemOccurrence } from "@/types";

const mocks = vi.hoisted(() => ({
  save: vi.fn(),
  exportProblemLogs: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  save: mocks.save,
}));

vi.mock("@/lib/ipc", () => ({
  exportProblemLogs: mocks.exportProblemLogs,
}));

const analysisToken: AnalysisToken = {
  sessionGeneration: 7,
  analysisGeneration: 11,
};

const occurrence: ProblemOccurrence = {
  eventId: 23,
  groupId: 5,
  kind: "java-crash",
  startLine: 120,
  endLine: 128,
  anchorLine: 121,
  timestamp: "07-26 12:00:00.000",
  pid: 4242,
  processInstanceId: 9,
  evidenceFlags: ["primary"],
  outcomeFlags: [],
  boundaryFlags: [],
};

beforeEach(() => {
  mocks.save.mockReset().mockResolvedValue("/tmp/problem.log");
  mocks.exportProblemLogs.mockReset().mockResolvedValue({
    writtenLines: 109,
    writtenBytes: 4096,
    cancelled: false,
  });
});

describe("ProblemExportDialog", () => {
  it("exports either the exact event range or a bounded ±50-line raw context", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(
      <ProblemExportDialog
        analysisToken={analysisToken}
        occurrence={occurrence}
        onClose={onClose}
      />,
    );

    expect(screen.getByText("事件范围：第 120–128 行")).toBeInTheDocument();
    expect(screen.getByText("上下文：事件范围外各增加 50 行")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "±50 行上下文" }));
    await user.click(screen.getByRole("button", { name: "选择位置并导出" }));

    await waitFor(() =>
      expect(mocks.exportProblemLogs).toHaveBeenCalledWith({
        eventId: 23,
        expectedAnalysisToken: analysisToken,
        mode: "context",
        radius: 50,
        path: "/tmp/problem.log",
      }),
    );
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("traps keyboard focus, closes on Escape, and restores the invoking control", async () => {
    const user = userEvent.setup();

    function Harness() {
      const trigger = useRef<HTMLButtonElement>(null);
      const [open, setOpen] = useState(false);
      return (
        <>
          <button ref={trigger} type="button" onClick={() => setOpen(true)}>
            打开事件导出
          </button>
          {open ? (
            <ProblemExportDialog
              analysisToken={analysisToken}
              occurrence={occurrence}
              returnFocus={trigger.current}
              onClose={() => setOpen(false)}
            />
          ) : null}
        </>
      );
    }

    render(<Harness />);
    const trigger = screen.getByRole("button", { name: "打开事件导出" });
    await user.click(trigger);
    const eventRange = screen.getByRole("button", { name: "事件范围" });
    await waitFor(() => expect(eventRange).toHaveFocus());

    await user.keyboard("{Shift>}{Tab}{/Shift}");
    expect(screen.getByRole("button", { name: "选择位置并导出" })).toHaveFocus();

    await user.keyboard("{Escape}");
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(screen.queryByRole("dialog", { name: "导出故障事件原始日志" })).toBeNull();
  });
});
