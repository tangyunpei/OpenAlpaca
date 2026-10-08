/**
 * `/v1/workspaces` — the client of a project's life cycle (storage design
 * §9). Each wrapper is pinned to the method, path, query and body the daemon
 * reads (interfaces §6.1), because a wrong one is a 405 or a 422 the owner
 * would meet as a dialog that does nothing.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";

import { resetConnection } from "@/lib/connection";
import { ApiError } from "@/lib/http";

import { getWorkspace, totalRows, type Workspace } from "./workspaces";

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
