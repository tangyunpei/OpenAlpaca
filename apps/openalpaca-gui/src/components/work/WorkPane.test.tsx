/**
 * The cross-chunk seam: `WorkPane` must be droppable into the chat aside as
 * `<WorkPane {...workPaneSlotProps} />`, so this exercises that exact call
 * shape as well as the pane's own chrome.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { WorkPaneSlotProps } from "@/views/chat/WorkPaneSlot";
import { useUiStore } from "@/stores/ui";

import { WorkPane } from "./WorkPane";

function renderPane(props: Partial<WorkPaneSlotProps> = {}) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const slotProps: WorkPaneSlotProps = {
    blocked: false,
    blockedRunId: null,
    onFullView: vi.fn(),
    onCollapse: vi.fn(),
    ...props,
  };
  return {
    slotProps,
    ...render(
      <QueryClientProvider client={client}>
        {/* The literal call site the chat view uses. */}
        <WorkPane {...slotProps} />
      </QueryClientProvider>,
    ),
  };
}

beforeEach(() => {
  useUiStore.setState({ workOpen: true, view: "chat" });
});

describe("WorkPane (§3.18)", () => {
  it("renders the aside header with its counts and both controls", () => {
    renderPane();
    expect(screen.getByRole("heading", { name: "Work" })).toBeInTheDocument();
    expect(screen.getByText("0 active · 0 done")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Full view" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Collapse work pane" }),
    ).toBeInTheDocument();
  });

  it("prefers the caller's handlers over its own store actions", async () => {
    const { slotProps } = renderPane();
    await userEvent.click(screen.getByRole("button", { name: "Full view" }));
    await userEvent.click(
      screen.getByRole("button", { name: "Collapse work pane" }),
    );
    expect(slotProps.onFullView).toHaveBeenCalledOnce();
    expect(slotProps.onCollapse).toHaveBeenCalledOnce();
    // The store is untouched: the chat view owns the aside's geometry.
    expect(useUiStore.getState().workOpen).toBe(true);
  });

  it("falls back to the store when mounted with no handlers", async () => {
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    render(
      <QueryClientProvider client={client}>
        <WorkPane />
      </QueryClientProvider>,
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Collapse work pane" }),
    );
    expect(useUiStore.getState().workOpen).toBe(false);
  });

  it("states the failure rather than showing an empty pane", async () => {
    renderPane();
    expect(
      await screen.findByText("Could not reach the daemon."),
    ).toBeInTheDocument();
  });
});
