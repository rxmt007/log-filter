import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { StatusBar } from "@/components/StatusBar";
import { useSession } from "@/store/session";

describe("StatusBar problem context", () => {
  beforeEach(() => {
    useSession.setState(useSession.getInitialState(), true);
    useSession.setState({
      status: {
        ...useSession.getState().status,
        totalLines: 100,
        stableLines: 100,
        filteredLines: 40,
        indexedBytes: 1_000,
        totalBytes: 1_000,
      },
      tableScope: {
        kind: "problem-context",
        occurrence: {
          eventId: 8,
          groupId: 2,
          startLine: 50,
          endLine: 55,
          anchorLine: 52,
        },
        eventRange: { startLine: 50, endLine: 55 },
        contextRange: { startLine: 20, endLine: 80 },
        returnPoint: {
          viewportLine: 12,
          selectedLine: 14,
          filterInputRevision: 3,
        },
      },
    });
  });

  it("labels the active scope as temporary unfiltered context", () => {
    render(<StatusBar />);

    expect(screen.getByText(/临时未过滤上下文/)).toHaveTextContent(
      "临时未过滤上下文 · 事件第 50–55 行",
    );
    expect(screen.queryByText(/临时原始上下文/)).not.toBeInTheDocument();
  });
});
