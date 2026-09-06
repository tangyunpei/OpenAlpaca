/**
 * `GET /v1/status`, over the real `apiFetch` stack.
 *
 * The one thing worth pinning is the header: the route answers a question
 * *about the request*, so the project it reports is the project the caller
 * sent. A window with no project must send none rather than an empty string —
 * `""` is a value the daemon would try to resolve.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";

import { resetConnection } from "@/lib/connection";
import type { DaemonStatus } from "@/lib/api/types";

import { getDaemonStatus } from "./status";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => ({
    baseUrl: "http://127.0.0.1:9999",
    token: "test-token",
    instanceId: "7f3a1122",
  })),
}));

const STATUS: DaemonStatus = {
  home_root: "/Users/dev/.openalpaca",
  state_dir: "/Users/dev/.openalpaca/state",
  db_path: "/Users/dev/.openalpaca/state/openalpaca.db",
  project_root: "/repo",
};

let requests: { url: string; headers: Headers }[] = [];

beforeEach(() => {
  requests = [];
  resetConnection();
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: unknown, init?: RequestInit) => {
      requests.push({
        url: String(input),
        headers: new Headers(init?.headers),
      });
      return new Response(JSON.stringify(STATUS), { status: 200 });
    }),
  );
});

describe("getDaemonStatus", () => {
  it("asks about the window's own project, as a turn would", async () => {
    const status = await getDaemonStatus("/repo/apps/gui");

    expect(requests[0]?.url).toContain("/v1/status");
    expect(requests[0]?.headers.get("x-workspace-path")).toBe("/repo/apps/gui");
    // The picker's path resolves to a root; that root is the comparable value.
    expect(status.project_root).toBe("/repo");
  });

  it("sends no project header at all when the window has none", async () => {
    await getDaemonStatus(null);

    expect(requests[0]?.headers.has("x-workspace-path")).toBe(false);
  });
});
