import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import { FilterBar } from "@/components/FilterBar";
import { DEFAULT_FILTER, LEVEL_BITS, useSession } from "@/store/session";

beforeEach(() => {
  useSession.setState({ filter: structuredClone(DEFAULT_FILTER) });
});

describe("FilterBar", () => {
  it("starts expanded and collapses to a single empty-state summary row", async () => {
    const user = userEvent.setup();
    render(<FilterBar />);

    const toggle = screen.getByRole("button", { name: "折叠过滤条件" });
    const tagFilterToggle = screen.getByRole("button", { name: "Tag 包含 过滤" });
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(tagFilterToggle).toHaveAttribute("aria-pressed", "false");
    expect(screen.getByPlaceholderText("*Manager")).toBeVisible();

    await user.click(tagFilterToggle);
    expect(tagFilterToggle).toHaveAttribute("aria-pressed", "true");
    await user.click(toggle);

    const collapsedToggle = screen.getByRole("button", {
      name: "展开过滤条件。当前配置：未启用过滤或高亮条件",
    });
    expect(collapsedToggle).toHaveAttribute("aria-expanded", "false");
    expect(
      document.getElementById(collapsedToggle.getAttribute("aria-controls") ?? ""),
    ).toHaveAttribute("hidden");
    expect(screen.queryByPlaceholderText("*Manager")).not.toBeInTheDocument();
    expect(screen.getByTestId("filter-summary")).toHaveTextContent(
      "当前配置：未启用过滤或高亮条件",
    );
  });

  it("keeps the collapsed summary synchronized with active filters", async () => {
    const user = userEvent.setup();
    const filter = structuredClone(DEFAULT_FILTER);
    filter.levels = LEVEL_BITS.E | LEVEL_BITS.F;
    filter.markedOnly = true;
    filter.wordInclude = { enabled: true, pattern: "network", regex: false };
    useSession.setState({ filter });

    render(<FilterBar />);
    await user.click(screen.getByRole("button", { name: "折叠过滤条件" }));

    const summary = screen.getByTestId("filter-summary");
    expect(summary).toHaveTextContent("3 项配置");
    expect(summary).toHaveTextContent("级别：E / F · 仅标记 · 内容包含：network");

    act(() => {
      useSession.setState({
        filter: {
          ...useSession.getState().filter,
          markedOnly: false,
        },
      });
    });

    expect(summary).toHaveTextContent("2 项配置");
    expect(summary).not.toHaveTextContent("仅标记");
  });
});
