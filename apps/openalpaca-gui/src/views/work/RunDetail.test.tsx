/**
 * The run detail's data-backed cards, over the real data layer.
 *
 * Phase 3 gave `Output` a typed source — `GET /v1/artifacts?task_id=` — where
 * it previously had only the run's free-form outcome blob. Phase 4 did the same
 * for `Event log`: `GET /v1/events/history?task_id=` replaced a filtered live
 * socket ring (GAP-10). Every edge is doubled and nothing else.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Run } from "@/components/work/run-model";
import { resetConnection } from "@/lib/connection";
import { useUiStore } from "@/stores/ui";

import { RunDetail } from "./RunDetail";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => ({
    baseUrl: "http://127.0.0.1:9999",
    token: "test-token",
    instanceId: "7f3a1122",
  })),
}));

const task = {
  id: "task-1",
  title: "connector audit",
  status: "completed",
  created_at: "2026-09-05T10:00:00Z",
  updated_at: "2026-09-05T10:30:00Z",
  completed_at: "2026-09-05T10:30:00Z",
  source_lane: "user:gui",
  artifact_count: 1,
  progress_current: null,
  progress_total: null,
  result_summary: null,
};

const artifact = {
  id: "art-1",
  name: "findings.md",
  kind: "markdown",
  mime_type: "text/markdown",
  size_bytes: 42,
  task_id: "task-1",
  task_title: "connector audit",
  agent_id: null,
  agent_template_id: "review_agent",
  version: 1,
  version_count: 1,
  summary: null,
  metadata: null,
  created_at: "2026-09-05T10:20:00Z",
  updated_at: "2026-09-05T10:20:00Z",
  origin: "produced",
  pinned: false,
  missing: false,
  path: "/tmp/findings.md",
  project_root: null,
  rel_path: "findings.md",
};

let artifactsReply: () => Response;
let eventsReply: () => Response;
let requested: string[] = [];

/** One persisted row, as `GET /v1/events/history` serves it. */
function row(
  id: number,
  event_type: string,
  detail: Record<string, unknown>,
  agent_id: string | null = null,
) {
  return {
    id,
    timestamp: "2026-09-05T10:20:00Z",
    agent_id,
    task_id: "task-1",
    event_type,
    detail,
  };
}

function json(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), { status });
}

beforeEach(() => {
  requested = [];
  resetConnection();
  useUiStore.setState({ view: "work", panelArtifactId: null });
  artifactsReply = () => json({ artifacts: [artifact], total: 1 });
  eventsReply = () => json({ events: [], next_before: null });
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: unknown) => {
      const url = String(input);
      requested.push(url);
      if (url.includes("/v1/events/history")) return eventsReply();
      if (url.includes("/v1/artifacts")) return artifactsReply();
      if (url.includes("/timeline")) {
        return json({
          task_id: "task-1",
          started_at: "2026-08-31T14:22:41Z",
          now: "2026-08-31T14:32:41Z",
          completed_at: null,
          lanes: [],
        });
      }
      if (url.includes("/v1/tasks/")) return json({ task });
      return json({ error: "not found" }, 404);
    }),
  );
});

function renderDetail(fallbackRun: Run | null = null) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return render(
    <QueryClientProvider client={client}>
      <RunDetail runId="task-1" fallbackRun={fallbackRun} onAction={vi.fn()} />
    </QueryClientProvider>,
  );
}

/** The Work list's own row for `task-1`, as `toRun` shapes it. */
const listRow: Run = {
  id: "task-1",
  title: "connector audit",
  status: "done",
  meta: "11m 04s",
  started: "10:00:00",
  stamp: "10:30",
  note: null,
  laneKey: "user:gui",
  artifactCount: 1,
  artifacts: [],
  finishedAt: new Date("2026-09-05T10:30:00Z"),
  costUsd: 0.41,
  subagentCount: 2,
  steerable: false,
  startedElsewhere: false,
};

describe("RunDetail — Output", () => {
  it("lists the run's own files and opens one in the side panel", async () => {
    const user = useAndRender();
    const row = await screen.findByRole("button", { name: /findings\.md/ });
    expect(requested.some((url) => url.includes("task_id=task-1"))).toBe(true);

    await user.click(row);
    expect(useUiStore.getState().panelArtifactId).toBe("art-1");
  });

  it("says why the files are missing rather than blaming the run", async () => {
    artifactsReply = () =>
      json({ error: { code: "DB_ERROR", message: "database is locked" } }, 500);
    renderDetail();

    await waitFor(() =>
      expect(screen.getByText(/database is locked/)).toBeInTheDocument(),
    );
    expect(screen.getByText(/reported 1 file/)).toBeInTheDocument();
  });
});

describe("RunDetail — Timeline", () => {
  it("says the timeline read failed rather than claiming no steps have run", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: unknown) => {
        const url = String(input);
        requested.push(url);
        if (url.includes("/v1/events/history")) return eventsReply();
        if (url.includes("/v1/artifacts")) return artifactsReply();
        if (url.includes("/timeline")) {
          return json(
            { error: { code: "DB_ERROR", message: "database is locked" } },
            500,
          );
        }
        if (url.includes("/v1/tasks/")) return json({ task });
        return json({ error: "not found" }, 404);
      }),
    );
    renderDetail();

    await waitFor(() =>
      expect(screen.getByText(/database is locked/)).toBeInTheDocument(),
    );
    expect(
      screen.getByText(/This run's timeline could not be loaded/),
    ).toBeInTheDocument();
    expect(screen.queryByText(/No steps have run yet/)).not.toBeInTheDocument();
  });
});

describe("RunDetail — Event log", () => {
  it("renders this run's persisted log, tool rows included (GAP-10)", async () => {
    eventsReply = () =>
      json({
        events: [
          row(12, "tool_executed", {
            tool_name: "shell_execute",
            success: true,
            task_id: "task-1",
          }),
          row(11, "artifact_written", {
            name: "findings.md",
            version: 1,
            task_id: "task-1",
          }),
          row(10, "workflow_started", { title: "connector audit" }),
        ],
        next_before: null,
      });
    renderDetail();

    // The card asked the server for *this* run rather than filtering a ring.
    await waitFor(() =>
      expect(
        requested.some(
          (url) =>
            url.includes("/v1/events/history") &&
            url.includes("task_id=task-1"),
        ),
      ).toBe(true),
    );

    // A tool row: the thing the live socket could never attribute to a run.
    expect(await screen.findByText("shell_execute")).toBeInTheDocument();
    expect(screen.getByText("tool")).toBeInTheDocument();
    expect(screen.getByText("findings.md · v1")).toBeInTheDocument();
    expect(screen.getByText("started · connector audit")).toBeInTheDocument();
    // …and no gap note, because there is no longer a gap.
    expect(
      screen.queryByText(/No events for this run yet/),
    ).not.toBeInTheDocument();
  });

  it("drops `dag_node_status`, which duplicates the span it mirrors", async () => {
    eventsReply = () =>
      json({
        events: [
          row(21, "subagent_span", {
            label: "review·1",
            state: "done",
            task_id: "task-1",
          }),
          row(20, "dag_node_status", {
            node_id: "node-1",
            status: "completed",
            task_id: "task-1",
          }),
        ],
        next_before: null,
      });
    renderDetail();

    expect(await screen.findByText("review·1 · done")).toBeInTheDocument();
    expect(screen.queryByText(/dag_node_status/)).not.toBeInTheDocument();
  });

  it("says the read failed rather than claiming the run is quiet", async () => {
    eventsReply = () =>
      json({ error: { code: "DB_ERROR", message: "database is locked" } }, 500);
    renderDetail();

    await waitFor(() =>
      expect(
        screen.getByText(/This run's event log could not be loaded/),
      ).toBeInTheDocument(),
    );
    expect(
      screen.queryByText(/No events for this run yet/),
    ).not.toBeInTheDocument();
  });
});

/** `userEvent.setup()` must run before render for its clipboard/pointer stubs. */
function useAndRender() {
  const user = userEvent.setup();
  renderDetail();
  return user;
}

/**
 * §3.26's terminal banner takes the same in-flight guard the live action bar
 * has. `Re-run` is a launch, not a transition: a second click is a second
 * `POST /v1/tasks/{id}/rerun` and a second lead agent.
 */
describe("RunDetail — Re-run in flight", () => {
  it("disables Re-run on the terminal banner while it is busy", async () => {
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false, gcTime: 0 } },
    });
    render(
      <QueryClientProvider client={client}>
        <RunDetail runId="task-1" busy="rerun" onAction={vi.fn()} />
      </QueryClientProvider>,
    );
    expect(
      await screen.findByRole("button", { name: "Re-run" }),
    ).toBeDisabled();
  });
});

/**
 * The terminal banner's status prop is computed from `run.status` at the
 * call site (RunDetail.tsx), independently of `TerminalBanner`'s own
 * `interrupted` text branch — a caller that never passes "interrupted"
 * through leaves that branch dead. §5.6b: an interrupted run is terminal,
 * not an error, and gets its own wording, not the cancelled copy.
 */
describe("RunDetail — interrupted banner", () => {
  it("shows the interrupted wording, not the cancelled copy, and no live action bar", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: unknown) => {
        const url = String(input);
        requested.push(url);
        if (url.includes("/v1/events/history")) return eventsReply();
        if (url.includes("/v1/artifacts")) return artifactsReply();
        if (url.includes("/timeline")) {
          return json({
            task_id: "task-1",
            started_at: "2026-08-31T14:22:41Z",
            now: "2026-08-31T14:32:41Z",
            completed_at: null,
            lanes: [],
          });
        }
        if (url.includes("/v1/tasks/")) {
          return json({ task: { ...task, status: "interrupted" } });
        }
        return json({ error: "not found" }, 404);
      }),
    );
    renderDetail();

    expect(
      await screen.findByText(/Interrupted by a daemon restart/),
    ).toBeInTheDocument();
    expect(screen.queryByText(/Cancelled by you/)).not.toBeInTheDocument();
    // Terminal, not live: no "Cancel run" control, only the banner's Re-run.
    expect(
      screen.queryByRole("button", { name: "Cancel run" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Re-run" })).toBeInTheDocument();
  });
});

describe("RunDetail — §5.6c Resume", () => {
  /** Every route the detail asks for, with `/v1/status` under our control. */
  function stubFetch(resumeEnabled: boolean) {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: unknown) => {
        const url = String(input);
        requested.push(url);
        if (url.includes("/v1/status")) {
          return json({
            home_root: "/home/.openalpaca",
            state_dir: "/home/.openalpaca/state",
            db_path: "/home/.openalpaca/state/openalpaca.db",
            project_root: null,
            started_at: "2026-09-05T09:00:00Z",
            uptime_secs: 10,
            schema_version: 39,
            log_path: null,
            upload_bytes: 0,
            produced_bytes: 0,
            retention: {
              log_max_session_bytes: 1,
              log_max_total_bytes: 1,
              log_retention_days: 0,
            },
            sessions: { last_sweep: null, dropped_records: 0 },
            routing: { resume_enabled: resumeEnabled },
          });
        }
        if (url.includes("/v1/events/history")) return eventsReply();
        if (url.includes("/v1/artifacts")) return artifactsReply();
        if (url.includes("/timeline")) {
          return json({
            task_id: "task-1",
            started_at: "2026-08-31T14:22:41Z",
            now: "2026-08-31T14:32:41Z",
            completed_at: null,
            lanes: [],
          });
        }
        if (url.includes("/v1/tasks/")) {
          return json({ task: { ...task, status: "interrupted" } });
        }
        return json({ error: "not found" }, 404);
      }),
    );
  }

  /**
   * The control is the daemon's to allow. S2 is experimental and off by
   * default, so a button whose only possible answer is `409 RESUME_DISABLED`
   * must not be on the banner — the daemon says, and this is where it is
   * asked.
   */
  it("hides Resume while the daemon reports the flag off", async () => {
    stubFetch(false);
    renderDetail();

    expect(
      await screen.findByRole("button", { name: "Re-run" }),
    ).toBeInTheDocument();
    await waitFor(() =>
      expect(requested.some((url) => url.includes("/v1/status"))).toBe(true),
    );
    expect(
      screen.queryByRole("button", { name: "Resume" }),
    ).not.toBeInTheDocument();
  });

  it("offers Resume beside Re-run once the daemon reports it enabled", async () => {
    stubFetch(true);
    renderDetail();

    expect(
      await screen.findByRole("button", { name: "Resume" }),
    ).toBeInTheDocument();
    // Never instead of `Re-run`: it is the fallback every resume refusal
    // points back at.
    expect(screen.getByRole("button", { name: "Re-run" })).toBeInTheDocument();
  });
});

/**
 * `fallbackRun` is the list's row, shown while the detail loads. When the
 * detail never arrives it is shown **instead**, which used to be silent: a
 * stale row rendered exactly as a fresh fetch would.
 */
describe("RunDetail — a detail read that failed", () => {
  it("names the list row as the source rather than passing it off as the detail", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: unknown) => {
        const url = String(input);
        requested.push(url);
        if (url.includes("/v1/events/history")) return eventsReply();
        if (url.includes("/v1/artifacts")) return artifactsReply();
        if (url.includes("/timeline")) return json({ error: "x" }, 500);
        if (url.includes("/v1/tasks/")) {
          return json(
            { error: { code: "DB_ERROR", message: "database is locked" } },
            500,
          );
        }
        return json({ error: "not found" }, 404);
      }),
    );
    renderDetail(listRow);

    // The column still draws off the row it has.
    expect(screen.getByText("connector audit")).toBeInTheDocument();
    await waitFor(() =>
      expect(
        screen.getByText(/Showing the list row — the run detail/),
      ).toBeInTheDocument(),
    );
    expect(screen.getByText(/database is locked/)).toBeInTheDocument();
  });

  it("says nothing of the sort once the detail answers", async () => {
    renderDetail(listRow);

    await waitFor(() =>
      expect(requested.some((url) => url.includes("/v1/tasks/"))).toBe(true),
    );
    await waitFor(() =>
      expect(screen.queryByText(/Showing the list row/)).toBeNull(),
    );
  });
});
