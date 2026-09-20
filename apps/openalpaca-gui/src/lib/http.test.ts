import { describe, expect, it } from "vitest";

import {
  ApiError,
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
