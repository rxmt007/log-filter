import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Minimap } from "@/components/Minimap";
import { DEFAULT_FILTER, useSession } from "@/store/session";
import { TestResizeObserver } from "@/test/setup";
import type { MinimapData, Status } from "@/types";

const mocks = vi.hoisted(() => ({
  getMinimap: vi.fn(),
}));

vi.mock("@/lib/ipc", () => ({ getMinimap: mocks.getMinimap }));

const status: Status = {
  totalLines: 1_000,
  stableLines: 1_000,
  filteredLines: 40,
  bookmarkLines: 0,
  errorLines: 0,
  indexedBytes: 10_000,
  totalBytes: 10_000,
  indexing: false,
  generation: 1,
  analysisGeneration: 1,
  filterInputRevision: 1,
  appliedFilterInputRevision: 1,
  filterResultRevision: 1,
  decodeRevision: 0,
  sourceDataRevision: 1,
};

describe("Minimap filtered viewport", () => {
  afterEach(() => vi.useRealTimers());

  beforeEach(() => {
    mocks.getMinimap.mockReset();
    mocks.getMinimap.mockResolvedValue({
      bucketCount: 40,
      bookmarks: [],
      errors: [],
    });
    const appConfig = useSession.getState().appConfig;
    useSession.setState({
      status,
      tableScope: { kind: "results", view: "filtered" },
      filter: DEFAULT_FILTER,
      filterResultRevision: 1,
      appConfig: { ...appConfig, rowHeight: 20 },
      tableViewportHeight: 400,
      viewportResultIndex: 10,
      scrollRequest: null,
    });
  });

  it("uses visible rows and filtered result count for the viewport thumb", () => {
    const { container } = render(<Minimap />);
    const track = container.querySelector<HTMLElement>(".lf-minimap");
    expect(track).not.toBeNull();
    vi.spyOn(track!, "getBoundingClientRect").mockReturnValue({
      x: 0,
      y: 0,
      top: 0,
      right: 26,
      bottom: 500,
      left: 0,
      width: 26,
      height: 500,
      toJSON: () => ({}),
    });

    act(() => {
      const observer = [...TestResizeObserver.instances].find((instance) =>
        instance.observed.has(track!),
      );
      expect(observer).toBeDefined();
      observer!.emit(track!, 26, 500);
    });

    const viewport = container.querySelector<HTMLElement>(".lf-minimap-viewport");
    expect(viewport).not.toBeNull();
    expect(viewport).toHaveStyle({ height: "250px", top: "125px" });
  });

  it("invalidates an old filter response before refreshing the current context", async () => {
    vi.useFakeTimers();
    let resolveOld: ((data: MinimapData) => void) | null = null;
    let resolveCurrent: ((data: MinimapData) => void) | null = null;
    mocks.getMinimap
      .mockReturnValueOnce(
        new Promise<MinimapData>((resolve) => {
          resolveOld = resolve;
        }),
      )
      .mockReturnValueOnce(
        new Promise<MinimapData>((resolve) => {
          resolveCurrent = resolve;
        }),
      );

    const { container } = render(<Minimap />);
    await act(async () => {
      vi.advanceTimersByTime(250);
    });
    expect(mocks.getMinimap).toHaveBeenCalledTimes(1);

    act(() => {
      useSession.setState({
        status: {
          ...status,
          filteredLines: 4,
          appliedFilterInputRevision: 2,
          filterResultRevision: 2,
        },
        filterResultRevision: 2,
      });
    });
    expect(mocks.getMinimap).toHaveBeenCalledTimes(1);

    await act(async () => {
      resolveOld?.({
        bucketCount: 40,
        bookmarks: [],
        errors: [{ bucket: 0, count: 1 }],
      });
      await Promise.resolve();
    });
    expect(container.querySelectorAll(".lf-minimap-error")).toHaveLength(0);

    await act(async () => {
      vi.advanceTimersByTime(250);
    });
    expect(mocks.getMinimap).toHaveBeenCalledTimes(2);

    await act(async () => {
      resolveCurrent?.({
        bucketCount: 4,
        bookmarks: [],
        errors: [{ bucket: 3, count: 1 }],
      });
      await Promise.resolve();
    });

    const errors = container.querySelectorAll<HTMLElement>(".lf-minimap-error");
    expect(errors).toHaveLength(1);
    expect(errors[0]).toHaveStyle({ top: "75%", height: "25%" });
  });

  it("coalesces continuous appends behind one slow request and performs one trailing refresh", async () => {
    vi.useFakeTimers();
    let resolveSlow: ((data: MinimapData) => void) | null = null;
    mocks.getMinimap
      .mockReturnValueOnce(
        new Promise<MinimapData>((resolve) => {
          resolveSlow = resolve;
        }),
      )
      .mockResolvedValue({
        bucketCount: 45,
        bookmarks: [],
        errors: [{ bucket: 44, count: 1 }],
      });

    const { container } = render(<Minimap />);
    await act(async () => {
      vi.advanceTimersByTime(250);
    });
    expect(mocks.getMinimap).toHaveBeenCalledTimes(1);

    for (let revision = 2; revision <= 6; revision += 1) {
      act(() => {
        useSession.setState({
          status: {
            ...status,
            filteredLines: 39 + revision,
            filterResultRevision: revision,
          },
          filterResultRevision: revision,
        });
      });
      await act(async () => {
        vi.advanceTimersByTime(250);
      });
    }
    expect(mocks.getMinimap).toHaveBeenCalledTimes(1);

    await act(async () => {
      resolveSlow?.({
        bucketCount: 40,
        bookmarks: [],
        errors: [{ bucket: 0, count: 1 }],
      });
      await Promise.resolve();
    });
    expect(container.querySelectorAll(".lf-minimap-error")).toHaveLength(1);

    await act(async () => {
      vi.advanceTimersByTime(0);
    });
    expect(mocks.getMinimap).toHaveBeenCalledTimes(2);

    await act(async () => {
      await Promise.resolve();
    });
    expect(mocks.getMinimap).toHaveBeenCalledTimes(2);
  });

  it("exposes scrollbar semantics and keyboard navigation", async () => {
    render(<Minimap />);

    const track = screen.getByRole("scrollbar", { name: "日志小地图" });
    expect(track).toHaveAttribute("aria-controls", "lf-log-table-scroll");
    expect(track).toHaveAttribute("aria-valuemin", "0");
    expect(track).toHaveAttribute("aria-valuemax", "20");
    expect(track).toHaveAttribute("aria-valuenow", "10");

    fireEvent.keyDown(track, { key: "End" });

    await waitFor(() =>
      expect(useSession.getState().scrollRequest).toMatchObject({
        index: 20,
        align: "start",
        reason: "minimap",
      }),
    );
  });
});
