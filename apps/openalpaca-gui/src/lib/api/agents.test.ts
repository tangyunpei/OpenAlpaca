/**
 * `DELETE /v1/agent-templates/{id}` — its two refusals (subagent design
 * §2.10 T-2): a built-in template no file holds, and a plugin's template
 * (PLUGIN-TEMPLATE-409), each `409 {error: "<word>", message}`. The shared
 * parser hands a string `error` through as the message, so without its own
 * reading a caller would show the bare word, not the sentence.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";

import { resetConnection } from "@/lib/connection";
import { ApiError } from "@/lib/http";

import { deleteAgentTemplate } from "./agents";

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
}

let requests: Recorded[] = [];
let reply: () => Response;

beforeEach(() => {
  requests = [];
  resetConnection();
  reply = () =>
    new Response(JSON.stringify({ template_id: "reader", status: "archived" }));
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: unknown, init?: RequestInit) => {
      requests.push({
        url: new URL(String(input)),
        method: init?.method ?? "GET",
      });
      return reply();
    }),
  );
});

async function refusal(): Promise<ApiError> {
  const error: unknown = await deleteAgentTemplate("acme agent").catch(
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(ApiError);
  return error as ApiError;
}

describe("deleteAgentTemplate", () => {
  it("sends DELETE to the template's encoded path", async () => {
    await deleteAgentTemplate("acme agent");
    expect(requests[0]?.method).toBe("DELETE");
    expect(requests[0]?.url.pathname).toBe("/v1/agent-templates/acme%20agent");
  });

  it("shows a plugin's refusal as its sentence, naming the plugin", async () => {
    const message =
      "a plugin's template cannot be removed; it comes from the plugin acme";
    reply = () =>
      new Response(
        JSON.stringify({ error: "plugin", plugin: "acme", message }),
        { status: 409 },
      );
    const error = await refusal();
    expect(error.status).toBe(409);
    expect(error.code).toBe("plugin");
    expect(error.message).toBe(message);
    expect(error.detail).toEqual({ error: "plugin", plugin: "acme", message });
  });

  it("shows a built-in template's refusal as its sentence", async () => {
    const message = "a built-in template cannot be removed";
    reply = () =>
      new Response(JSON.stringify({ error: "bundled", message }), {
        status: 409,
      });
    const error = await refusal();
    expect(error.code).toBe("bundled");
    expect(error.message).toBe(message);
  });

  it("leaves any other error as the shared parser reads it", async () => {
    reply = () =>
      new Response(JSON.stringify({ error: "Template 'x' not found" }), {
        status: 404,
      });
    const error = await refusal();
    expect(error.status).toBe(404);
    expect(error.code).toBeNull();
    expect(error.message).toBe("Template 'x' not found");
  });
});
