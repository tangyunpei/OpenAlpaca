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
  started_at: "2026-09-01T08:00:00Z",
  uptime_secs: 3_725,
  schema_version: 39,
  log_path: "/Users/dev/.openalpaca/state/logs/daemon.log",
  upload_bytes: 1_024,
  produced_bytes: 2_048,
  retention: {
    log_max_session_bytes: 256 * 1024 * 1024,
    log_max_total_bytes: 2 * 1024 * 1024 * 1024,
    log_retention_days: 0,
  },
  sessions: {
    last_sweep: {
      sessions_visited: 4,
      sessions_evicted: 1,
      files_removed: 2,
      bytes_freed: 512,
      bytes_before: 4_096,
      bytes_after: 3_584,
      over_cap_after: false,
      index_rows_cleared: 0,
    },
    dropped_records: 0,
  },
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

  // GAP-14's fields ride the same route: one request answers both "which
  // project is this window on" and "how is this daemon doing".
  it("carries the daemon's own numbers alongside the project answer", async () => {
    const status = await getDaemonStatus(null);

    expect(status.uptime_secs).toBe(3_725);
    expect(status.schema_version).toBe(39);
    expect(status.log_path).toBe(
      "/Users/dev/.openalpaca/state/logs/daemon.log",
    );
    expect(status.upload_bytes).toBe(1_024);
    expect(status.produced_bytes).toBe(2_048);
    expect(status.sessions.last_sweep?.over_cap_after).toBe(false);
    expect(status.sessions.dropped_records).toBe(0);
  });
});
