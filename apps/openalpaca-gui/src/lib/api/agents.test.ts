/**
 * `DELETE /v1/agent-templates/{id}` — its two refusals (subagent design
 * §2.10 T-2): a built-in template no file holds, and a plugin's template
 * (PLUGIN-TEMPLATE-409), each `409 {error: "<word>", message}`. The shared
 * parser hands a string `error` through as the message, so without its own
 * reading a caller would show the bare word, not the sentence.
 */

import { beforeEach, describe, expect, expectTypeOf, it, vi } from "vitest";

import { resetConnection } from "@/lib/connection";
import { fixtureHost } from "@/lib/host/fixture";
import { ApiError } from "@/lib/http";

import { deleteAgentTemplate, listAgentTemplates } from "./agents";
import type { AgentTemplate } from "./types";

beforeEach(() => {
  fixtureHost!.answerConnection({
    baseUrl: "http://127.0.0.1:9999",
    token: "test-token",
    instanceId: "7f3a1122",
  });
});

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

  // DELETE-TEMPLATE-TYPE (item 25 of the owner's answer of 2026-11-14): the
  // declared answer is the route's, `{template_id, status}`. The type half
  // is checked by `bun run check`, which type-checks this file.
  it("declares the route's answer, {template_id, status}", async () => {
    const answer = await deleteAgentTemplate("reader");
    expectTypeOf(answer).toEqualTypeOf<{
      template_id: string;
      status: string;
    }>();
    expect(answer).toEqual({ template_id: "reader", status: "archived" });
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

// TEMPLATE-TEMP-OPTIONAL (item 24 of the owner's answer of 2026-11-14): a
// template that sets no `temperature` is listed without one, and the type
// says so. The type half is checked by `bun run check`.
describe("listAgentTemplates", () => {
  it("reads a template that sets no temperature", async () => {
    const row: AgentTemplate = {
      id: "reader",
      name: "Reader",
      description: "Reads things",
      singleton: false,
      capabilities: ["file_read"],
      denied_capabilities: [],
      verbosity: "normal",
      fallback_models: [],
      require_confirmation_for: [],
      persona: "You read.",
      body: "## Persona\n\nYou read.",
      run_count: 0,
      window: "7d",
    };
    reply = () => new Response(JSON.stringify([row]));
    const [listed] = await listAgentTemplates();
    expectTypeOf<AgentTemplate["temperature"]>().toEqualTypeOf<
      number | undefined
    >();
    expect(listed?.temperature).toBeUndefined();
    expect(listed).toEqual(row);
  });
});
