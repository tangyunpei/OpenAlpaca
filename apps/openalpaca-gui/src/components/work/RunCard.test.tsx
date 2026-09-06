import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { resetConnection } from "@/lib/connection";
import type { TaskTimeline } from "@/lib/api/tasks";

import { RunCard } from "./RunCard";
import type { Run } from "./run-model";
import { useRunController } from "./useRunController";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => ({
    baseUrl: "http://127.0.0.1:9999",
    token: "tok en/+",
    instanceId: "7f3a1122",
  })),
}));

const TIMELINE: TaskTimeline = {
  task_id: "b41c8e02",
  started_at: "2026-08-31T14:22:41Z",
  now: "2026-08-31T14:32:41Z",
  completed_at: null,
  lanes: [],
};

const run = (patch: Partial<Run> = {}): Run => ({
  id: "b41c8e02",
  title: "Audit the connector surface",
  status: "running",
  meta: "11m 04s · 5/8 steps",
  started: "14:22:41",
  stamp: null,
  note: "cargo tree is waiting on you",
  laneKey: "local:gui",
  artifactCount: 0,
  artifacts: [],
  finishedAt: null,
  costUsd: null,
  subagentCount: null,
  steerable: true,
  startedElsewhere: false,
  ...patch,
});

const card = (
  patch: Partial<Run> = {},
  props: Partial<Parameters<typeof RunCard>[0]> = {},
) =>
  render(
    <RunCard
      run={run(patch)}
      timeline={TIMELINE}
      onAction={vi.fn()}
      {...props}
    />,
  );

describe("RunCard (§3.19)", () => {
  it("raises a running card and flattens a queued one", () => {
    const { container, unmount } = card({ status: "running" });
    expect(container.querySelector("article")?.className).toContain(
      "bg-raised",
    );
    unmount();

    const queued = card({ status: "queued" });
    expect(queued.container.querySelector("article")?.className).toContain(
      "bg-inactive",
    );
  });

  // On a *running* card all five are live. `Start now` takes Pause's slot only
  // on a queued run (§3.19).
  it("shows the five live controls, all enabled", () => {
    card({ status: "running" });
    expect(screen.getByRole("button", { name: "Pause" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Steer" })).toBeEnabled();
    expect(
      screen.getByRole("button", { name: "Queue follow-up" }),
    ).toBeEnabled();
    expect(screen.getByRole("button", { name: "Jump to chat" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeEnabled();
  });

  /**
   * R40 — a run the daemon would refuse a steer for (another channel's run,
   * or one with no live workflow) renders `Steer` disabled with the reason,
   * instead of a button that answers 404 the first time it is used.
   */
  it("disables Steer, with the reason, on a run it cannot steer", () => {
    card({ status: "running", steerable: false, startedElsewhere: true });
    const steer = screen.getByRole("button", { name: "Steer" });
    expect(steer).toBeDisabled();
    expect(steer.getAttribute("title")).toMatch(/another channel/i);
    // Not a missing API — it stays out of the gap footnote.
    expect(screen.queryByText(/Steer —/)).not.toBeInTheDocument();
  });

  // GAP-06 closed: the action route dispatches the queued row under its own
  // id (D5), so the control is live and has nothing to apologise for.
  it("offers a live `Start now` on a queued run", () => {
    card({ status: "queued" });
    const start = screen.getByRole("button", { name: "Start now" });
    expect(start).toBeEnabled();
    expect(start).not.toHaveAttribute("title");
  });

  it("replaces the bar with a note and a live Re-run when terminal", () => {
    card({ status: "done", note: "wrote 2 files" });
    expect(screen.getByText("wrote 2 files")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Re-run" })).toBeEnabled();
    expect(
      screen.queryByRole("button", { name: "Cancel" }),
    ).not.toBeInTheDocument();
  });

  it("colours the note dot red only while this run is blocked (§4.4)", () => {
    const { container, unmount } = card(
      { status: "running" },
      { blocked: true },
    );
    expect(container.querySelector(".bg-red")).not.toBeNull();
    unmount();

    const resolved = card({ status: "running" }, { blocked: false });
    expect(resolved.container.querySelector(".bg-red")).toBeNull();
    expect(resolved.container.querySelector(".bg-green")).not.toBeNull();
  });

  it("hides the parallel-work block on a run that is not running or paused", () => {
    card({ status: "queued" });
    expect(screen.queryByText("Parallel work")).not.toBeInTheDocument();
  });

  it("lists the files the outcome reported and names the missing API", () => {
    card(
      {
        artifactCount: 2,
        artifacts: [
          { id: null, name: "findings.md", kind: "md", stamp: "14:31" },
          { id: null, name: "audit.csv", kind: "table", stamp: null },
        ],
      },
      { filesNote: "Artifact API not yet available" },
    );
    expect(screen.getByText("Files · 2")).toBeInTheDocument();
    expect(screen.getByText("findings.md")).toBeInTheDocument();
    expect(
      screen.getByText("Artifact API not yet available"),
    ).toBeInTheDocument();
  });

  it("does not make a file row clickable when it has no id", () => {
    card({
      artifactCount: 1,
      artifacts: [{ id: null, name: "findings.md", kind: "md", stamp: null }],
    });
    expect(
      screen.queryByRole("button", { name: /findings\.md/ }),
    ).not.toBeInTheDocument();
  });

  it("reports the action and the run it belongs to", async () => {
    const onAction = vi.fn();
    card({ status: "running" }, { onAction });
    await userEvent.click(screen.getByRole("button", { name: "Pause" }));
    expect(onAction).toHaveBeenCalledWith(
      "pause",
      expect.objectContaining({ id: "b41c8e02" }),
    );
  });
});

/**
 * `Re-run` is the only live control on a terminal card, and it is a *launch*:
 * every extra click is a real `POST /v1/tasks/{id}/rerun` → a real `201` → a
 * real lead agent and real spend. `rerun` has no server-side idempotency (nor
 * should it — two re-runs of one goal is a legitimate request), so the in-flight
 * guard has to be the button.
 */
describe("RunCard — Re-run in flight", () => {
  it("disables Re-run while the controller says this run is busy", async () => {
    const onAction = vi.fn();
    card({ status: "done" }, { onAction, busy: "rerun" });

    const button = screen.getByRole("button", { name: "Re-run" });
    expect(button).toBeDisabled();
    await userEvent.click(button);
    expect(onAction).not.toHaveBeenCalled();
  });

  it("does not POST twice when the user double-clicks", async () => {
    resetConnection();
    const deferred: { settle: () => void } = { settle: () => {} };
    const inFlight = new Promise<void>((resolve) => {
      deferred.settle = resolve;
    });
    const fetchMock = vi.fn(async () => {
      await inFlight;
      return new Response(
        JSON.stringify({
          task_id: "task-2",
          source_task_id: "b41c8e02",
          title: "Audit the connector surface",
          status: "queued",
        }),
        { status: 201 },
      );
    });
    vi.stubGlobal("fetch", fetchMock);

    // The real wiring: the controller owns `busy`, the card renders it.
    function Wired() {
      const controller = useRunController();
      const r = run({ status: "done" });
      return (
        <RunCard
          run={r}
          timeline={TIMELINE}
          busy={controller.busyFor(r.id)}
          onAction={controller.perform}
        />
      );
    }
    render(
      <QueryClientProvider
        client={
          new QueryClient({ defaultOptions: { queries: { retry: false } } })
        }
      >
        <Wired />
      </QueryClientProvider>,
    );

    const button = screen.getByRole("button", { name: "Re-run" });
    await userEvent.click(button);
    await userEvent.click(button);

    expect(button).toBeDisabled();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    deferred.settle();
  });
});
