/**
 * `/v1/workspaces` — the client of a project's life cycle (storage design
 * §9). Each wrapper is pinned to the method, path, query and body the daemon
 * reads (interfaces §6.1), because a wrong one is a 405 or a 422 the owner
 * would meet as a dialog that does nothing.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";

import { resetConnection } from "@/lib/connection";
import { ApiError } from "@/lib/http";

import {
  chooseProject,
  deleteProjectData,
  getWorkspace,
  listProjects,
  repointProject,
  totalRows,
  type Project,
  type ProjectDataDeletion,
  type Workspace,
} from "./workspaces";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => ({
    baseUrl: "http://127.0.0.1:9999",
    token: "test-token",
    instanceId: "7f3a1122",
  })),
}));

interface Recorded {
  url: URL;
  method: string;
  body: unknown;
}

let requests: Recorded[] = [];
let reply: () => Response;

const WORKSPACE: Workspace = {
  path: "/Users/me/repo/src",
  root: "/Users/me/repo",
  id: "0b5c1d2e-3f40-4a5b-8c6d-7e8f9a0b1c2d",
  name: "-Users-me-repo",
  missing: false,
  rows: { artifacts: 1, sessions: 2, tasks: 3, messages: 4, followups: 5 },
  active_tasks: 0,
  queued_tasks: 1,
};

beforeEach(() => {
  requests = [];
  resetConnection();
  reply = () => new Response(JSON.stringify(WORKSPACE));
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: unknown, init?: RequestInit) => {
      requests.push({
        url: new URL(String(input)),
        method: init?.method ?? "GET",
        body: typeof init?.body === "string" ? JSON.parse(init.body) : null,
      });
      return reply();
    }),
  );
});

describe("getWorkspace", () => {
  it("asks about one path and keeps the root the daemon resolved it to", async () => {
    const answer = await getWorkspace("/Users/me/repo/src");

    expect(requests[0]?.method).toBe("GET");
    expect(requests[0]?.url.pathname).toBe("/v1/workspaces");
    expect(requests[0]?.url.searchParams.get("path")).toBe(
      "/Users/me/repo/src",
    );
    expect(answer.root).toBe("/Users/me/repo");
    expect(answer.rows.messages).toBe(4);
  });

  it("carries the daemon's code and sentence when it refuses a relative path", async () => {
    reply = () =>
      new Response(
        JSON.stringify({
          error: {
            code: "INVALID_PATH",
            message: "path must be absolute: repo",
          },
        }),
        { status: 400 },
      );

    const error = await getWorkspace("repo").catch((cause: unknown) => cause);

    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).code).toBe("INVALID_PATH");
    expect((error as ApiError).message).toBe("path must be absolute: repo");
  });
});

describe("totalRows", () => {
  it("sums all five path columns a re-point rewrites", () => {
    expect(totalRows(WORKSPACE.rows)).toBe(15);
  });
});

const PROJECT: Project = {
  id: "0b5c1d2e-3f40-4a5b-8c6d-7e8f9a0b1c2d",
  root: "/Users/me/repo",
  name: "-Users-me-repo",
  missing: false,
};

const PLAN: ProjectDataDeletion = {
  dry_run: true,
  applied: false,
  project: PROJECT,
  // One of the nine rows S5a's `plan_entries` writes, as it words it.
  entries: [
    {
      entry: "projects/-Users-me-repo/",
      holds:
        "project.json — the project's id and path; OpenAlpaca forgets the project",
      retention: "store metadata",
      action: "delete",
    },
  ],
  counts: {
    sessions: 2,
    messages: 40,
    tool_calls: 9,
    followups: 0,
    tasks: 3,
    spans: 4,
    run_events: 30,
    uploads: 1,
    artifacts: 5,
    memory_notes: 12,
  },
  kept: { uploads: 0, artifacts: 1 },
  removed: null,
};

describe("listProjects", () => {
  it("reads the bare list, with no path", async () => {
    reply = () => new Response(JSON.stringify([PROJECT]));

    expect(await listProjects()).toEqual([PROJECT]);
    expect(requests[0]?.method).toBe("GET");
    expect(requests[0]?.url.pathname).toBe("/v1/workspaces");
    expect(requests[0]?.url.search).toBe("");
  });
});

describe("chooseProject", () => {
  it("posts the folder and answers the project it is", async () => {
    reply = () => new Response(JSON.stringify(PROJECT));

    expect(await chooseProject("/Users/me/repo/src")).toEqual(PROJECT);
    expect(requests[0]?.method).toBe("POST");
    expect(requests[0]?.url.pathname).toBe("/v1/workspaces");
    expect(requests[0]?.body).toEqual({ path: "/Users/me/repo/src" });
  });

  it("carries the home-store refusal with the daemon's sentence", async () => {
    const sentence =
      "/Users/me is, contains or lies inside the OpenAlpaca home store, so it cannot be a project. Name the project directory itself";
    reply = () =>
      new Response(
        JSON.stringify({
          error: { code: "WORKSPACE_IS_HOME", message: sentence },
        }),
        { status: 409 },
      );

    const error = await chooseProject("/Users/me").catch(
      (cause: unknown) => cause,
    );

    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).status).toBe(409);
    expect((error as ApiError).code).toBe("WORKSPACE_IS_HOME");
    expect((error as ApiError).message).toBe(sentence);
  });
});

describe("repointProject", () => {
  it("patches the old and the new path under the names the daemon reads", async () => {
    reply = () =>
      new Response(
        JSON.stringify({
          old_path: "/Users/me/repo",
          new_path: "/Users/me/work/repo",
          moved: WORKSPACE.rows,
          project: {
            ...PROJECT,
            root: "/Users/me/work/repo",
            name: "-Users-me-work-repo",
          },
        }),
      );

    const answer = await repointProject(
      "/Users/me/repo",
      "/Users/me/work/repo",
    );

    expect(requests[0]?.method).toBe("PATCH");
    expect(requests[0]?.url.pathname).toBe("/v1/workspaces");
    expect(requests[0]?.body).toEqual({
      old_path: "/Users/me/repo",
      new_path: "/Users/me/work/repo",
    });
    expect(answer.project.root).toBe("/Users/me/work/repo");
  });
});

describe("deleteProjectData", () => {
  it("asks for the plan with dry_run sent as true", async () => {
    reply = () => new Response(JSON.stringify(PLAN));

    expect(await deleteProjectData("/Users/me/repo", true)).toEqual(PLAN);
    expect(requests[0]?.method).toBe("POST");
    expect(requests[0]?.url.pathname).toBe("/v1/workspaces/purge");
    expect(requests[0]?.body).toEqual({
      path: "/Users/me/repo",
      dry_run: true,
    });
  });

  it("deletes only with dry_run sent as false, never by leaving it out", async () => {
    reply = () =>
      new Response(
        JSON.stringify({
          ...PLAN,
          dry_run: false,
          applied: true,
          removed: {
            session_dirs: 2,
            memory_partitions: 1,
            project_folder: true,
          },
        }),
      );

    const done = await deleteProjectData("/Users/me/repo", false);

    expect(requests[0]?.body).toEqual({
      path: "/Users/me/repo",
      dry_run: false,
    });
    expect(done.removed?.project_folder).toBe(true);
  });
});
