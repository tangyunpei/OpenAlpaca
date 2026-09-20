/**
 * `/v1/lanes/{lane_key}/followups*` (GAP-03, closed), over the real `apiFetch`
 * stack.
 *
 * Only the two edges are doubled — the Tauri discovery command and `fetch` — so
 * the method, path, headers and body under test are the ones the daemon would
 * receive.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiError } from "@/lib/http";
import { resetConnection } from "@/lib/connection";

import {
  cancelFollowup,
  followupErrorMessage,
  listFollowups,
  queueFollowup,
} from "./followups";

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
  headers: Headers;
}

let requests: Recorded[] = [];
let reply: () => Response;

function json(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), { status });
}

/** The daemon's shared envelope: `{"error":{"code","message"}}`. */
function apiError(status: number, code: string, message: string): Response {
  return json({ error: { code, message } }, status);
}

const ROW = {
  id: 42,
  lane_key: "user:gui",
  kind: "followup" as const,
  content: "audit the connectors",
  source_task_id: "task-7",
  status: "queued" as const,
  created_at: "2026-09-05 10:00:00",
  updated_at: "2026-09-05 10:00:00",
};

beforeEach(() => {
  requests = [];
  resetConnection();
  reply = () => json(ROW);
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: unknown, init?: RequestInit) => {
      requests.push({
        url: String(input),
        method: init?.method ?? "GET",
        body: typeof init?.body === "string" ? JSON.parse(init.body) : null,
        headers: new Headers(init?.headers),
      });
      return reply();
    }),
  );
});

describe("listFollowups", () => {
  it("reads the lane's queue as a bare array", async () => {
    reply = () => json([ROW]);

    const rows = await listFollowups("user:gui");

    expect(requests).toHaveLength(1);
    expect(requests[0]?.method).toBe("GET");
    expect(requests[0]?.url).toBe(
      "http://127.0.0.1:9999/v1/lanes/user%3Agui/followups",
    );
    expect(rows).toEqual([ROW]);
  });

  it("escapes the lane key rather than splicing it into the path raw", async () => {
    reply = () => json([]);
    await listFollowups("someone/else:cli");
    expect(requests[0]?.url).toBe(
      "http://127.0.0.1:9999/v1/lanes/someone%2Felse%3Acli/followups",
    );
  });
});

describe("queueFollowup", () => {
  it("posts the content and returns the stored row", async () => {
    const row = await queueFollowup("user:gui", "audit the connectors");

    expect(requests[0]?.method).toBe("POST");
    expect(requests[0]?.url).toBe(
      "http://127.0.0.1:9999/v1/lanes/user%3Agui/followups",
    );
    expect(requests[0]?.body).toEqual({ content: "audit the connectors" });
    expect(row).toEqual(ROW);
  });

  it("names the run it was queued behind, when there is one", async () => {
    await queueFollowup("user:gui", "then write it up", "task-7");
    expect(requests[0]?.body).toEqual({
      content: "then write it up",
      source_task_id: "task-7",
    });
  });

  /**
   * The project rides the header, not the body — the daemon resolves
   * `x-workspace-path` through the one resolver every route shares, so a
   * follow-up cannot name a project the rest of the app would not accept.
   */
  it("sends the project as the workspace header, and only when named", async () => {
    await queueFollowup("user:gui", "go");
    expect(requests[0]?.headers.has("x-workspace-path")).toBe(false);

    await queueFollowup("user:gui", "go", undefined, "/Users/dev/openalpaca");
    expect(requests[1]?.headers.get("x-workspace-path")).toBe(
      "/Users/dev/openalpaca",
    );
  });

  // The daemon owns the kind; nothing in this client can ask for the other one.
  it("never sends a kind", async () => {
    await queueFollowup("user:gui", "go", "task-7", "/tmp/p");
    expect(requests[0]?.body).not.toHaveProperty("kind");
  });

  it("surfaces the daemon's code on the thrown ApiError", async () => {
    reply = () => apiError(400, "EMPTY_CONTENT", "content must not be empty");

    const error = await queueFollowup("user:gui", " ").catch(
      (caught: unknown) => caught,
    );
    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).status).toBe(400);
    expect((error as ApiError).code).toBe("EMPTY_CONTENT");
  });
});

describe("cancelFollowup", () => {
  it("deletes the row by id and returns the receipt", async () => {
    reply = () => json({ id: 42, status: "cancelled" });

    const result = await cancelFollowup("user:gui", 42);

    expect(requests[0]?.method).toBe("DELETE");
    expect(requests[0]?.url).toBe(
      "http://127.0.0.1:9999/v1/lanes/user%3Agui/followups/42",
    );
    expect(result).toEqual({ id: 42, status: "cancelled" });
  });

  /**
   * The race the route exists to report: the daemon claimed the item first, so
   * the cancel lost its compare-and-swap. That is a `409` and it must reach the
   * caller as one — a swallowed failure would leave the UI claiming a turn was
   * stopped while it runs.
   */
  it("throws FOLLOWUP_NOT_QUEUED when the autostart won the race", async () => {
    reply = () =>
      apiError(
        409,
        "FOLLOWUP_NOT_QUEUED",
        "That follow-up is no longer queued",
      );

    const error = await cancelFollowup("user:gui", 42).catch(
      (caught: unknown) => caught,
    );
    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).status).toBe(409);
    expect((error as ApiError).code).toBe("FOLLOWUP_NOT_QUEUED");
  });
});

describe("followupErrorMessage", () => {
  it("gives every documented code its own sentence", () => {
    const codes = [
      "FOLLOWUP_NOT_QUEUED",
      "NOT_FOUND",
      "EMPTY_CONTENT",
      "INVALID_KIND",
      "INVALID_LANE_KEY",
    ] as const;
    const messages = codes.map((code) =>
      followupErrorMessage(new ApiError("raw daemon text", 409, code)),
    );

    for (const message of messages) {
      expect(message.length).toBeGreaterThan(0);
      // Never the raw daemon string, and never an empty shrug.
      expect(message).not.toBe("raw daemon text");
    }
    expect(new Set(messages).size).toBe(codes.length);
    // The one that matters most: "already running" is a different fact from
    // "gone", and the user acts on the difference.
    expect(messages[0]).toMatch(/already started|running/i);
    expect(messages[0]).not.toMatch(/no longer on this lane/i);
    expect(messages[1]).toMatch(/no longer on this lane/i);
    expect(messages[2]).toMatch(/empty/i);
  });

  it("falls back to the daemon's own message for an unknown code", () => {
    expect(
      followupErrorMessage(new ApiError("something specific", 500, "DB_ERROR")),
    ).toBe("something specific");
  });

  it("says the daemon is unreachable rather than nothing at all", () => {
    expect(followupErrorMessage(new ApiError("fetch failed", 0, null))).toMatch(
      /reach the daemon/i,
    );
    expect(followupErrorMessage(new Error("boom"))).toBe("boom");
    expect(followupErrorMessage("not an error")).toMatch(/could not/i);
  });
});
