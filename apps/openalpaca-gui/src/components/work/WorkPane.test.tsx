/**
 * The cross-chunk seam: `WorkPane` must be droppable into the chat aside as
 * `<WorkPane {...workPaneSlotProps} />`, so this exercises that exact call
 * shape as well as the pane's own chrome.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Task } from "@/lib/api/types";
import { qk } from "@/lib/query-keys";
import type { WorkPaneSlotProps } from "@/views/chat/WorkPaneSlot";
import { useUiStore } from "@/stores/ui";

import { WorkPane } from "./WorkPane";

function renderPane(
  props: Partial<WorkPaneSlotProps> = {},
  tasks: Task[] | null = null,
) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  // The pane's own list read (`limit` 60), seeded fresh so it does not refetch.
  if (tasks !== null) client.setQueryData(qk.tasks.list({ limit: 60 }), tasks);
  const slotProps: WorkPaneSlotProps = {
    blocked: false,
    blockedRunId: null,
    blockedOn: null,
    blockedHidden: 0,
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

  /**
   * The run card shows the pending tool's name alone, so the whole request's
   * hidden-character count rides the slot down to it with the name.
   */
  it("hands the blocked run's card the request's hidden-character count", () => {
    renderPane(
      {
        blocked: true,
        blockedRunId: "b41c8e02",
        blockedOn: "shell_execute",
        blockedHidden: 2,
      },
      [
        {
          id: "b41c8e02",
          title: "Audit the connector surface",
          description: null,
          status: "running",
          priority: 0,
          progress_current: null,
          progress_total: null,
          result_summary: null,
          created_by: "user",
          source_lane: "local:gui",
          created_at: "2026-08-31T14:22:41Z",
          updated_at: "2026-08-31T14:33:45Z",
          completed_at: null,
          state_version: 1,
        },
      ],
    );
    expect(screen.getByText(/Waiting on you:/).textContent).toBe(
      "Waiting on you: shell_execute — approve or deny it in the chat; its card shows 2 hidden characters.",
    );
  });

  it("states the failure rather than showing an empty pane", async () => {
    renderPane();
    expect(
      await screen.findByText("Could not reach the daemon."),
    ).toBeInTheDocument();
  });
});
