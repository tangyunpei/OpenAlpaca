/**
 * `/v1/sessions*` (plan §5.7), over the real `apiFetch` stack.
 *
 * Only the two edges are doubled — the Tauri discovery command and `fetch` —
 * so the method, path and body under test are the ones the daemon would
 * receive. The write verbs are the sidebar's whole vocabulary, and each of
 * their refusals is a different fact: a delete blocked by a live run is not a
 * delete that found nothing, and a project that cannot be re-pointed is not a
 * conversation that vanished.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";

import { resetConnection } from "@/lib/connection";
import { ApiError } from "@/lib/http";
import type { Session } from "@/lib/api/types";

import {
  activateSession,
  archiveSession,
  createSession,
  deleteSession,
  listSessions,
  sessionErrorMessage,
  updateSession,
} from "./sessions";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => ({
    baseUrl: "http://127.0.0.1:9999",
    token: "tok en/+",
    instanceId: "7f3a1122",
  })),
}));

interface Recorded {
  url: string;
  method: string;
  body: unknown;
}

let requests: Recorded[] = [];
let reply: () => Response;

function json(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), { status });
}

/** The daemon's shared envelope: `{"error":{"code","message"}}`. */
function apiErrorResponse(
  status: number,
  code: string,
  message: string,
): Response {
  return json({ error: { code, message } }, status);
}

const SESSION: Session = {
  id: "sess-1",
  lane_key: "user:gui",
  source: "gui",
  title: "",
  workspace_id: "/Users/dev/openalpaca",
  status: "active",
  message_count: 4,
  last_message_at: "2026-09-06 10:00:00",
  created_at: "2026-09-06 09:00:00",
  updated_at: "2026-09-06 10:00:00",
  ended_at: null,
  active_task_count: 0,
  interrupted_task_count: 0,
};

beforeEach(() => {
  requests = [];
  resetConnection();
  reply = () => json(SESSION);
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: unknown, init?: RequestInit) => {
      requests.push({
        url: String(input),
        method: init?.method ?? "GET",
        body: typeof init?.body === "string" ? JSON.parse(init.body) : null,
      });
      return reply();
    }),
  );
});

describe("listSessions", () => {
  it("reads the envelope, filters included", async () => {
    reply = () => json({ sessions: [SESSION], total: 1 });

    const page = await listSessions({ source: "gui", limit: 100 });

    expect(requests[0]?.method).toBe("GET");
    expect(requests[0]?.url).toBe(
      "http://127.0.0.1:9999/v1/sessions?source=gui&limit=100",
    );
    expect(page).toEqual({ sessions: [SESSION], total: 1 });
  });
});

describe("createSession", () => {
  it('posts the "New chat" body and answers the created row', async () => {
    reply = () => json(SESSION, 201);

    const created = await createSession({
      workspacePath: "/Users/dev/openalpaca",
    });

    expect(requests[0]?.method).toBe("POST");
    expect(requests[0]?.url).toBe("http://127.0.0.1:9999/v1/sessions");
    expect(requests[0]?.body).toEqual({
      source: "gui",
      workspace_path: "/Users/dev/openalpaca",
    });
    expect(created).toEqual(SESSION);
  });

  /**
   * No project chosen must send no `workspace_path` at all — an empty string
   * is the daemon's *unbind* spelling, not "none".
   */
  it("omits the project entirely when the window has none", async () => {
    reply = () => json(SESSION, 201);
    await createSession();
    expect(requests[0]?.body).toEqual({ source: "gui" });
  });
});

describe("the row verbs", () => {
  it("activates, archives, renames and deletes by id", async () => {
    await activateSession("sess-1");
    await archiveSession("sess-1");
    await updateSession("sess-1", { title: "Rust workspace" });
    reply = () => new Response(null, { status: 204 });
    await deleteSession("sess-1");

    expect(requests.map((r) => `${r.method} ${r.url}`)).toEqual([
      "POST http://127.0.0.1:9999/v1/sessions/sess-1/activate",
      "POST http://127.0.0.1:9999/v1/sessions/sess-1/archive",
      "PATCH http://127.0.0.1:9999/v1/sessions/sess-1",
      "DELETE http://127.0.0.1:9999/v1/sessions/sess-1",
    ]);
    expect(requests[2]?.body).toEqual({ title: "Rust workspace" });
  });

  it("escapes the id rather than splicing it into the path raw", async () => {
    await activateSession("a/b");
    expect(requests[0]?.url).toBe(
      "http://127.0.0.1:9999/v1/sessions/a%2Fb/activate",
    );
  });

  /**
   * R48: a session's project is bound once. The rename half of `PATCH` must
   * not carry a `workspace_path` it was not asked for, or every rename of a
   * bound conversation would answer `409 SESSION_WORKSPACE_BOUND`.
   */
  it("sends only the fields the caller named", async () => {
    await updateSession("sess-1", { workspacePath: "/Users/dev/other" });
    expect(requests[0]?.body).toEqual({
      workspace_path: "/Users/dev/other",
    });
  });
});

describe("sessionErrorMessage", () => {
  it("gives each refusal its own sentence", () => {
    const cases: [string, number, RegExp][] = [
      ["SESSION_HAS_ACTIVE_WORKFLOWS", 409, /run in flight/i],
      ["SESSION_WORKSPACE_BOUND", 409, /bound to a project/i],
      ["SESSION_ARCHIVED", 409, /archived/i],
      ["SESSION_LANE_MISMATCH", 409, /another lane/i],
      ["SESSION_NOT_FOUND", 404, /no longer/i],
      ["INVALID_STATUS", 400, /active.*archived/i],
    ];
    for (const [code, status, expected] of cases) {
      expect(
        sessionErrorMessage(new ApiError("daemon words", status, code)),
      ).toMatch(expected);
    }
  });

  it("says nothing changed when the daemon was never reached", () => {
    expect(sessionErrorMessage(new ApiError("boom", 0))).toMatch(
      /Could not reach the daemon/,
    );
  });

  /** An unrecognised code falls back to the daemon's own words, not a shrug. */
  it("passes an unknown code's message through", () => {
    expect(
      sessionErrorMessage(new ApiError("something specific", 500, "DB_ERROR")),
    ).toBe("something specific");
  });

  it("renders the envelope the daemon actually sends", async () => {
    reply = () =>
      apiErrorResponse(
        409,
        "SESSION_HAS_ACTIVE_WORKFLOWS",
        "This conversation has a run in flight. Cancel it before deleting.",
      );

    await expect(deleteSession("sess-1")).rejects.toThrow(ApiError);
    await deleteSession("sess-1").catch((error: unknown) => {
      expect(sessionErrorMessage(error)).toMatch(/run in flight/i);
    });
  });
});
