/**
 * The approval card that came back (G1, Round G) — the owner's report: "the
 * request for approval jumps out again … and after click it returns no
 * pending approvals".
 *
 * The cause (`tasks`' diagnosis of 2026-10-03): the chat view's record of the
 * prompts it had seen was component state, the views are swapped, not
 * stacked, and the pending-confirmations snapshot lives in the app-wide Query
 * cache — so a list read *before* an answer was handed to the remount *after*
 * a trip to another view, the answered prompts were drawn again over their
 * own `Approved` rows, and the click went to an id the daemon no longer held
 * (`404 No pending confirmation`), which left the card up.
 *
 * Only the edges are doubled, as in `ChatView.test.tsx`: the Tauri discovery
 * command, `fetch`, `EventSource` and the socket's `onEvent`. The hook
 * (`useChatSession`), the stream reducer, the React Query layer and the
 * WS→cache bridge (`invalidateForEvent`) are the production ones — and so is
 * the **cache configuration**: `createQueryClient()` (staleTime 30 s, gcTime
 * 5 min). `ChatView.test.tsx` builds its client with `gcTime: 0`, which empties
 * the cache the moment the view unmounts — exactly what hid this fault from
 * its own trip-to-another-view test (P2).
 *
 * The fake daemon keeps a broker like `ConfirmationBroker`
 * (`crates/openalpaca_core/src/security/confirmation.rs`): `GET
 * /v1/chat/confirmations` lists what is pending *now*, and `POST
 * /v1/chat/confirmations/{id}` answers 200 for a pending id and
 * `404 {"error":{"code":"NOT_FOUND","message":"No pending confirmation: <id>"}}`
 * for anything else (`apps/openalpacad/src/routes/chat.rs`, `confirm_tool`).
 */

import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import {
  act,
  fireEvent,
  render,
  renderHook,
  screen,
  waitFor,
} from "@testing-library/react";
import { useEffect, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useGlobalKeys } from "@/components/shell";
import {
  CONFIRMATIONS_POLL_BLOCKED_MS,
  CONFIRMATIONS_POLL_MS,
  useChatStream,
} from "@/hooks/useChat";
import {
  clearConfirmationsOver,
  isConfirmationOver,
  noteConfirmationOver,
} from "@/lib/confirmations-over";
import { resetConnection } from "@/lib/connection";
import { daemonEvents, type ServerEvent } from "@/lib/events";
import { createQueryClient, invalidateForEvent } from "@/lib/query-client";
import { qk } from "@/lib/query-keys";
import { QueryProvider } from "@/lib/query-provider";
import { useConfirmationStore } from "@/stores/confirmation";
import { useProjectStore } from "@/stores/project";
import { useSessionSelection } from "@/stores/session";
import { useUiStore } from "@/stores/ui";

import ChatView from "./ChatView";
import { useResolutions } from "./resolution-store";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => ({
    baseUrl: "http://127.0.0.1:9999",
    token: "test-token",
    instanceId: "7f3a1122",
  })),
}));

/* ── transports ──────────────────────────────────────────────────────────── */

type Listener = (event: { data?: unknown }) => void;

class FakeEventSource {
  static instances: FakeEventSource[] = [];
  private readonly listeners = new Map<string, Listener[]>();
  closed = false;

  constructor(readonly url: string) {
    FakeEventSource.instances.push(this);
  }

  addEventListener(type: string, listener: Listener): void {
    const bucket = this.listeners.get(type) ?? [];
    bucket.push(listener);
    this.listeners.set(type, bucket);
  }

  close(): void {
    this.closed = true;
  }

  emit(type: string, data?: unknown): void {
    for (const listener of this.listeners.get(type) ?? []) {
      listener({ data: data === undefined ? undefined : JSON.stringify(data) });
    }
  }
}

interface RecordedRequest {
  url: string;
  method: string;
  body: unknown;
  status: number;
}

let requests: RecordedRequest[] = [];

/** The fake daemon's broker: `request_id → the row the snapshot serves`. */
let broker = new Map<string, Record<string, unknown>>();
/** Swapped by a test to hold a snapshot read open across an answer. */
let snapshotReply: () => Response | Promise<Response>;
/** Set by a test to hold every answer open — a slow round trip. */
let answersHeld: Promise<void> | null = null;

function json(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), { status });
}

function snapshotOfBroker(): Response {
  return json({ confirmations: [...broker.values()] });
}

function installFetch() {
  const fetchMock = vi.fn(async (input: unknown, init?: RequestInit) => {
    const url = String(input);
    const method = init?.method ?? "GET";
    const rawBody = init?.body;
    const record: RecordedRequest = {
      url,
      method,
      body: typeof rawBody === "string" ? JSON.parse(rawBody) : null,
      status: 200,
    };
    requests.push(record);
    const reply = async (): Promise<Response> => {
      const answer = /\/v1\/chat\/confirmations\/([^/?]+)/.exec(url);
      if (answer !== null) {
        if (answersHeld !== null) await answersHeld;
        const id = decodeURIComponent(answer[1] ?? "");
        // `ConfirmationBroker::respond`: remove-and-send, or the `None` arm.
        if (broker.delete(id)) return new Response("", { status: 200 });
        return json(
          {
            error: {
              code: "NOT_FOUND",
              message: `No pending confirmation: ${id}`,
            },
          },
          404,
        );
      }
      if (url.includes("/v1/chat/confirmations")) return await snapshotReply();
      if (url.includes("/v1/status")) {
        return json({
          home_root: "/Users/dev/.openalpaca",
          state_dir: "/Users/dev/.openalpaca/state",
          db_path: "/Users/dev/.openalpaca/state/openalpaca.db",
          project_root: null,
          llm: {
            default_model: "claude-sonnet-4-6",
            default_model_routable: true,
            effective_default_model: "claude-sonnet-4-6",
          },
        });
      }
      if (url.includes("/v1/sessions")) return json({ sessions: [], total: 0 });
      if (url.includes("/v1/chat/history")) {
        return json({
          messages: [],
          total: 0,
          lane_key: "user:gui",
          session_id: null,
        });
      }
      if (url.includes("/v1/chat")) {
        return json({ stream_id: "stream-1", lane_key: "user:gui" });
      }
      if (url.includes("/followups")) return json([]);
      if (url.includes("/v1/tasks")) return json([]);
      if (url.includes("/v1/models")) {
        return json([
          {
            id: "claude-sonnet-4-6",
            provider: "anthropic",
            context_window: 200000,
            input_price_per_million: 3,
            output_price_per_million: 15,
            supports_tools: true,
          },
        ]);
      }
      if (url.includes("/v1/agent-templates")) {
        return json([
          { id: "lead_agent", name: "Lead Agent" },
          { id: "general_agent", name: "General Purpose Agent" },
        ]);
      }
      if (url.includes("/v1/orchestrator/config")) {
        return json({
          model: "claude-sonnet-4-6",
          fallback_models: [],
          active_agents: 0,
          active_tasks: 0,
          daily_cost_usd: 0,
        });
      }
      if (url.includes("/v1/llm/usage/daily")) return json([]);
      return json({ error: "not found" }, 404);
    };
    const response = await reply();
    record.status = response.status;
    return response;
  });
  vi.stubGlobal("fetch", fetchMock);
}

let eventListeners: Array<(event: ServerEvent) => void> = [];

function emitServerEvent(frame: Record<string, unknown>): void {
  const event = {
    ts: new Date().toISOString(),
    instance_id: "7f3a1122",
    _id: eventListeners.length + 1,
    ...frame,
  } as ServerEvent;
  for (const listener of [...eventListeners]) listener(event);
}

function installEventBus() {
  eventListeners = [];
  vi.spyOn(daemonEvents, "onEvent").mockImplementation((listener) => {
    eventListeners.push(listener);
    return () => {
      eventListeners = eventListeners.filter((entry) => entry !== listener);
    };
  });
}

/* ── the window ──────────────────────────────────────────────────────────── */

/** `AppFrame`'s key ladder: Enter and Escape answer the published prompt. */
function KeyLadder() {
  const pending = useConfirmationStore((s) => s.pending);
  useGlobalKeys({
    blocked: pending !== null,
    onApprove: pending?.approve,
    onDeny: pending?.deny,
  });
  return null;
}

/**
 * `App.tsx`'s `AppFrame` in miniature: one view at a time in one slot, so
 * leaving chat **unmounts** `ChatView`; the Query cache, the socket bridge
 * (`query-provider.tsx`) and the key ladder sit above the slot and outlive it.
 */
function Window({ client }: { client: QueryClient }) {
  const view = useUiStore((s) => s.view);
  useEffect(
    () => daemonEvents.onEvent((event) => invalidateForEvent(client, event)),
    [client],
  );
  return (
    <QueryProvider client={client} connectEvents={false}>
      <KeyLadder />
      {view === "chat" ? <ChatView /> : <p>{`the ${view} view`}</p>}
    </QueryProvider>
  );
}

function openWindow(): QueryClient {
  const client = createQueryClient();
  render(<Window client={client} />);
  return client;
}

function eyebrow(tool: string) {
  return (_: string, node: Element | null) =>
    node?.tagName === "P" &&
    node.textContent === `Confirmation required · ${tool}`;
}

function cards(tool: string): HTMLElement[] {
  return screen.queryAllByText(eyebrow(tool));
}

/** Let fetches, query notifications and effects land (fake clock). */
async function settle(): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(50);
  });
}

async function advance(ms: number): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
  await settle();
}

function snapshotGets(): number {
  return requests.filter(
    (r) => r.method === "GET" && /\/v1\/chat\/confirmations(\?|$)/.test(r.url),
  ).length;
}

function answersTo(id: string): RecordedRequest[] {
  return requests.filter(
    (r) =>
      r.method === "POST" && r.url.includes(`/v1/chat/confirmations/${id}`),
  );
}

function cachedSnapshotIds(client: QueryClient): string[] | undefined {
  const rows = client.getQueryData<{ request_id: string }[]>(
    qk.chat.confirmations(),
  );
  return rows?.map((row) => row.request_id);
}

/* ── the daemon's side of a background run ───────────────────────────────── */

const RUN = "run-pasta";
const FILE_WRITE = "req-file-write";
const ARTIFACT_WRITE = "req-artifact-write";

/** A subagent's prompt: no chat stream, no lane, the run's id (`sandbox/mod.rs:367-383`). */
function raise(
  requestId: string,
  toolName: string,
  agentId = "general_agent",
): void {
  const row = {
    request_id: requestId,
    tool_name: toolName,
    tool_arguments: { path: "reports/pasta-night.md" },
    task_id: RUN,
    agent_id: agentId,
    lane_key: null,
    preview: null,
    per_call: false,
    raised_at: new Date().toISOString(),
  };
  broker.set(requestId, row);
  emitServerEvent({
    type: "tool_confirmation_requested",
    request_id: requestId,
    agent_id: agentId,
    tool_name: toolName,
    tool_arguments: row.tool_arguments,
    stream_id: null,
    lane_key: null,
    task_id: RUN,
  });
}

/** What the daemon publishes when its wait ends on an approval (`sandbox/mod.rs:410-426`). */
function resolvedApproved(
  requestId: string,
  toolName: string,
  agentId = "general_agent",
): void {
  emitServerEvent({
    type: "tool_confirmation_resolved",
    request_id: requestId,
    agent_id: agentId,
    tool_name: toolName,
    outcome: "approved",
    stream_id: null,
    lane_key: null,
    task_id: RUN,
  });
  emitServerEvent({
    type: "tool_executed",
    agent_id: agentId,
    tool_name: toolName,
    success: true,
    duration_ms: 12,
    task_id: RUN,
  });
}

/** Send the turn that delegates the run, and let that turn finish. */
async function delegateARun(): Promise<void> {
  fireEvent.change(await screen.findByLabelText("Message"), {
    target: { value: "make a pasta night shopping list" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Send" }));
  await waitFor(() => expect(FakeEventSource.instances).toHaveLength(1));
  const source = FakeEventSource.instances[0];
  if (source === undefined) throw new Error("no stream opened");
  await act(async () => {
    emitServerEvent({
      type: "workflow_started",
      task_id: RUN,
      lane_key: "user:gui",
      title: "Pasta night shopping list",
    });
    source.emit("done", {
      content: "Started a run for the shopping list.",
      model: "qwen3.8:27b-mlx",
      tokens_in: 15372,
      tokens_out: 303,
      duration_ms: 14183,
      delegation: { task_id: RUN, title: "Pasta night shopping list" },
    });
  });
  await settle();
}

/** Click Approve, and let the daemon say what it says after one. */
async function approve(
  requestId: string,
  toolName: string,
  agentId = "general_agent",
): Promise<void> {
  fireEvent.click(screen.getByRole("button", { name: /Approve/ }));
  await waitFor(() => expect(answersTo(requestId)).toHaveLength(1));
  expect(answersTo(requestId)[0]?.status).toBe(200);
  await act(async () => {
    resolvedApproved(requestId, toolName, agentId);
  });
  await settle();
}

/**
 * The owner's run, as `daemon.log` of the scratch store records it
 * (06:37:49 – 06:38:44): a subagent raises `file_write` and `artifact_write`
 * together, both wait ~45 s, both are approved a second apart, the file is
 * written at once, the run finishes ten seconds later.
 *
 * Returns with the chat view showing two `Approved` rows, no card, the
 * composer back, and the FILE WRITTEN card on screen.
 */
async function approveBothPrompts(client: QueryClient): Promise<void> {
  await delegateARun();

  await act(async () => {
    raise(FILE_WRITE, "file_write");
    raise(ARTIFACT_WRITE, "artifact_write");
  });
  expect(await screen.findByText(eyebrow("file_write"))).toBeInTheDocument();
  expect(screen.getByText(eyebrow("artifact_write"))).toBeInTheDocument();

  // The person reads the cards for longer than ten seconds, so the
  // blocked-cadence poll reads the snapshot while both are truly pending.
  const before = snapshotGets();
  await advance(CONFIRMATIONS_POLL_BLOCKED_MS + 100);
  expect(snapshotGets()).toBeGreaterThan(before);
  expect(cachedSnapshotIds(client)).toEqual([FILE_WRITE, ARTIFACT_WRITE]);

  await approve(FILE_WRITE, "file_write");
  await approve(ARTIFACT_WRITE, "artifact_write");
  await act(async () => {
    emitServerEvent({
      type: "artifact_written",
      artifact_id: "art-1",
      task_id: RUN,
      agent_id: "general_agent",
      name: "01-pasta-night.md",
      kind: "markdown",
      version: 1,
      path: "/p/.openalpaca/artifacts/01-pasta-night.md",
    });
  });
  await settle();

  expect(cards("file_write")).toHaveLength(0);
  expect(cards("artifact_write")).toHaveLength(0);
  expect(screen.getAllByText("Approved")).toHaveLength(2);
  expect(screen.getByLabelText("Message")).toBeInTheDocument();
  expect(broker.size).toBe(0);
}

/** FILE WRITTEN → Library; the run finishes while the chat view is gone; back. */
async function tripToTheLibraryAndBack(awayMs: number): Promise<void> {
  fireEvent.click(screen.getByRole("button", { name: "Open in Library" }));
  expect(await screen.findByText("the library view")).toBeInTheDocument();

  await act(async () => {
    emitServerEvent({
      type: "task_status",
      task_id: RUN,
      title: "Pasta night shopping list",
      status: "completed",
      progress_current: null,
      progress_total: null,
      result_summary: "done",
      outcome_kind: "artifact_only",
      artifact_count: 1,
      outcome_summary: "Wrote the list.",
    });
  });
  await advance(awayMs);

  await act(async () => {
    useUiStore.getState().setView("chat");
  });
  await settle();
}

const initialUi = useUiStore.getState();

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  requests = [];
  broker = new Map();
  snapshotReply = snapshotOfBroker;
  answersHeld = null;
  // Module state outlives a test exactly as it outlives a view.
  clearConfirmationsOver();
  FakeEventSource.instances = [];
  resetConnection();
  useUiStore.setState({ ...initialUi, model: null, view: "chat" });
  useResolutions.getState().clear();
  useProjectStore.setState({ path: null });
  useSessionSelection.setState({ selectedId: null });
  vi.stubGlobal("EventSource", FakeEventSource);
  installFetch();
  installEventBus();
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("an answered approval card does not come back (G1, Round G)", () => {
  /**
   * Both prompts were answered in this window, the daemon accepted both
   * answers and said so, and the transcript holds their two `Approved` rows.
   * Coming back to the chat view must not ask again.
   */
  it("prompts answered here are not asked again after a trip to another view", async () => {
    const client = openWindow();
    await approveBothPrompts(client);

    await tripToTheLibraryAndBack(3_000);

    expect(screen.getAllByText("Approved")).toHaveLength(2);
    expect({
      file_write_cards: cards("file_write").length,
      artifact_write_cards: cards("artifact_write").length,
      composer_shown: screen.queryByLabelText("Message") !== null,
    }).toEqual({
      file_write_cards: 0,
      artifact_write_cards: 0,
      composer_shown: true,
    });
  });

  /**
   * The mechanism, step by step — this test pinned the bug before the fix
   * and now pins the fix:
   *
   * 1. nothing refreshes the cached snapshot after the answers, so the cache
   *    still holds what the last poll before them read — the precondition
   *    that made the cards come back;
   * 2. the remount reads the daemon (`refetchOnMount: "always"`) and seeds
   *    nothing from the list it inherited: exactly one read, answering `[]`;
   * 3. no card, so nothing is clicked and no answer goes to a dead id;
   * 4. and the next poll changes nothing.
   */
  it("on return the remount reads the daemon once and draws nothing from the cache it inherited", async () => {
    const client = openWindow();
    await approveBothPrompts(client);
    const getsBeforeTrip = snapshotGets();
    const postsBeforeTrip = requests.filter((r) => r.method === "POST").length;
    // (1)
    expect(cachedSnapshotIds(client)).toEqual([FILE_WRITE, ARTIFACT_WRITE]);

    await tripToTheLibraryAndBack(3_000);

    // (2)
    expect(snapshotGets()).toBe(getsBeforeTrip + 1);
    expect(cachedSnapshotIds(client)).toEqual([]);
    expect(cards("file_write")).toHaveLength(0);
    expect(cards("artifact_write")).toHaveLength(0);
    expect(screen.getAllByText("Approved")).toHaveLength(2);
    // (3)
    expect(screen.getByLabelText("Message")).toBeInTheDocument();
    expect(requests.filter((r) => r.method === "POST")).toHaveLength(
      postsBeforeTrip,
    );
    expect(useUiStore.getState().toast).toBeNull();

    // (4)
    await advance(CONFIRMATIONS_POLL_MS + 100);
    expect(cards("file_write")).toHaveLength(0);
    expect(cards("artifact_write")).toHaveLength(0);
    expect(screen.getAllByText("Approved")).toHaveLength(2);
  });

  /**
   * The other shape in the same log (06:43:37 –
   * 06:44:38): the lead's first `artifact_write` is approved, the lead asks
   * again six seconds later with a new id. If the chat view was away in
   * between, the dead prompt comes back *in front of* the live one, and the
   * composer answers the oldest — so the click meant for the live prompt is
   * spent on the dead id and the run goes on waiting.
   */
  it("after a trip, Approve answers the prompt the daemon is waiting on, not one it already resolved", async () => {
    const client = openWindow();
    await delegateARun();

    await act(async () => {
      raise("req-first", "artifact_write", "lead_agent");
    });
    expect(
      await screen.findByText(eyebrow("artifact_write")),
    ).toBeInTheDocument();
    await advance(CONFIRMATIONS_POLL_BLOCKED_MS + 100);
    expect(cachedSnapshotIds(client)).toEqual(["req-first"]);
    await approve("req-first", "artifact_write", "lead_agent");
    expect(cards("artifact_write")).toHaveLength(0);

    // Off to the Work view; the lead asks again while chat is unmounted.
    await act(async () => {
      useUiStore.getState().setView("work");
    });
    expect(await screen.findByText("the work view")).toBeInTheDocument();
    await act(async () => {
      raise("req-second", "artifact_write", "lead_agent");
    });
    await advance(40_000);
    await act(async () => {
      useUiStore.getState().setView("chat");
    });
    await settle();

    fireEvent.click(screen.getByRole("button", { name: /Approve/ }));
    await waitFor(() =>
      expect(
        answersTo("req-first").length + answersTo("req-second").length,
      ).toBe(2),
    );
    await settle();

    expect({
      cards_on_return_click_went_to:
        answersTo("req-second").length === 1
          ? "req-second (pending)"
          : "req-first (already resolved)",
      status: (answersTo("req-second")[0] ?? answersTo("req-first")[1])?.status,
      toast: useUiStore.getState().toast,
      daemon_still_waiting_on: [...broker.keys()],
    }).toEqual({
      cards_on_return_click_went_to: "req-second (pending)",
      status: 200,
      toast: null,
      daemon_still_waiting_on: [],
    });
  });

  /**
   * Nobody answered here: the prompt timed out while
   * the chat view was unmounted, so no subscriber heard
   * `tool_confirmation_resolved`. A fix that only remembers the ids this
   * window answered does not cover this one.
   */
  it("a prompt that timed out while the chat view was away is not drawn on return", async () => {
    const client = openWindow();
    await delegateARun();
    await act(async () => {
      raise("req-slow", "artifact_write", "lead_agent");
    });
    expect(
      await screen.findByText(eyebrow("artifact_write")),
    ).toBeInTheDocument();
    await advance(CONFIRMATIONS_POLL_BLOCKED_MS + 100);
    expect(cachedSnapshotIds(client)).toEqual(["req-slow"]);

    await act(async () => {
      useUiStore.getState().setView("work");
    });
    expect(await screen.findByText("the work view")).toBeInTheDocument();
    // The broker's own timeout (`ConfirmationBroker::wait`): un-pending, and
    // announced — to a socket nobody in the chat view is listening on.
    await act(async () => {
      broker.delete("req-slow");
      emitServerEvent({
        type: "tool_confirmation_resolved",
        request_id: "req-slow",
        agent_id: "lead_agent",
        tool_name: "artifact_write",
        outcome: "timed_out",
        stream_id: null,
        lane_key: null,
        task_id: RUN,
      });
    });
    await advance(60_000);
    await act(async () => {
      useUiStore.getState().setView("chat");
    });
    await settle();

    expect(cards("artifact_write")).toHaveLength(0);
  });

  /**
   * The same fault with no trip at all, in its narrow form: the card was drawn by the SSE frame and its WebSocket twin never
   * arrived (so `confirmationMeta` has no entry), and a snapshot read that
   * left before the answer lands after it.
   */
  it("a snapshot read before the answer and delivered after it does not re-draw the card", async () => {
    const client = openWindow();
    fireEvent.change(await screen.findByLabelText("Message"), {
      target: { value: "update my persona" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(FakeEventSource.instances).toHaveLength(1));
    const source = FakeEventSource.instances[0];
    if (source === undefined) throw new Error("no stream opened");

    const row = {
      request_id: "req-sse-only",
      tool_name: "update_persona",
      tool_arguments: { section: "USER.md" },
      task_id: null,
      agent_id: "orchestrator",
      lane_key: "user:gui",
      preview: null,
      per_call: false,
      raised_at: new Date().toISOString(),
    };
    broker.set(row.request_id, row);
    await act(async () => {
      source.emit("confirmation_requested", {
        request_id: row.request_id,
        tool_name: row.tool_name,
        tool_arguments: row.tool_arguments,
      });
    });
    expect(
      await screen.findByText(eyebrow("update_persona")),
    ).toBeInTheDocument();

    // A poll leaves; the daemon reads its pending list for it *now*…
    let deliver: (response: Response) => void = () => undefined;
    snapshotReply = () => {
      const readBeforeTheAnswer = snapshotOfBroker();
      return new Promise<Response>((resolve) => {
        deliver = () => resolve(readBeforeTheAnswer);
      });
    };
    const poll = client.refetchQueries({ queryKey: qk.chat.confirmations() });
    await settle();

    // …the answer lands and the card settles…
    fireEvent.click(screen.getByRole("button", { name: /Approve/ }));
    await waitFor(() => expect(answersTo("req-sse-only")).toHaveLength(1));
    await act(async () => {
      source.emit("confirmation_resolved", {
        request_id: "req-sse-only",
        outcome: "approved",
      });
    });
    await settle();
    expect(cards("update_persona")).toHaveLength(0);

    // …and then the poll's answer arrives.
    snapshotReply = snapshotOfBroker;
    await act(async () => {
      deliver(new Response());
      await poll;
    });
    await settle();

    expect(cards("update_persona")).toHaveLength(0);
  });

  /**
   * The cached snapshot was wrong in the other direction too: with data younger than `staleTime` (30 s) the remount issues no read
   * at all, so a prompt raised while the chat view was away — its frame went
   * to a socket nobody in chat was listening on — is not drawn until the next
   * 30 s poll. S9's promise ("a prompt raised before this window opened still
   * gets a card") was late by up to that long on every re-entry.
   */
  it("a prompt raised while the chat view was away is drawn on return", async () => {
    openWindow();
    await delegateARun();

    await act(async () => {
      useUiStore.getState().setView("work");
    });
    expect(await screen.findByText("the work view")).toBeInTheDocument();
    await act(async () => {
      raise("req-while-away", "artifact_write", "lead_agent");
    });
    await advance(3_000);
    await act(async () => {
      useUiStore.getState().setView("chat");
    });
    await settle();

    expect(cards("artifact_write")).toHaveLength(1);
  });

  /**
   * GUARD — passed before the fix and has to keep passing: a prompt that is
   * still pending when the chat view comes back is drawn again and is answerable.
   * Whatever a fix remembers, it must not be "every id this window has seen".
   */
  it("GUARD: a prompt still pending when the chat view comes back is drawn again and answerable", async () => {
    openWindow();
    await delegateARun();
    await act(async () => {
      raise("req-live", "artifact_write", "lead_agent");
    });
    expect(
      await screen.findByText(eyebrow("artifact_write")),
    ).toBeInTheDocument();
    await advance(CONFIRMATIONS_POLL_BLOCKED_MS + 100);

    await act(async () => {
      useUiStore.getState().setView("work");
    });
    expect(await screen.findByText("the work view")).toBeInTheDocument();
    await advance(3_000);
    await act(async () => {
      useUiStore.getState().setView("chat");
    });
    await settle();

    expect(cards("artifact_write")).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: /Approve/ }));
    await waitFor(() => expect(answersTo("req-live")).toHaveLength(1));
    expect(answersTo("req-live")[0]?.status).toBe(200);
    expect(broker.size).toBe(0);
  });

  /**
   * GUARD — the old window's bound, and why `ChatView.test.tsx` never saw the
   * fault: once the cached snapshot has been collected (gcTime, 5 min with no
   * observer — immediately under that suite's `gcTime: 0`), re-entry reads
   * the daemon first and draws nothing.
   */
  it("GUARD: no card comes back once the cached snapshot has been collected (> 5 min away)", async () => {
    const client = openWindow();
    await approveBothPrompts(client);

    await tripToTheLibraryAndBack(5 * 60_000 + 1_000);

    expect(cards("file_write")).toHaveLength(0);
    expect(cards("artifact_write")).toHaveLength(0);
    expect(screen.getByLabelText("Message")).toBeInTheDocument();
  });

  /**
   * Where the memory lives. The reducer still dedupes against the cards on
   * screen only and stays pure; every path that adopts a card into it — the
   * snapshot seed, the SSE frame, the WebSocket twin — goes through one
   * dispatch that drops a prompt the daemon has said is over. And only the
   * daemon's word goes in: a card retired by inference (its turn ended, its
   * run finished, the snapshot stopped listing it) can be drawn again,
   * because the `{owner}:gui` lane is shared (T18) and a later snapshot that
   * still lists it may be right.
   */
  it("the adopt path drops a prompt the daemon said is over, and only that", async () => {
    const client = createQueryClient();
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const { result } = renderHook(() => useChatStream(), { wrapper });
    const shown = () =>
      result.current.state.pendingConfirmations.map((c) => c.request_id);
    const request = (requestId: string) => ({
      request_id: requestId,
      tool_name: "artifact_write",
      tool_arguments: null,
    });

    // The daemon's word, from the socket — whether or not a card was drawn.
    await act(async () => {
      emitServerEvent({
        type: "tool_confirmation_resolved",
        request_id: "req-over",
        agent_id: "lead_agent",
        tool_name: "artifact_write",
        outcome: "approved",
        stream_id: null,
        lane_key: null,
        task_id: RUN,
      });
    });
    expect(isConfirmationOver("req-over")).toBe(true);
    await act(async () => {
      result.current.adoptConfirmation(request("req-over"));
      emitServerEvent({
        type: "tool_confirmation_requested",
        request_id: "req-over",
        agent_id: "lead_agent",
        tool_name: "artifact_write",
        tool_arguments: null,
        stream_id: null,
        lane_key: null,
        task_id: RUN,
      });
    });
    expect(shown()).toEqual([]);

    // An inference retires the card and records nothing.
    await act(async () => {
      result.current.adoptConfirmation(request("req-guessed"));
    });
    expect(shown()).toEqual(["req-guessed"]);
    await act(async () => {
      result.current.dismissConfirmation("req-guessed");
    });
    expect(shown()).toEqual([]);
    expect(isConfirmationOver("req-guessed")).toBe(false);
    await act(async () => {
      result.current.adoptConfirmation(request("req-guessed"));
    });
    expect(shown()).toEqual(["req-guessed"]);

    // Recorded from elsewhere (an answer's 200 or 404) counts the same.
    noteConfirmationOver("req-answered");
    await act(async () => {
      result.current.adoptConfirmation(request("req-answered"));
    });
    expect(shown()).toEqual(["req-guessed"]);
  });

  /**
   * The WebSocket half of the in-flight read: a prompt raised before this
   * window opened is answered from another window while this one's first read
   * of the list is still in flight. The daemon's resolved frame lands first;
   * the read, made before the answer, lands after it.
   */
  it("a read in flight across another window's answer does not draw the card", async () => {
    raise("req-elsewhere", "artifact_write", "lead_agent");
    let deliver: () => void = () => undefined;
    snapshotReply = () => {
      const readBeforeTheAnswer = snapshotOfBroker();
      return new Promise<Response>((resolve) => {
        deliver = () => resolve(readBeforeTheAnswer);
      });
    };
    openWindow();
    expect(await screen.findByLabelText("Message")).toBeInTheDocument();
    await waitFor(() => expect(snapshotGets()).toBe(1));

    await act(async () => {
      broker.delete("req-elsewhere");
      resolvedApproved("req-elsewhere", "artifact_write", "lead_agent");
    });
    snapshotReply = snapshotOfBroker;
    await act(async () => {
      deliver();
    });
    await settle();

    expect(cards("artifact_write")).toHaveLength(0);
    expect(screen.getByLabelText("Message")).toBeInTheDocument();
  });

  /**
   * A click on a card the daemon no longer holds — answered from the CLI or
   * another window while this one missed the frame — is refused
   * `404 No pending confirmation`. That is a resolution, not an error to
   * toast: the card goes, the row says so in a plain sentence, the list is
   * read again, and the card the dead one sat in front of now takes the
   * click.
   */
  it("a 404 settles the card with a plain sentence, re-reads the list, and the next card takes the click", async () => {
    openWindow();
    await delegateARun();
    await act(async () => {
      raise("req-dead", "artifact_write");
      raise("req-live", "file_write");
    });
    expect(
      await screen.findByText(eyebrow("artifact_write")),
    ).toBeInTheDocument();
    expect(screen.getByText(eyebrow("file_write"))).toBeInTheDocument();

    // Answered somewhere else; this window never heard.
    broker.delete("req-dead");
    const getsBeforeClick = snapshotGets();

    fireEvent.click(screen.getByRole("button", { name: /Approve/ }));
    await waitFor(() => expect(answersTo("req-dead")).toHaveLength(1));
    expect(answersTo("req-dead")[0]?.status).toBe(404);
    await settle();

    expect(cards("artifact_write")).toHaveLength(0);
    expect(screen.getByText("Not waiting")).toBeInTheDocument();
    expect(
      screen.getByText(
        "artifact_write was answered already or is no longer waiting, so nothing was changed.",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText("Approved")).toBeNull();
    expect(useUiStore.getState().toast).toBeNull();
    expect(snapshotGets()).toBeGreaterThan(getsBeforeClick);
    expect(isConfirmationOver("req-dead")).toBe(true);

    // The live prompt is the composer's now, and it is answered.
    expect(cards("file_write")).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: /Approve/ }));
    await waitFor(() => expect(answersTo("req-live")).toHaveLength(1));
    expect(answersTo("req-live")[0]?.status).toBe(200);
    expect(answersTo("req-dead")).toHaveLength(1);
    expect(broker.size).toBe(0);
  });

  /** …and *Always allow* on such a card adds nothing to any allowlist. */
  it("Always allow on a prompt the daemon no longer holds says nothing about an allowlist", async () => {
    openWindow();
    await delegateARun();
    await act(async () => {
      raise("req-dead", "artifact_write");
    });
    expect(
      await screen.findByText(eyebrow("artifact_write")),
    ).toBeInTheDocument();
    broker.delete("req-dead");

    fireEvent.click(screen.getByRole("button", { name: "Always allow" }));
    await waitFor(() => expect(answersTo("req-dead")).toHaveLength(1));
    await settle();

    expect(cards("artifact_write")).toHaveLength(0);
    expect(screen.getByText("Not waiting")).toBeInTheDocument();
    expect(useUiStore.getState().toast).toBeNull();
  });

  /**
   * The buttons are disabled while an answer is in flight; the window's key
   * ladder reaches the answer too, and two Enters posted it twice — the
   * second drawing the daemon's 404.
   */
  it("Enter twice while the answer is in flight posts it once", async () => {
    openWindow();
    await delegateARun();
    await act(async () => {
      raise("req-enter", "artifact_write");
    });
    expect(
      await screen.findByText(eyebrow("artifact_write")),
    ).toBeInTheDocument();

    let release: () => void = () => undefined;
    answersHeld = new Promise<void>((resolve) => {
      release = resolve;
    });
    await act(async () => {
      fireEvent.keyDown(window, { key: "Enter" });
      fireEvent.keyDown(window, { key: "Enter" });
    });
    await act(async () => {
      fireEvent.keyDown(window, { key: "Enter" });
    });
    await act(async () => {
      release();
    });
    await settle();

    expect(answersTo("req-enter")).toHaveLength(1);
    expect(answersTo("req-enter")[0]?.status).toBe(200);
    expect(cards("artifact_write")).toHaveLength(0);
    expect(screen.queryByText("Not waiting")).toBeNull();
    expect(screen.getByText("Approved")).toBeInTheDocument();
  });
});
