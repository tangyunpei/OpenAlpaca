/**
 * The run detail's `Output` card, over the real data layer.
 *
 * Phase 3 gave the card a typed source — `GET /v1/artifacts?task_id=` — where
 * it previously had only the run's free-form outcome blob. Both edges are
 * doubled and nothing else.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

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
let requested: string[] = [];

function json(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), { status });
}

beforeEach(() => {
  requested = [];
  resetConnection();
  useUiStore.setState({ view: "work", panelArtifactId: null });
  artifactsReply = () => json({ artifacts: [artifact], total: 1 });
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: unknown) => {
      const url = String(input);
      requested.push(url);
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
      if (url.includes("/v1/tasks/")) return json({ task, assignments: [] });
      return json({ error: "not found" }, 404);
    }),
  );
});

function renderDetail() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return render(
    <QueryClientProvider client={client}>
      <RunDetail runId="task-1" onAction={vi.fn()} />
    </QueryClientProvider>,
  );
}

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
        if (url.includes("/v1/artifacts")) return artifactsReply();
        if (url.includes("/timeline")) {
          return json(
            { error: { code: "DB_ERROR", message: "database is locked" } },
            500,
          );
        }
        if (url.includes("/v1/tasks/")) return json({ task, assignments: [] });
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

/** `userEvent.setup()` must run before render for its clipboard/pointer stubs. */
function useAndRender() {
  const user = userEvent.setup();
  renderDetail();
  return user;
}
