/**
 * The `/v1/memories*` client, over the real `apiFetch` stack (design §7a).
 *
 * Only the two edges are doubled — the Tauri discovery command and `fetch` —
 * so the method, the query string, the headers and the body under test are
 * the ones the daemon would actually receive. The contract that matters most
 * here is the revision's place on the wire: **`If-Match` and nowhere else**. A
 * body `expected_revision` on a single-note mutation is a `400` from the
 * daemon, so a client that sent one would never manage to edit anything.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";

import { resetConnection } from "@/lib/connection";
import { ApiError } from "@/lib/http";

import {
  createMemory,
  forgetMemory,
  getMemory,
  getMemoryHistory,
  isMemoryConflict,
  listMemories,
  listRecentlyLearned,
  searchMemories,
  undoMemory,
  updateMemory,
} from "./memories";

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
  headers: Headers;
}

let requests: Recorded[] = [];
let reply: (url: URL) => Response;

function json(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), { status });
}

const REV = "a".repeat(64);
const ID = "55e811b6-3220-4758-a47b-034bd1023f38";

beforeEach(() => {
  requests = [];
  resetConnection();
  reply = () => json({ items: [], next_cursor: null });
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: unknown, init?: RequestInit) => {
      const url = new URL(String(input));
      requests.push({
        url,
        method: init?.method ?? "GET",
        body: typeof init?.body === "string" ? JSON.parse(init.body) : null,
        headers: new Headers(init?.headers),
      });
      return reply(url);
    }),
  );
});

function only(): Recorded {
  expect(requests).toHaveLength(1);
  const request = requests[0];
  if (request === undefined) throw new Error("no request");
  return request;
}

describe("reads", () => {
  it("lists with the filters the view chose and nothing it did not", async () => {
    await listMemories(
      { scope: "project", status: "candidate", limit: 100 },
      "/Users/me/repo",
    );
    const request = only();
    expect(request.method).toBe("GET");
    expect(request.url.pathname).toBe("/v1/memories");
    expect(Object.fromEntries(request.url.searchParams)).toEqual({
      scope: "project",
      status: "candidate",
      limit: "100",
    });
    // The project partition is selected by the header, exactly as on a turn.
    expect(request.headers.get("x-workspace-path")).toBe("/Users/me/repo");
  });

  it("sends no workspace header for a window with no project", async () => {
    await listMemories({ scope: "global" }, null);
    expect(only().headers.has("x-workspace-path")).toBe(false);
  });

  it("passes a cursor through untouched", async () => {
    await listMemories({ scope: "global", cursor: "eyJ0cyI6IjIwMjYifQ" }, null);
    expect(only().url.searchParams.get("cursor")).toBe("eyJ0cyI6IjIwMjYifQ");
  });

  it("searches with `q`, never a caller-built MATCH expression", async () => {
    reply = () =>
      json({ results: [], count: 0, partial: false, index_state: "ready" });
    const page = await searchMemories(
      { q: "editor preference", scope: "global" },
      null,
    );
    const request = only();
    expect(request.url.pathname).toBe("/v1/memories/search");
    expect(request.url.searchParams.get("q")).toBe("editor preference");
    expect(request.url.searchParams.get("scope")).toBe("global");
    expect(page.index_state).toBe("ready");
  });

  it("reads the recently-learned envelope's items", async () => {
    reply = () =>
      json({
        items: [
          {
            id: ID,
            revision: REV,
            title: "Prefers tabs",
            kind: "preference",
            scope: "global",
            status: "active",
            effective_status: "active",
            origin: "extracted_user",
            updated_at: "2026-09-24T10:00:00Z",
            created_at: "2026-09-24T10:00:00Z",
            pinned: false,
          },
        ],
      });
    const items = await listRecentlyLearned(null);
    expect(only().url.pathname).toBe("/v1/memories/recently-learned");
    expect(items.map((item) => item.title)).toEqual(["Prefers tabs"]);
  });

  it("reads one note, or one kept revision of it", async () => {
    reply = () => json({ id: ID, revision: REV });
    await getMemory(ID, null);
    await getMemory(ID, null, { revision: "b".repeat(64) });
    expect(requests.map((r) => r.url.pathname)).toEqual([
      `/v1/memories/${ID}`,
      `/v1/memories/${ID}`,
    ]);
    expect(requests[0]?.url.search).toBe("");
    expect(requests[1]?.url.searchParams.get("revision")).toBe("b".repeat(64));
  });

  it("reads the history envelope's revisions", async () => {
    reply = () =>
      json({
        revisions: [{ revision: REV, written_at: "2026-09-24T10:00:00Z" }],
      });
    const revisions = await getMemoryHistory(ID, null);
    expect(only().url.pathname).toBe(`/v1/memories/${ID}/history`);
    expect(revisions).toHaveLength(1);
  });
});

describe("writes carry the revision in If-Match and nowhere else", () => {
  it("edits with the quoted revision in If-Match and a body with no revision", async () => {
    reply = () => json({ id: ID, revision: "c".repeat(64), status: "active" });
    await updateMemory(
      ID,
      REV,
      { action: "edit", patch: { title: "Uses tabs", body: "Tabs." } },
      "/Users/me/repo",
    );
    const request = only();
    expect(request.method).toBe("POST");
    expect(request.url.pathname).toBe(`/v1/memories/${ID}`);
    expect(request.headers.get("If-Match")).toBe(`"${REV}"`);
    expect(request.headers.get("x-workspace-path")).toBe("/Users/me/repo");
    expect(request.body).toEqual({
      action: "edit",
      patch: { title: "Uses tabs", body: "Tabs." },
    });
    expect(request.body).not.toHaveProperty("expected_revision");
  });

  it("confirms with no patch at all — `patch` goes with `edit` only", async () => {
    reply = () => json({ id: ID, revision: "c".repeat(64), status: "active" });
    await updateMemory(ID, REV, { action: "confirm" }, null);
    expect(only().body).toEqual({ action: "confirm" });
  });

  it("undoes by POST …/undo with If-Match", async () => {
    reply = () => json({ id: ID, status: "forgotten" });
    await undoMemory(ID, REV, null);
    const request = only();
    expect(request.method).toBe("POST");
    expect(request.url.pathname).toBe(`/v1/memories/${ID}/undo`);
    expect(request.headers.get("If-Match")).toBe(`"${REV}"`);
  });

  it("forgets by DELETE with If-Match", async () => {
    reply = () =>
      json({
        id: ID,
        status: "forgotten",
        retained: { transcript: true, session_logs: true, note: "kept" },
      });
    const result = await forgetMemory(ID, REV, null);
    const request = only();
    expect(request.method).toBe("DELETE");
    expect(request.url.pathname).toBe(`/v1/memories/${ID}`);
    expect(request.headers.get("If-Match")).toBe(`"${REV}"`);
    expect(result.retained.transcript).toBe(true);
  });

  it("creates with the store body and no If-Match — a create has nothing to match", async () => {
    reply = () =>
      json(
        {
          id: ID,
          revision: REV,
          scope: "global",
          origin: "user",
          superseded: [],
          dedup: "created",
        },
        201,
      );
    const created = await createMemory(
      { title: "Uses tabs", body: "Tabs, width 4.", kind: "preference" },
      null,
    );
    const request = only();
    expect(request.method).toBe("POST");
    expect(request.url.pathname).toBe("/v1/memories");
    expect(request.headers.has("If-Match")).toBe(false);
    expect(request.body).toEqual({
      title: "Uses tabs",
      body: "Tabs, width 4.",
      kind: "preference",
    });
    expect(created.origin).toBe("user");
  });
});

describe("isMemoryConflict", () => {
  it("is the stale-revision refusal and nothing else", async () => {
    reply = () =>
      json(
        {
          error: {
            code: "MEMORY_REVISION_CONFLICT",
            message: `note ${ID} is at revision ${"d".repeat(64)}`,
          },
        },
        409,
      );
    const error = await updateMemory(ID, REV, { action: "archive" }, null).then(
      () => null,
      (e: unknown) => e,
    );
    expect(error).toBeInstanceOf(ApiError);
    expect(isMemoryConflict(error)).toBe(true);

    // Another 409 is a different refusal with its own sentence.
    expect(
      isMemoryConflict(
        new ApiError("not undoable", 409, "MEMORY_NOT_UNDOABLE"),
      ),
    ).toBe(false);
    expect(isMemoryConflict(new Error("boom"))).toBe(false);
  });
});
