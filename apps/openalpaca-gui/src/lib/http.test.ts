import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { getCachedConnection, resetConnection } from "./connection";
import { fixtureHost } from "./host/fixture";
import {
  ApiError,
  apiFetch,
  buildQuery,
  errorFromResponse,
  parseErrorPayload,
} from "./http";

describe("parseErrorPayload", () => {
  it("reads the structured envelope used by chat and settings", () => {
    expect(
      parseErrorPayload(
        { error: { code: "NOT_FOUND", message: "Stream not found" } },
        "fallback",
      ),
    ).toEqual({ code: "NOT_FOUND", message: "Stream not found" });
  });

  it("reads the string envelope used by tasks, agents, plugins and connectors", () => {
    expect(
      parseErrorPayload({ error: "Unknown action: 'rerun'" }, "fallback"),
    ).toEqual({
      code: null,
      message: "Unknown action: 'rerun'",
    });
  });

  it("appends `details` when the string envelope carries one", () => {
    expect(
      parseErrorPayload(
        { error: "Failed to query event history", details: "db locked" },
        "f",
      ),
    ).toEqual({
      code: null,
      message: "Failed to query event history: db locked",
    });
  });

  it("handles the plain-text 401 from the SSE and WS routes", () => {
    expect(parseErrorPayload("Invalid token", "fallback")).toEqual({
      code: null,
      message: "Invalid token",
    });
  });

  it("falls back when the body is empty, blank or unrecognised", () => {
    expect(parseErrorPayload(null, "Not Found").message).toBe("Not Found");
    expect(parseErrorPayload("   ", "Not Found").message).toBe("Not Found");
    expect(parseErrorPayload({ unexpected: true }, "Not Found").message).toBe(
      "Not Found",
    );
  });

  it("keeps a partial structured envelope usable", () => {
    expect(
      parseErrorPayload({ error: { code: "STEERING_INBOX_FULL" } }, "fallback"),
    ).toEqual({
      code: "STEERING_INBOX_FULL",
      message: "fallback",
    });
  });
});

describe("errorFromResponse", () => {
  it("carries status, code and the raw body", async () => {
    const response = new Response(
      JSON.stringify({
        error: {
          code: "STREAM_NOT_FOUND",
          message: "Stream not found or expired",
        },
      }),
      { status: 404, statusText: "Not Found" },
    );

    const error = await errorFromResponse(response);

    expect(error).toBeInstanceOf(ApiError);
    expect(error.status).toBe(404);
    expect(error.code).toBe("STREAM_NOT_FOUND");
    expect(error.message).toBe("Stream not found or expired");
    expect(error.isNotFound).toBe(true);
    expect(error.isRetryable).toBe(false);
  });

  it("marks 5xx and transport failures retryable", async () => {
    const server = await errorFromResponse(new Response("", { status: 503 }));
    expect(server.isRetryable).toBe(true);
    expect(new ApiError("offline", 0).isRetryable).toBe(true);
    expect(new ApiError("offline", 0).isTransport).toBe(true);
  });
});

describe("buildQuery", () => {
  it("omits undefined and null so optional params never reach the daemon", () => {
    expect(
      buildQuery({
        limit: 50,
        status: undefined,
        agent_id: null,
        task_id: "b41c",
      }),
    ).toBe("?limit=50&task_id=b41c");
  });

  it("returns an empty string when nothing survives", () => {
    expect(buildQuery({ a: undefined })).toBe("");
    expect(buildQuery(undefined)).toBe("");
  });
});

/**
 * T21: the daemon rotates `discovery.json`'s token while it runs and honours
 * the one before it only until that one's own expiry, so a window open longer
 * than that holds a token the daemon refuses. A request refused `401` reads
 * the connection again — once, shared by every request refused at the same
 * time — and is sent again only when the token changed.
 */
describe("apiRequest across a token rotation", () => {
  const base = { baseUrl: "http://127.0.0.1:51823", instanceId: "i-1" };
  const fetchMock = vi.fn<typeof fetch>();

  function refused(): Response {
    return new Response("Invalid token", { status: 401 });
  }

  function bearer(call: number): string | null {
    const init = fetchMock.mock.calls[call]?.[1];
    return new Headers(init?.headers).get("Authorization");
  }

  function answers(commands: Record<string, string>): void {
    const answer = (command: string) => () => {
      const token = commands[command];
      if (token === undefined) {
        return Promise.reject(new Error(`unexpected ${command}`));
      }
      return Promise.resolve({ ...base, token, daemonPath: null });
    };
    fixtureHost!.answer("ensureDaemonRunning", answer("ensure_daemon_running"));
    fixtureHost!.answer("getConnectionInfo", answer("get_connection_info"));
  }

  beforeEach(() => {
    resetConnection();
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    fetchMock.mockReset();
    fixtureHost!.reset();
    vi.restoreAllMocks();
  });

  it("re-reads the connection on a 401 and sends the request again with the new token", async () => {
    answers({ ensure_daemon_running: "old", get_connection_info: "new" });
    const reads = vi.spyOn(fixtureHost!, "getConnectionInfo");
    fetchMock
      .mockResolvedValueOnce(refused())
      .mockResolvedValueOnce(Response.json({ ok: true }));

    await expect(
      apiFetch("/v1/memories", { method: "POST", body: { a: 1 } }),
    ).resolves.toEqual({ ok: true });

    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(bearer(0)).toBe("Bearer old");
    expect(bearer(1)).toBe("Bearer new");
    expect(fetchMock.mock.calls[1]?.[1]?.body).toBe('{"a":1}');
    expect(reads).toHaveBeenCalledWith();
    expect(getCachedConnection()?.token).toBe("new");
  });

  it("throws the 401 when the re-read token is the same", async () => {
    answers({ ensure_daemon_running: "same", get_connection_info: "same" });
    fetchMock.mockResolvedValue(refused());

    const error = await apiFetch("/v1/status").catch((e: unknown) => e);

    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).status).toBe(401);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("does not send a request again to another daemon", async () => {
    // The first boot finds i-1; the daemon then restarts as i-2, which the
    // re-read and the re-bootstrap it leads to both find.
    let boots = 0;
    const answer = (first: boolean) =>
      Promise.resolve({
        ...base,
        token: first ? "old" : "new",
        instanceId: first ? "i-1" : "i-2",
        daemonPath: null,
      });
    fixtureHost!.answer("ensureDaemonRunning", () => answer(boots++ === 0));
    fixtureHost!.answer("getConnectionInfo", () => answer(false));
    fetchMock.mockResolvedValue(refused());

    const error = await apiFetch("/v1/status").catch((e: unknown) => e);

    expect((error as ApiError).status).toBe(401);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("sends a request again once at most", async () => {
    answers({ ensure_daemon_running: "old", get_connection_info: "new" });
    fetchMock.mockResolvedValue(refused());

    const error = await apiFetch("/v1/status").catch((e: unknown) => e);

    expect((error as ApiError).status).toBe(401);
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("reads the connection once for every request refused together", async () => {
    answers({ ensure_daemon_running: "old", get_connection_info: "new" });
    const reads = vi.spyOn(fixtureHost!, "getConnectionInfo");
    fetchMock.mockImplementation((_url, init) =>
      Promise.resolve(
        new Headers(init?.headers).get("Authorization") === "Bearer new"
          ? Response.json({ ok: true })
          : refused(),
      ),
    );

    await Promise.all([
      apiFetch("/v1/a"),
      apiFetch("/v1/b"),
      apiFetch("/v1/c"),
    ]);

    expect(reads).toHaveBeenCalledTimes(1);
  });
});
