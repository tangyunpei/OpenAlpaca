/**
 * The run-addressed writes — `POST /v1/tasks/{id}/steer` (GAP-02) and the two
 * launch verbs (GAP-06) — over the real `apiFetch` stack.
 *
 * Only the two edges are doubled — the Tauri discovery command and `fetch` — so
 * the method, path and body under test are the ones the daemon would receive.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiError } from "@/lib/http";

import { resetConnection } from "@/lib/connection";

import {
  launchErrorMessage,
  rerunTask,
  startTaskNow,
  steerErrorMessage,
  steerTask,
} from "./tasks";

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
function apiError(status: number, code: string, message: string): Response {
  return json({ error: { code, message } }, status);
}

beforeEach(() => {
  requests = [];
  resetConnection();
  reply = () =>
    json({
      task_id: "task-1",
      accepted: true,
      inbox_depth: 2,
      lane_key: "user:gui",
    });
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

describe("steerTask", () => {
  it("posts the message to the run's own steer route and returns the receipt", async () => {
    const result = await steerTask("task-1", "focus on the tests");

    expect(requests).toHaveLength(1);
    expect(requests[0]?.method).toBe("POST");
    expect(requests[0]?.url).toBe(
      "http://127.0.0.1:9999/v1/tasks/task-1/steer",
    );
    expect(requests[0]?.body).toEqual({ message: "focus on the tests" });

    // `inbox_depth` is the receipt — not a promise the workflow read it.
    expect(result).toEqual({
      task_id: "task-1",
      accepted: true,
      inbox_depth: 2,
      lane_key: "user:gui",
    });
  });

  it("escapes the task id rather than splicing it into the path raw", async () => {
    await steerTask("task/1 2", "go");
    expect(requests[0]?.url).toBe(
      "http://127.0.0.1:9999/v1/tasks/task%2F1%202/steer",
    );
  });

  it("sends the project only when the caller names one", async () => {
    await steerTask("task-1", "go", "/Users/dev/openalpaca");
    expect(requests[0]?.body).toEqual({
      message: "go",
      workspace_path: "/Users/dev/openalpaca",
    });
  });

  it("surfaces the daemon's code on the thrown ApiError", async () => {
    reply = () =>
      apiError(409, "STEERING_INBOX_FULL", "The steering queue is full (16).");

    const error = await steerTask("task-1", "go").catch(
      (caught: unknown) => caught,
    );
    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).status).toBe(409);
    expect((error as ApiError).code).toBe("STEERING_INBOX_FULL");
  });
});

describe("steerErrorMessage", () => {
  it("gives every documented code its own sentence", () => {
    const codes = [
      "STEERING_INBOX_FULL",
      "TASK_NOT_STEERABLE",
      "STEERING_DISABLED",
      "EMPTY_MESSAGE",
      "NOT_FOUND",
    ] as const;
    const messages = codes.map((code) =>
      steerErrorMessage(new ApiError("raw daemon text", 409, code)),
    );

    for (const message of messages) {
      expect(message.length).toBeGreaterThan(0);
      // Never the raw daemon string, and never an empty shrug.
      expect(message).not.toBe("raw daemon text");
    }
    expect(new Set(messages).size).toBe(codes.length);
    expect(messages[0]).toMatch(/queue/i);
    expect(messages[1]).toMatch(/no longer running|not running|finished/i);
    expect(messages[2]).toMatch(/disabled/i);
    expect(messages[4]).toMatch(/gone|not found|no longer/i);
  });

  // A `404` has two very different causes, and the client is the only side
  // that knows which one it is looking at: the daemon answers the same body
  // for a run that never existed and one that belongs to another channel.
  it("distinguishes a run it can still see from one that is gone", () => {
    const notFound = new ApiError("Task not found", 404, "NOT_FOUND");

    // The row is on screen and updating — "gone" would be a lie.
    const onScreen = steerErrorMessage(notFound, true);
    expect(onScreen).toMatch(/can't be steered/i);
    expect(onScreen).not.toMatch(/gone|no longer exists/i);

    // Nothing on screen for it: "no longer exists" is the honest reading.
    expect(steerErrorMessage(notFound, false)).toMatch(/no longer exists/i);
    // Unknown by default — the same sentence as an explicit `false`.
    expect(steerErrorMessage(notFound)).toBe(
      steerErrorMessage(notFound, false),
    );
  });

  it("falls back to the daemon's own message for an unknown code", () => {
    expect(
      steerErrorMessage(new ApiError("something specific", 500, "DB_ERROR")),
    ).toBe("something specific");
  });

  it("says the daemon is unreachable rather than nothing at all", () => {
    expect(steerErrorMessage(new ApiError("fetch failed", 0, null))).toMatch(
      /reach the daemon/i,
    );
    expect(steerErrorMessage(new Error("boom"))).toBe("boom");
    expect(steerErrorMessage("not an error")).toMatch(/could not/i);
  });
});

// ── Re-run and start (GAP-06) ───────────────────────────────────────────────

describe("rerunTask", () => {
  it("posts to the run's own rerun route and returns a run that is not it", async () => {
    reply = () =>
      json(
        {
          task_id: "task-2",
          source_task_id: "task-1",
          title: "Ship the release",
          status: "queued",
        },
        201,
      );

    const result = await rerunTask("task-1");

    expect(requests).toHaveLength(1);
    expect(requests[0]?.method).toBe("POST");
    expect(requests[0]?.url).toBe(
      "http://127.0.0.1:9999/v1/tasks/task-1/rerun",
    );
    // Nothing to send: the goal comes from the stored row, not the client.
    expect(requests[0]?.body).toBeNull();

    expect(result.task_id).toBe("task-2");
    expect(result.source_task_id).toBe("task-1");
  });

  it("escapes the task id rather than splicing it into the path raw", async () => {
    reply = () =>
      json(
        {
          task_id: "task-2",
          source_task_id: "task/1 2",
          title: "t",
          status: "queued",
        },
        201,
      );
    await rerunTask("task/1 2");
    expect(requests[0]?.url).toBe(
      "http://127.0.0.1:9999/v1/tasks/task%2F1%202/rerun",
    );
  });

  it("surfaces the daemon's code on the thrown ApiError", async () => {
    reply = () =>
      apiError(409, "TASK_NOT_TERMINAL", "This run has not finished (running)");

    const error = await rerunTask("task-1").catch((caught: unknown) => caught);
    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).status).toBe(409);
    expect((error as ApiError).code).toBe("TASK_NOT_TERMINAL");
  });
});

describe("startTaskNow", () => {
  // D5 — the response's id is the id that was sent, which is why `start` can
  // ride the same route and the same shape as the three transitions.
  it("posts `start` to the action route and answers with the same id", async () => {
    reply = () => json({ task_id: "task-1", status: "running" });

    const result = await startTaskNow("task-1");

    expect(requests[0]?.url).toBe(
      "http://127.0.0.1:9999/v1/tasks/task-1/action",
    );
    expect(requests[0]?.body).toEqual({ action: "start" });
    expect(result.task_id).toBe("task-1");
  });

  it("surfaces the daemon's code on the thrown ApiError", async () => {
    reply = () =>
      apiError(409, "TASK_ALREADY_RUNNING", "This run is already running.");

    const error = await startTaskNow("task-1").catch(
      (caught: unknown) => caught,
    );
    expect((error as ApiError).code).toBe("TASK_ALREADY_RUNNING");
  });
});

describe("launchErrorMessage", () => {
  it("gives every documented code its own sentence", () => {
    const codes = [
      "TASK_NOT_TERMINAL",
      "TASK_ALREADY_RUNNING",
      "TASK_NOT_RERUNNABLE",
      "TASK_NOT_STARTABLE",
      "DISPATCH_FAILED",
      "NOT_FOUND",
    ] as const;
    const messages = codes.map((code) =>
      launchErrorMessage(new ApiError("raw daemon text", 409, code)),
    );

    for (const message of messages) {
      expect(message.length).toBeGreaterThan(0);
      expect(message).not.toBe("raw daemon text");
    }
    // The two 422s differ only by verb, so they must not read the same.
    expect(new Set(messages).size).toBe(codes.length);
    expect(messages[0]).toMatch(/hasn't finished|steer or cancel/i);
    expect(messages[1]).toMatch(/already running/i);
    expect(messages[2]).toMatch(/re-run/i);
    expect(messages[3]).toMatch(/start/i);
    expect(messages[4]).toMatch(/no agent is free/i);
  });

  it("falls back to the daemon's own message for an unknown code", () => {
    expect(
      launchErrorMessage(new ApiError("something specific", 500, "DB_ERROR")),
    ).toBe("something specific");
  });

  it("says the daemon is unreachable rather than nothing at all", () => {
    expect(launchErrorMessage(new ApiError("fetch failed", 0, null))).toMatch(
      /reach the daemon/i,
    );
    expect(launchErrorMessage(new Error("boom"))).toBe("boom");
    expect(launchErrorMessage("not an error")).toMatch(/could not/i);
  });
});
