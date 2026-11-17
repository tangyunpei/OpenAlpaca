/**
 * The API contract's request half, this side (CLI-PROTOCOL; *Rev 107*'s
 * request-fields item, (b)): every body a wrapper in `./` (and
 * `lib/chat-stream.ts`'s `sendChatMessage`) sends holds only
 * keys the daemon's request type reads, as `./contract/requests/` records
 * them — each sample written by the daemon's tests from the very type the
 * route deserializes (`apps/openalpacad/src/contract/requests.rs`). A field
 * the daemon renames fails its bump check there; once `PROTOCOL` moved and
 * the folder was rewritten, the wrapper still sending the old name fails
 * here, naming the sample and the key.
 *
 * Only the two edges are doubled — the Tauri discovery command and `fetch`
 * — so each body is the one the daemon would receive. A wrapper given a
 * typed request is called with every key of its type (`Required<…>`), so
 * an optional key is checked too. A sample's value that is `null` is a
 * field whose inside the sample does not describe; what a wrapper sends
 * under it is not checked.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";

import { sendChatMessage } from "@/lib/chat-stream";
import { resetConnection } from "@/lib/connection";

import {
  createAgentTemplate,
  performAgentAction,
  updateAgentConfig,
  updateAgentTemplate,
} from "./agents";
import { setArtifactPinned } from "./artifacts";
import { respondToConfirmation, setMessageFeedback } from "./chat";
import {
  configureConnector,
  performConnectorAction,
  updateConnectorSettings,
} from "./connectors";
import {
  addMcpServer,
  installPlugin,
  setExtensionConfig,
  updatePlugin,
  validatePlugin,
} from "./extensions";
import { queueFollowup } from "./followups";
import {
  createMemory,
  updateMemory,
  type CreateMemoryInput,
  type MemoryPatch,
} from "./memories";
import { updateOrchestratorConfig } from "./orchestrator";
import { putPersonaUser } from "./persona";
import { createSession, updateSession } from "./sessions";
import {
  reorderKeys,
  setKeyPriority,
  setProviderEnabled,
  upsertKey,
  validateKey,
} from "./settings";
import { requestDaemonShutdown } from "./status";
import { createTask, performTaskAction, steerTask } from "./tasks";
import type {
  AddKeyRequest,
  AgentConfigFile,
  CreateTaskRequest,
  McpDeclaration,
  ReorderKeysRequest,
  SetKeyPriorityRequest,
  UpdateOrchestratorRequest,
  ValidateKeyRequest,
} from "./types";
import { chooseProject, deleteProjectData, repointProject } from "./workspaces";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => ({
    baseUrl: "http://127.0.0.1:9999",
    token: "test-token",
    instanceId: "7f3a1122",
  })),
}));

// ── The samples ─────────────────────────────────────────────────────────────

const requestSamples = new Map(
  Object.entries(
    import.meta.glob("./contract/requests/*.json", {
      eager: true,
      import: "default",
    }) as Record<string, unknown>,
  ).map(([path, value]) => [
    path.replace(/^.*\//, "").replace(/\.json$/, ""),
    value,
  ]),
);

// ── What the wrappers send ──────────────────────────────────────────────────

interface Sent {
  method: string;
  body: unknown;
}

let sent: Sent[] = [];
let reply: () => Response;

function json(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), {
    status,
    headers: { ETag: '"e2"' },
  });
}

beforeEach(() => {
  sent = [];
  resetConnection();
  reply = () => json({});
  vi.stubGlobal(
    "fetch",
    vi.fn(async (_input: unknown, init?: RequestInit) => {
      sent.push({
        method: init?.method ?? "GET",
        body: typeof init?.body === "string" ? JSON.parse(init.body) : null,
      });
      return reply();
    }),
  );
});

/**
 * Every key path `body` holds that `sample` does not: an object's keys
 * against the sample's object, an array's elements against the sample's
 * one element; a `null` in the sample is not looked inside.
 */
function unknownKeys(body: unknown, sample: unknown, path = ""): string[] {
  if (sample === null) return [];
  if (Array.isArray(body)) {
    if (!Array.isArray(sample)) return [];
    const element = sample[0];
    if (element === undefined) return [];
    return body.flatMap((item) => unknownKeys(item, element, `${path}[]`));
  }
  if (typeof body !== "object" || body === null) return [];
  if (typeof sample !== "object" || Array.isArray(sample)) return [];
  const known = sample as Record<string, unknown>;
  return Object.entries(body as Record<string, unknown>).flatMap(
    ([key, value]) => {
      const here = path === "" ? key : `${path}.${key}`;
      if (!(key in known)) return [here];
      return unknownKeys(value, known[key], here);
    },
  );
}

// ── Every key of each typed request ─────────────────────────────────────────

const AGENT_CONFIG: AgentConfigFile = {
  agent: { id: "a", name: "A", description: "d", icon: "i" },
  capabilities: { assigned: ["web_access"], denied: ["shell"] },
  preset: { persona: "p", temperature: 0.2, verbosity: "brief" },
  constraints: {
    max_tool_calls: 1,
    timeout_seconds: 1,
    max_cost_per_task: 1,
    max_rounds: 1,
    require_confirmation_for: ["shell"],
    allowed_capabilities: ["web_access"],
    denied_capabilities: ["shell"],
  } satisfies Required<NonNullable<AgentConfigFile["constraints"]>>,
  llm: { model: "m", fallback_models: ["f"] } satisfies Required<
    NonNullable<AgentConfigFile["llm"]>
  >,
};

const ADD_KEY: AddKeyRequest = {
  provider: "openai",
  key: {
    id: "k",
    secret: "not-a-secret",
    tier: "t",
    priority: "primary",
    source: "api_console",
    notes: "n",
  } satisfies Required<AddKeyRequest["key"]>,
};

const MCP: Required<McpDeclaration> = {
  name: "github",
  transport: "stdio",
  command: "npx",
  args: ["-y"],
  env: { A: "b" },
  cwd: "/tmp/x",
  url: "http://127.0.0.1:1",
  bearer_env: "B",
  api_key_header: "X-Key",
  api_key_env: "K",
  connect_timeout_secs: 1,
  request_timeout_secs: 1,
  enabled: true,
};

const MEMORY: Required<CreateMemoryInput> = {
  title: "t",
  body: "b",
  kind: "fact",
  scope: "global",
  aliases: ["a"],
  tags: ["x"],
  fact_key: "k",
};

const PATCH: Required<MemoryPatch> = {
  title: "t",
  body: "b",
  aliases: ["a"],
  tags: ["x"],
  fact_key: null,
};

const REV = "a".repeat(64);
const ID = "55e811b6-3220-4758-a47b-034bd1023f38";

/** Each sample and a call of a wrapper that sends its route's body. */
const CASES: Array<[string, () => Promise<unknown>]> = [
  ["post-agent-templates", () => createAgentTemplate(AGENT_CONFIG)],
  ["put-agent-templates-id", () => updateAgentTemplate("a", AGENT_CONFIG)],
  ["put-agents-id-config", () => updateAgentConfig("a", AGENT_CONFIG, 1)],
  ["post-agents-id-action", () => performAgentAction("a", "pause")],
  ["put-artifacts-id-pin", () => setArtifactPinned("a", true)],
  [
    "post-chat",
    () =>
      sendChatMessage({
        content: "c",
        attachments: [{ file_id: "f", caption: "c" }],
        model: "m",
        workspacePath: "/r",
        sessionId: "s",
      }),
  ],
  [
    "post-chat-confirmations-id",
    () =>
      respondToConfirmation({
        requestId: "r",
        approved: true,
        approvalScope: "entire_tool",
      }),
  ],
  [
    "put-chat-messages-id-feedback",
    () => setMessageFeedback(1, "positive", "c"),
  ],
  ["post-connectors-id-action", () => performConnectorAction("c", "enable")],
  ["post-connectors-id-config", () => configureConnector("c", "t")],
  [
    "put-connectors-id-settings",
    () => updateConnectorSettings("c", { a: "b" }),
  ],
  ["post-extensions-kind-id-config", () => setExtensionConfig("p", "k", "v")],
  ["post-extensions-plugin", () => installPlugin("/tmp/p")],
  ["post-extensions-plugin-validate", () => validatePlugin("/tmp/p")],
  ["put-extensions-plugin-id", () => updatePlugin("p", "/tmp/p")],
  ["post-extensions-mcp", () => addMcpServer(MCP)],
  ["post-lanes-key-followups", () => queueFollowup("o:gui", "c", "t", "/r")],
  ["post-memories", () => createMemory(MEMORY, null)],
  [
    "post-memories-id",
    () =>
      updateMemory(
        ID,
        REV,
        { action: "edit", patch: PATCH, reason: "r" },
        null,
      ),
  ],
  [
    "post-memories-id",
    () =>
      updateMemory(
        ID,
        REV,
        { action: "confirm", target_revision: REV, reason: "r" },
        null,
      ),
  ],
  [
    "put-orchestrator-config",
    () =>
      updateOrchestratorConfig({
        model: "m",
        fallback_models: ["f"],
      } satisfies Required<UpdateOrchestratorRequest>),
  ],
  ["put-persona-user", () => putPersonaUser("c", "e1")],
  [
    "post-sessions",
    () =>
      createSession({
        source: "gui",
        workspacePath: "/r",
        title: "t",
        inherit: false,
      }),
  ],
  [
    "patch-sessions-id",
    () => updateSession("s", { title: "t", workspacePath: "/r" }),
  ],
  ["put-settings-llm", () => upsertKey(ADD_KEY)],
  [
    "put-settings-llm-keys-reorder",
    () =>
      reorderKeys({
        provider: "openai",
        key_order: ["k"],
        primary_key_id: "k",
      } satisfies Required<ReorderKeysRequest>),
  ],
  [
    "put-settings-llm-keys-priority",
    () =>
      setKeyPriority({
        provider: "openai",
        key_id: "k",
        priority: "primary",
      } satisfies Required<SetKeyPriorityRequest>),
  ],
  [
    "put-settings-llm-providers-provider-enabled",
    () => setProviderEnabled("openai", true),
  ],
  [
    "post-settings-llm-validate",
    () =>
      validateKey({
        provider: "openai",
        secret: "not-a-secret",
      } satisfies Required<ValidateKeyRequest>),
  ],
  [
    "post-tasks",
    () =>
      createTask({
        title: "t",
        description: "d",
        priority: 1,
        created_by: "o",
        source_lane: "o:gui",
      } satisfies Required<CreateTaskRequest>),
  ],
  ["post-tasks-id-action", () => performTaskAction("t", "cancel")],
  ["post-tasks-id-steer", () => steerTask("t", "m", "/r")],
  ["post-workspaces", () => chooseProject("/r")],
  ["patch-workspaces", () => repointProject("/a", "/b")],
  ["post-workspaces-purge", () => deleteProjectData("/r", true)],
];

describe("the API contract's request samples", () => {
  it.each(CASES)(
    "requests/%s holds every key its wrapper sends",
    async (name, call) => {
      await call();
      expect(sent).toHaveLength(1);
      const request = sent[0];
      if (request === undefined) throw new Error("no request");
      // A sample is named after its route, its method first.
      expect(name.startsWith(`${request.method.toLowerCase()}-`)).toBe(true);
      const sample = requestSamples.get(name);
      expect(sample, `requests/${name}.json`).toBeDefined();
      expect(
        unknownKeys(request.body, sample),
        `keys requests/${name}.json does not hold`,
      ).toEqual([]);
    },
  );

  it("the daemon's shutdown is asked with the command body", async () => {
    reply = () => json({ status: "shutting_down" });
    await requestDaemonShutdown();
    const request = sent[0];
    expect(request?.method).toBe("POST");
    const sample = requestSamples.get("post-command");
    expect(sample, "requests/post-command.json").toBeDefined();
    expect(unknownKeys(request?.body, sample)).toEqual([]);
  });

  it("a key the sample does not hold is named, at its path", () => {
    expect(
      unknownKeys(
        { a: 1, b: { c: 1, d: 2 }, e: [{ f: 1, g: 2 }], h: { i: 1 } },
        { a: null, b: { c: null }, e: [{ f: null }], h: null },
      ),
    ).toEqual(["b.d", "e[].g"]);
  });
});
