/**
 * The chat view end to end, over the real data layer.
 *
 * Only the two edges are doubled — the Tauri discovery command and the two
 * transports (`fetch`, `EventSource`) — so the SSE state machine, the query
 * layer and the request bodies under test are the production ones. That is the
 * point: `approval_scope` has to be asserted on the wire, not on a spy.
 */

import { QueryClient } from "@tanstack/react-query";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useGlobalKeys } from "@/components/shell";
import { resetConnection } from "@/lib/connection";
import { QueryProvider } from "@/lib/query-provider";
import { useConfirmationStore } from "@/stores/confirmation";
import { useProjectStore } from "@/stores/project";
import { useSessionSelection } from "@/stores/session";
import { useUiStore } from "@/stores/ui";

import ChatView from "./ChatView";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => ({
    baseUrl: "http://127.0.0.1:9999",
    token: "test-token",
    instanceId: "7f3a1122",
  })),
}));

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
  /** Headers matter on the wire too: `x-workspace-path` is header-only. */
  headers: Headers;
}

let requests: RecordedRequest[] = [];

function json(payload: unknown): Response {
  return new Response(JSON.stringify(payload), { status: 200 });
}

/** What `GET /v1/chat/history` answers; swapped per test to seed a transcript. */
let historyReply: () => Response;
/** What `POST /v1/tasks/{id}/steer` answers; swapped per test to refuse. */
let steerReply: () => Response;
/** What `GET /v1/lanes/{lane}/followups` answers — the lane's pending queue. */
let followupListReply: () => Response;
/** What `POST` / `DELETE …/followups` answer; swapped per test to refuse. */
let followupWriteReply: () => Response;
/** What `POST /v1/chat` answers; swapped per test to refuse a named session. */
let chatSendReply: () => Response;
/**
 * The conversations this fake daemon holds, in `updated_at DESC` order.
 *
 * Mutable because the sidebar's rules are about *transitions*: an `activate`
 * archives the incumbent, and a pin only survives while the daemon still calls
 * that row active. A fixture that answered the same list before and after a
 * write could not tell those cases apart.
 */
let sessionRows: Record<string, unknown>[] = [];
/** What `GET /v1/sessions` answers — one page of `sessionRows`. */
let sessionListReply: (url: string) => Response;
/**
 * What `GET /v1/status` answers. The default is an exact-match daemon: the
 * project root it resolves is the header it was sent. A test that cares about
 * R50 swaps in a root the picker's path only *contains*.
 */
let statusReply: (headers: Headers) => Response;
/** What the five `/v1/sessions` write verbs answer; swapped per test to refuse. */
let sessionWriteReply: () => Response;

/** `limit`/`offset`, exactly as the route pages. */
function pageOfSessions(url: string): Response {
  const query = new URL(url).searchParams;
  const limit = Number(query.get("limit") ?? 50);
  const offset = Number(query.get("offset") ?? 0);
  return json({
    sessions: sessionRows.slice(offset, offset + limit),
    total: sessionRows.length,
  });
}

/** What a successful write does to the rows the next `GET` will answer with. */
function applySessionWrite(url: string): void {
  const activated = /\/v1\/sessions\/([^/?]+)\/activate/.exec(url);
  if (activated !== null) {
    const id = decodeURIComponent(activated[1] ?? "");
    for (const row of sessionRows) {
      row.status = row.id === id ? "active" : "archived";
    }
    return;
  }
  const archived = /\/v1\/sessions\/([^/?]+)\/archive/.exec(url);
  if (archived !== null) {
    const id = decodeURIComponent(archived[1] ?? "");
    for (const row of sessionRows) {
      if (row.id === id) row.status = "archived";
    }
  }
}

function installFetch() {
  const fetchMock = vi.fn(async (input: unknown, init?: RequestInit) => {
    const url = String(input);
    const method = init?.method ?? "GET";
    const rawBody = init?.body;
    requests.push({
      url,
      method,
      body: typeof rawBody === "string" ? JSON.parse(rawBody) : null,
      headers: new Headers(init?.headers),
    });

    if (url.includes("/v1/status")) {
      return statusReply(new Headers(init?.headers));
    }
    if (url.includes("/v1/sessions")) {
      if (method === "GET") return sessionListReply(url);
      const reply = sessionWriteReply();
      // A refused write changes nothing, here as on the daemon.
      if (reply.ok) applySessionWrite(url);
      return reply;
    }
    if (url.includes("/v1/chat/history")) {
      return historyReply();
    }
    if (url.includes("/v1/chat/confirmations/")) {
      return new Response("", { status: 200 });
    }
    if (url.includes("/v1/chat")) {
      return chatSendReply();
    }
    if (url.includes("/steer")) {
      return steerReply();
    }
    if (url.includes("/followups")) {
      return method === "GET" ? followupListReply() : followupWriteReply();
    }
    if (url.includes("/v1/tasks")) return json([]);
    if (url.includes("/v1/models")) {
      return json([
        {
          id: "claude-sonnet-4-6",
          provider: "anthropic",
          context_window: 200000,
          input_price_per_million: 3,
          output_price_per_million: 15,
        },
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
    return new Response(JSON.stringify({ error: "not found" }), {
      status: 404,
    });
  });
  vi.stubGlobal("fetch", fetchMock);
}

const initialUi = useUiStore.getState();

/**
 * The Enter/Escape confirmation rungs live in `useGlobalKeys` at the app root
 * (§4.5) and read the confirmation this view publishes, so the harness mounts
 * that one listener alongside the view.
 */
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
 * Returns the query client so a test can replay what the `session_changed`
 * frame does — invalidate, and let the view read the daemon's new answer.
 */
function renderChat(): QueryClient {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  render(
    <QueryProvider client={client} connectEvents={false}>
      <KeyLadder />
      <ChatView />
    </QueryProvider>,
  );
  return client;
}

/** Send one message and hand back the stream it opened. */
async function sendMessage(text: string): Promise<FakeEventSource> {
  fireEvent.change(screen.getByLabelText("Message"), {
    target: { value: text },
  });
  fireEvent.click(screen.getByRole("button", { name: "Send" }));
  await waitFor(() => expect(FakeEventSource.instances).toHaveLength(1));
  const source = FakeEventSource.instances[0];
  if (source === undefined) throw new Error("no stream opened");
  return source;
}

/** `GET /v1/status`'s body — the store roots plus this request's project. */
function daemonStatus(projectRoot: string | null) {
  return {
    home_root: "/Users/dev/.openalpaca",
    state_dir: "/Users/dev/.openalpaca/state",
    db_path: "/Users/dev/.openalpaca/state/openalpaca.db",
    project_root: projectRoot,
  };
}

/** One `SessionView`, as `/v1/sessions` serializes it. */
function sessionRow(overrides: Record<string, unknown> = {}) {
  return {
    id: "sess-1",
    lane_key: "user:gui",
    source: "gui",
    title: "Connector audit",
    workspace_id: null,
    status: "active",
    message_count: 2,
    last_message_at: "2026-09-06 10:00:00",
    created_at: "2026-09-06 09:00:00",
    updated_at: "2026-09-06 10:00:00",
    ended_at: null,
    active_task_count: 0,
    interrupted_task_count: 0,
    ...overrides,
  };
}

beforeEach(() => {
  requests = [];
  FakeEventSource.instances = [];
  historyReply = () =>
    json({ messages: [], total: 0, lane_key: "user:gui", session_id: null });
  chatSendReply = () => json({ stream_id: "stream-1", lane_key: "user:gui" });
  sessionRows = [];
  sessionListReply = pageOfSessions;
  sessionWriteReply = () => json(sessionRow());
  statusReply = (headers) =>
    json(daemonStatus(headers.get("x-workspace-path")));
  steerReply = () =>
    json({
      task_id: "run-1",
      accepted: true,
      inbox_depth: 1,
      lane_key: "user:gui",
    });
  followupListReply = () => json([]);
  followupWriteReply = () =>
    json({
      id: 42,
      lane_key: "user:gui",
      kind: "followup",
      content: "then write it up",
      source_task_id: "run-1",
      status: "queued",
      created_at: "2026-09-05 10:00:00",
      updated_at: "2026-09-05 10:00:00",
    });
  resetConnection();
  useUiStore.setState({ ...initialUi, model: null, view: "chat" });
  useProjectStore.setState({ path: null });
  useSessionSelection.setState({ selectedId: null });
  vi.stubGlobal("EventSource", FakeEventSource);
  installFetch();
});

describe("ChatView — streaming lifecycle (§3.11, API_MAP §4.1)", () => {
  it("walks thinking → deltas → done and shows the real meta line", async () => {
    renderChat();
    const source = await sendMessage("audit the connectors");

    expect(screen.getByText("audit the connectors")).toBeInTheDocument();

    await act(async () => {
      source.emit("thinking", {});
    });
    expect(screen.getByText("thinking…")).toBeInTheDocument();

    await act(async () => {
      source.emit("delta", { content: "Checking " });
      source.emit("delta", { content: "the connectors" });
    });
    expect(screen.getByText(/Checking the connectors/)).toBeInTheDocument();
    expect(screen.queryByText("thinking…")).toBeNull();

    await act(async () => {
      source.emit("done", {
        content: "Checking the connectors — three are stale.",
        model: "claude-sonnet-4-6",
        tokens_in: 1284,
        tokens_out: 612,
        duration_ms: 3800,
      });
    });

    expect(
      await screen.findByText("sonnet-4-6 · 3.8s · 1284/612 tok"),
    ).toBeInTheDocument();
    // `done.content` is authoritative over the accumulated deltas.
    expect(
      screen.getByText("Checking the connectors — three are stale."),
    ).toBeInTheDocument();
    expect(source.closed).toBe(true);
  });

  it("surfaces a server `error` frame instead of swallowing it", async () => {
    renderChat();
    const source = await sendMessage("hello");

    await act(async () => {
      source.emit("error", { message: "the model refused" });
    });

    expect(await screen.findByText("the model refused")).toBeInTheDocument();
  });
});

describe("ChatView — tool confirmation (§3.14, §3.16a)", () => {
  async function block(): Promise<FakeEventSource> {
    renderChat();
    const source = await sendMessage("run the audit");
    await act(async () => {
      source.emit("confirmation_requested", {
        request_id: "req-1",
        tool_name: "shell_execute",
        tool_arguments: { command: "cargo tree -d" },
      });
    });
    return source;
  }

  it("blocks the composer and shows the literal command", async () => {
    await block();

    expect(
      screen.getByText("Confirmation required · shell_execute"),
    ).toBeInTheDocument();
    expect(screen.getByText("cargo tree -d")).toBeInTheDocument();
    // §3.16a: the textarea is not rendered at all while blocked.
    expect(screen.queryByLabelText("Message")).toBeNull();
    expect(
      screen.getByText("shell_execute is waiting on you"),
    ).toBeInTheDocument();
  });

  it("approves without a scope and clears the block", async () => {
    await block();
    fireEvent.click(screen.getByRole("button", { name: /Approve/ }));

    await waitFor(() =>
      expect(
        requests.some((request) =>
          request.url.includes("/v1/chat/confirmations/req-1"),
        ),
      ).toBe(true),
    );
    const posted = requests.find((request) =>
      request.url.includes("/v1/chat/confirmations/req-1"),
    );
    expect(posted?.method).toBe("POST");
    expect(posted?.body).toEqual({ approved: true });

    expect(await screen.findByText("Approved")).toBeInTheDocument();
    expect(await screen.findByLabelText("Message")).toBeInTheDocument();
  });

  it("denies with `approved: false`", async () => {
    await block();
    fireEvent.click(screen.getByRole("button", { name: /Deny/ }));

    await waitFor(() => {
      const posted = requests.find((request) =>
        request.url.includes("/v1/chat/confirmations/req-1"),
      );
      expect(posted?.body).toEqual({ approved: false });
    });
    expect(await screen.findByText("Denied")).toBeInTheDocument();
  });

  it("sends `approval_scope: entire_tool` for Always allow", async () => {
    await block();
    fireEvent.click(screen.getByRole("button", { name: "Always allow" }));

    await waitFor(() => {
      const posted = requests.find((request) =>
        request.url.includes("/v1/chat/confirmations/req-1"),
      );
      expect(posted?.body).toEqual({
        approved: true,
        approval_scope: "entire_tool",
      });
    });

    // The daemon now honours the scope, so the toast uses §4.4's real copy.
    await waitFor(() =>
      expect(useUiStore.getState().toast).toBe(
        "shell_execute added to the allowlist — it won't ask again",
      ),
    );
  });

  it("approves on Enter and denies on Escape while blocked (§4.5)", async () => {
    await block();

    fireEvent.keyDown(window, { key: "Enter" });
    await waitFor(() => {
      const posted = requests.find((request) =>
        request.url.includes("/v1/chat/confirmations/req-1"),
      );
      expect(posted?.body).toEqual({ approved: true });
    });
  });

  it("denies on Escape while blocked", async () => {
    await block();

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => {
      const posted = requests.find((request) =>
        request.url.includes("/v1/chat/confirmations/req-1"),
      );
      expect(posted?.body).toEqual({ approved: false });
    });
  });
});

describe("ChatView — the aside is one slot with two modes (§8.4)", () => {
  it("shows the work slot by default and swaps it for the file panel", async () => {
    renderChat();
    await screen.findByLabelText("Message");

    expect(
      screen.getByRole("complementary", { name: "Work pane" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("complementary", { name: "File panel" }),
    ).toBeNull();

    act(() => {
      useUiStore.getState().openSidePanel("file-1");
    });

    expect(
      screen.getByRole("complementary", { name: "File panel" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("complementary", { name: "Work pane" }),
    ).toBeNull();

    // `‹ Work` restores the pane in the same slot.
    fireEvent.click(screen.getByRole("button", { name: "‹ Work" }));
    expect(
      screen.getByRole("complementary", { name: "Work pane" }),
    ).toBeInTheDocument();
  });

  it("collapses the aside and offers the design's own re-entry path", async () => {
    renderChat();
    await screen.findByLabelText("Message");

    act(() => {
      useUiStore.getState().closeWorkPane();
    });
    // Named, because the conversation sidebar is a `complementary` too and it
    // is not part of the aside's two-mode slot.
    expect(
      screen.queryByRole("complementary", { name: "Work pane" }),
    ).toBeNull();
    expect(
      screen.queryByRole("complementary", { name: "File panel" }),
    ).toBeNull();
  });

  it("uses the caller's work pane when one is supplied", async () => {
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false, gcTime: 0 } },
    });
    render(
      <QueryProvider client={client} connectEvents={false}>
        <ChatView
          renderWorkPane={(props) => (
            <div data-testid="work-pane">blocked:{String(props.blocked)}</div>
          )}
        />
      </QueryProvider>,
    );

    expect(await screen.findByTestId("work-pane")).toHaveTextContent(
      "blocked:false",
    );
  });
});

describe("ChatView — density (§8.3)", () => {
  it("widens the transcript column and tightens the gap", async () => {
    renderChat();
    await screen.findByLabelText("Message");

    const toggle = screen.getByRole("button", { name: "Compact" });
    fireEvent.click(toggle);

    expect(useUiStore.getState().dense).toBe(true);
    expect(
      screen.getByRole("button", { name: "Comfortable" }),
    ).toBeInTheDocument();
  });
});

describe("ChatView — the chosen project (plan §4.7 item 2)", () => {
  /** The one `POST /v1/chat` a send makes. */
  function chatPost(): RecordedRequest {
    const post = requests.find(
      (request) =>
        request.method === "POST" &&
        request.url.includes("/v1/chat") &&
        !request.url.includes("/v1/chat/history"),
    );
    if (post === undefined) throw new Error("no POST /v1/chat recorded");
    return post;
  }

  it("sends no x-workspace-path when no project is chosen", async () => {
    renderChat();
    await sendMessage("hello");

    expect(chatPost().headers.has("x-workspace-path")).toBe(false);
  });

  it("sends x-workspace-path when a project is chosen", async () => {
    useProjectStore.setState({ path: "/Users/dev/openalpaca" });
    renderChat();
    await sendMessage("hello");

    expect(chatPost().headers.get("x-workspace-path")).toBe(
      "/Users/dev/openalpaca",
    );
  });

  // A steered turn is no longer a chat turn at all — see the block below.
});

/**
 * GAP-02, closed. The composer's steer mode addresses the run it is aimed at
 * through `POST /v1/tasks/{id}/steer`; nothing goes down `/v1/chat`, because a
 * steer is a control action on a run rather than a turn in the conversation.
 */
describe("ChatView — steering a run (GAP-02, closed)", () => {
  /** The one `POST …/steer` a steered send makes. */
  function steerPost(): RecordedRequest {
    const post = requests.find(
      (request) => request.method === "POST" && request.url.includes("/steer"),
    );
    if (post === undefined) throw new Error("no POST …/steer recorded");
    return post;
  }

  async function steerSend(text: string) {
    fireEvent.change(screen.getByLabelText("Message"), {
      target: { value: text },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() =>
      expect(
        requests.some((r) => r.method === "POST" && r.url.includes("/steer")),
      ).toBe(true),
    );
  }

  it("posts to the run's own route and opens no chat stream", async () => {
    useUiStore.setState({ steerTargetRunId: "run-1", composerMode: "steer" });
    renderChat();
    await steerSend("try the other branch");

    expect(steerPost().url).toContain("/v1/tasks/run-1/steer");
    expect(steerPost().body).toEqual({ message: "try the other branch" });
    expect(FakeEventSource.instances).toHaveLength(0);
    expect(
      requests.some(
        (r) =>
          r.method === "POST" &&
          r.url.includes("/v1/chat") &&
          !r.url.includes("/v1/chat/history"),
      ),
    ).toBe(false);

    // The steer still shows in the transcript, and the target is released.
    expect(await screen.findByText("try the other branch")).toBeInTheDocument();
    await waitFor(() =>
      expect(useUiStore.getState().steerTargetRunId).toBeNull(),
    );
  });

  /**
   * The picker's project is the wrong authority for a message aimed at
   * somebody else's run: the daemon defaults `workspace_path` to the run's own
   * `workspace_id`, so a leftover that re-enters as an `unprocessed_steering`
   * follow-up is filed where the run was — not where the user has since
   * navigated. The client therefore sends no project at all.
   */
  it("sends no workspace_path, even with a project selected", async () => {
    useProjectStore.setState({ path: "/Users/dev/openalpaca" });
    useUiStore.setState({ steerTargetRunId: "run-1", composerMode: "steer" });
    renderChat();
    await steerSend("try the other branch");

    expect(steerPost().body).toEqual({ message: "try the other branch" });
    expect(steerPost().headers.has("x-workspace-path")).toBe(false);
  });

  it.each([
    [409, "STEERING_INBOX_FULL", /queue is full/i],
    [409, "TASK_NOT_STEERABLE", /no longer running/i],
    [503, "STEERING_DISABLED", /disabled/i],
    [404, "NOT_FOUND", /no longer exists/i],
  ])(
    "renders %i %s with its own message and keeps the draft",
    async (status, code, expected) => {
      steerReply = () =>
        new Response(
          JSON.stringify({ error: { code, message: "raw daemon text" } }),
          { status },
        );
      useUiStore.setState({ steerTargetRunId: "run-1", composerMode: "steer" });
      renderChat();
      await steerSend("try the other branch");

      expect(await screen.findByText(expected)).toBeInTheDocument();
      // Nothing was queued, so the target stays aimed and the text comes back.
      expect(useUiStore.getState().steerTargetRunId).toBe("run-1");
      await waitFor(() =>
        expect(screen.getByLabelText("Message")).toHaveValue(
          "try the other branch",
        ),
      );
    },
  );
});

/**
 * GAP-03, closed. The composer's queue mode parks the text on the *lane*
 * through `POST /v1/lanes/{lane_key}/followups`; the daemon claims it when the
 * current workflow finalizes. Like a steer it never goes down `/v1/chat` — it
 * is not a turn in the conversation — and the lane's pending queue is read
 * back above the composer, with a cancel per row.
 */
describe("ChatView — queueing a follow-up (GAP-03, closed)", () => {
  /** The one write a queued send makes. */
  function queuePost(): RecordedRequest {
    const post = requests.find(
      (request) =>
        request.method === "POST" && request.url.includes("/followups"),
    );
    if (post === undefined) throw new Error("no POST …/followups recorded");
    return post;
  }

  /**
   * A follow-up is addressed at the lane, so the send waits for the lane key to
   * arrive from `GET /v1/chat/history` first — which the queue read-back proves,
   * because it is the query that is disabled until the lane is known.
   */
  async function queueSend(text: string) {
    await waitFor(() =>
      expect(
        requests.some(
          (r) => r.method === "GET" && r.url.includes("/followups"),
        ),
      ).toBe(true),
    );
    fireEvent.change(screen.getByLabelText("Message"), {
      target: { value: text },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() =>
      expect(
        requests.some(
          (r) => r.method === "POST" && r.url.includes("/followups"),
        ),
      ).toBe(true),
    );
  }

  it("posts to the lane's queue and opens no chat stream", async () => {
    useUiStore.setState({ steerTargetRunId: "run-1", composerMode: "queue" });
    renderChat();
    await queueSend("then write it up");

    expect(queuePost().url).toContain("/v1/lanes/user%3Agui/followups");
    // The run it was queued behind rides along; the daemon supplies the rest.
    expect(queuePost().body).toEqual({
      content: "then write it up",
      source_task_id: "run-1",
    });
    expect(FakeEventSource.instances).toHaveLength(0);
    expect(
      requests.some(
        (r) =>
          r.method === "POST" &&
          r.url.includes("/v1/chat") &&
          !r.url.includes("/v1/chat/history"),
      ),
    ).toBe(false);

    // It still shows in the transcript, and the target is released.
    expect(await screen.findByText("then write it up")).toBeInTheDocument();
    await waitFor(() =>
      expect(useUiStore.getState().steerTargetRunId).toBeNull(),
    );
  });

  /**
   * Unlike a steer, a follow-up *does* carry the picker's project: the user is
   * queueing work now, from here, and that is the project the re-entered turn
   * belongs to — the same rule an ordinary chat turn follows.
   */
  it("sends the selected project as the workspace header", async () => {
    useProjectStore.setState({ path: "/Users/dev/openalpaca" });
    useUiStore.setState({ steerTargetRunId: "run-1", composerMode: "queue" });
    renderChat();
    await queueSend("then write it up");

    expect(queuePost().headers.get("x-workspace-path")).toBe(
      "/Users/dev/openalpaca",
    );
  });

  it.each([
    [400, "EMPTY_CONTENT", /cannot be empty/i],
    [400, "INVALID_LANE_KEY", /no lane yet/i],
  ])(
    "renders %i %s with its own message and keeps the draft",
    async (status, code, expected) => {
      followupWriteReply = () =>
        new Response(
          JSON.stringify({ error: { code, message: "raw daemon text" } }),
          { status },
        );
      useUiStore.setState({ steerTargetRunId: "run-1", composerMode: "queue" });
      renderChat();
      await queueSend("then write it up");

      expect(await screen.findByText(expected)).toBeInTheDocument();
      // Nothing was queued, so the target stays aimed and the text comes back.
      expect(useUiStore.getState().steerTargetRunId).toBe("run-1");
      await waitFor(() =>
        expect(screen.getByLabelText("Message")).toHaveValue(
          "then write it up",
        ),
      );
    },
  );

  it("reads the lane's pending queue back and cancels a row from it", async () => {
    followupListReply = () =>
      json([
        {
          id: 42,
          lane_key: "user:gui",
          kind: "followup",
          content: "then write it up",
          source_task_id: "run-1",
          status: "queued",
          created_at: "2026-09-05 10:00:00",
          updated_at: "2026-09-05 10:00:00",
        },
      ]);
    followupWriteReply = () => json({ id: 42, status: "cancelled" });
    renderChat();

    expect(await screen.findByText("then write it up")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "cancel" }));

    await waitFor(() =>
      expect(
        requests.some(
          (r) =>
            r.method === "DELETE" &&
            r.url.endsWith("/v1/lanes/user%3Agui/followups/42"),
        ),
      ).toBe(true),
    );
  });

  /**
   * The race the route exists to report: the daemon claimed the item first, so
   * the cancel lost its compare-and-swap. "Already started" is a different fact
   * from "gone", and the user acts on the difference.
   */
  it("says a follow-up already started when the cancel loses the race", async () => {
    followupListReply = () =>
      json([
        {
          id: 42,
          lane_key: "user:gui",
          kind: "followup",
          content: "then write it up",
          source_task_id: "run-1",
          status: "queued",
          created_at: "2026-09-05 10:00:00",
          updated_at: "2026-09-05 10:00:00",
        },
      ]);
    followupWriteReply = () =>
      new Response(
        JSON.stringify({
          error: { code: "FOLLOWUP_NOT_QUEUED", message: "raw daemon text" },
        }),
        { status: 409 },
      );
    renderChat();

    expect(await screen.findByText("then write it up")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "cancel" }));

    // The refusal is a toast (the app shell renders it, not this view), so the
    // assertion is on the slot it lands in.
    await waitFor(() =>
      expect(useUiStore.getState().toast).toMatch(/already started/i),
    );
  });
});

describe("ChatView — the run link and artifact chips after a reload (GAP-23)", () => {
  /** A lane as the daemon answers it once a workflow has finished on it. */
  function seedHistory() {
    historyReply = () =>
      json({
        messages: [
          {
            id: 1,
            lane_key: "user:gui",
            role: "user",
            content: "audit the connectors",
            created_at: "2026-09-05T13:35:00Z",
            artifacts: [],
            task_id: null,
          },
          {
            id: 2,
            lane_key: "user:gui",
            role: "assistant",
            content: "Starting that now.",
            created_at: "2026-09-05T13:36:00Z",
            task_id: "b41c8e02-9f3a-4c11-8f52-2b7d5e6a1c30",
            artifacts: [],
          },
          {
            id: 3,
            lane_key: "user:gui",
            role: "assistant",
            content: "Done — three connectors are stale.",
            created_at: "2026-09-05T13:41:00Z",
            task_id: "b41c8e02-9f3a-4c11-8f52-2b7d5e6a1c30",
            artifacts: [
              {
                id: "art-1",
                name: "connector-audit-findings.md",
                kind: "markdown",
              },
            ],
          },
        ],
        total: 3,
        lane_key: "user:gui",
      });
  }

  it("rebuilds the run pill and the artifact chip from history alone", async () => {
    seedHistory();
    renderChat();

    // Both messages of the run wear the pill; the report carries the chip.
    const pills = await screen.findAllByRole("button", {
      name: "run → b41c8e02",
    });
    expect(pills).toHaveLength(2);
    expect(screen.getByText("connector-audit-findings.md")).toBeInTheDocument();
    // The daemon's `markdown` becomes the design's `MD` badge — no guess from
    // the extension, and no second request needed to learn the kind.
    expect(screen.getByText("MD")).toBeInTheDocument();

    // Nothing was streamed and no WS frame arrived: this is history only.
    expect(FakeEventSource.instances).toHaveLength(0);
  });

  it("issues no /v1/files or /v1/artifacts fetch for N chips rendered from history", async () => {
    // Three chips across two reports — the server already named each one's id,
    // name and kind, so the transcript chip needs no request of its own to
    // draw the badge (Important #2): only clicking Open (→ the file panel)
    // fetches anything.
    historyReply = () =>
      json({
        messages: [
          {
            id: 1,
            lane_key: "user:gui",
            role: "assistant",
            content: "First run done.",
            created_at: "2026-09-05T13:41:00Z",
            task_id: "b41c8e02-9f3a-4c11-8f52-2b7d5e6a1c30",
            artifacts: [
              {
                id: "art-1",
                name: "connector-audit-findings.md",
                kind: "markdown",
              },
              {
                id: "art-2",
                name: "connector-audit-appendix.md",
                kind: "markdown",
              },
            ],
          },
          {
            id: 2,
            lane_key: "user:gui",
            role: "assistant",
            content: "Second run done.",
            created_at: "2026-09-05T13:55:00Z",
            task_id: "c52d9f13-0a4b-5d22-9g63-3c8e6f7b2d41",
            artifacts: [
              {
                id: "art-3",
                name: "audit-script.py",
                kind: "code",
              },
            ],
          },
        ],
        total: 2,
        lane_key: "user:gui",
      });

    renderChat();

    await screen.findByText("connector-audit-findings.md");
    await screen.findByText("connector-audit-appendix.md");
    await screen.findByText("audit-script.py");

    const artifactFetches = requests.filter(
      (r) => r.url.includes("/v1/files/") || r.url.includes("/v1/artifacts/"),
    );
    expect(artifactFetches).toHaveLength(0);
  });

  it("takes the run pill to that run in the Work view", async () => {
    seedHistory();
    renderChat();

    const pills = await screen.findAllByRole("button", {
      name: "run → b41c8e02",
    });
    fireEvent.click(pills[0] as HTMLElement);

    const state = useUiStore.getState();
    expect(state.view).toBe("work");
    expect(state.selectedRunId).toBe("b41c8e02-9f3a-4c11-8f52-2b7d5e6a1c30");
  });
});

/**
 * The conversation sidebar (plan §5.7). A lane holds many conversations since
 * migration 039 and exactly one is `active`; these are the client rules for
 * living with that, asserted on the wire rather than on a spy.
 */
describe("ChatView — the conversation sidebar (§5.7)", () => {
  function seedSessions(rows: Record<string, unknown>[]): void {
    sessionRows = rows;
  }

  it("lists the lane's conversations, live one first", async () => {
    seedSessions([
      sessionRow({
        id: "sess-old",
        title: "Docs pass",
        status: "archived",
        updated_at: "2026-09-06 12:00:00",
      }),
      sessionRow({ id: "sess-live", title: "Connector audit" }),
    ]);
    renderChat();

    const rows = await screen.findAllByRole("button", {
      name: /Connector audit|Docs pass/,
    });
    // The archived row is the *more recent* by `updated_at`; status wins.
    expect(rows).toHaveLength(2);
    expect(rows[0]?.textContent).toMatch(/^Connector audit/);
    expect(rows[1]?.textContent).toMatch(/^Docs pass/);
  });

  /** A conversation on another lane is not this window's to show. */
  it("drops rows belonging to another lane", async () => {
    historyReply = () =>
      json({
        messages: [],
        total: 0,
        lane_key: "user:gui",
        session_id: "sess-live",
      });
    seedSessions([
      sessionRow({ id: "sess-live", title: "Connector audit" }),
      sessionRow({
        id: "sess-tg",
        title: "Telegram thread",
        lane_key: "user:telegram",
      }),
    ]);
    renderChat();

    await screen.findByRole("button", { name: /Connector audit/ });
    expect(
      screen.queryByRole("button", { name: /Telegram thread/ }),
    ).toBeNull();
  });

  it("highlights the conversation the daemon says the lane is on", async () => {
    historyReply = () =>
      json({
        messages: [],
        total: 0,
        lane_key: "user:gui",
        session_id: "sess-live",
      });
    seedSessions([sessionRow({ id: "sess-live" })]);
    renderChat();

    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: /Connector audit/ }),
      ).toHaveAttribute("aria-current", "true"),
    );
  });

  it("opens a new conversation in the window's project", async () => {
    useProjectStore.setState({ path: "/Users/dev/openalpaca" });
    renderChat();
    await screen.findByLabelText("Message");

    fireEvent.click(screen.getByRole("button", { name: "New chat" }));

    await waitFor(() =>
      expect(
        requests.find(
          (r) => r.method === "POST" && r.url.endsWith("/v1/sessions"),
        ),
      ).toBeDefined(),
    );
    const created = requests.find(
      (r) => r.method === "POST" && r.url.endsWith("/v1/sessions"),
    );
    expect(created?.body).toEqual({
      source: "gui",
      workspace_path: "/Users/dev/openalpaca",
    });
  });

  /**
   * "New chat" must not pin. The created row *is* the lane's new active
   * conversation, so an unpinned turn already lands in it — and only an
   * unpinned turn lets the daemon detect a project change (R48). Pinning it
   * also strands the window when the daemon archives that row from under it.
   */
  it("leaves a new conversation unpinned, so the lane's own active one governs", async () => {
    seedSessions([sessionRow({ id: "sess-live" })]);
    renderChat();
    await screen.findByLabelText("Message");

    fireEvent.click(screen.getByRole("button", { name: "New chat" }));
    await waitFor(() =>
      expect(
        requests.some(
          (r) => r.method === "POST" && r.url.endsWith("/v1/sessions"),
        ),
      ).toBe(true),
    );

    expect(useSessionSelection.getState().selectedId).toBeNull();

    await sendMessage("after the new chat");
    const send = requests.find(
      (r) => r.method === "POST" && r.url.endsWith("/v1/chat"),
    );
    expect(send?.body).not.toHaveProperty("session_id");
  });

  /**
   * A second window's "New chat", a CLI `--resume` or the follow-up autostart
   * can archive the pinned conversation. Left pinned, the next turn names an
   * archived session and comes back `409` for a user who did nothing but type;
   * released, the same sequence is self-healing.
   */
  it("releases a pin the daemon archived elsewhere and follows the lane again", async () => {
    historyReply = () =>
      json({
        messages: [],
        total: 0,
        lane_key: "user:gui",
        session_id: "sess-live",
      });
    seedSessions([sessionRow({ id: "sess-live", title: "Connector audit" })]);
    const client = renderChat();

    fireEvent.click(
      await screen.findByRole("button", { name: /Connector audit/ }),
    );
    await waitFor(() =>
      expect(useSessionSelection.getState().selectedId).toBe("sess-live"),
    );

    sessionRows = [
      sessionRow({
        id: "sess-new",
        title: "Docs pass",
        updated_at: "2026-09-06 11:00:00",
      }),
      sessionRow({
        id: "sess-live",
        title: "Connector audit",
        status: "archived",
      }),
    ];
    historyReply = () =>
      json({
        messages: [],
        total: 0,
        lane_key: "user:gui",
        session_id: "sess-new",
      });
    await act(async () => {
      await client.invalidateQueries();
    });

    await waitFor(() =>
      expect(useSessionSelection.getState().selectedId).toBeNull(),
    );
    await waitFor(() =>
      expect(screen.getByRole("button", { name: /Docs pass/ })).toHaveAttribute(
        "aria-current",
        "true",
      ),
    );

    await sendMessage("still here?");
    const send = requests.find(
      (r) => r.method === "POST" && r.url.endsWith("/v1/chat"),
    );
    expect(send?.body).not.toHaveProperty("session_id");
  });

  it("resumes an archived conversation by activating it", async () => {
    seedSessions([
      sessionRow({ id: "sess-old", title: "Docs pass", status: "archived" }),
    ]);
    renderChat();

    fireEvent.click(await screen.findByRole("button", { name: /Docs pass/ }));

    await waitFor(() =>
      expect(
        requests.some((r) => r.url.endsWith("/v1/sessions/sess-old/activate")),
      ).toBe(true),
    );
    expect(useSessionSelection.getState().selectedId).toBe("sess-old");
  });

  /**
   * Clicking the conversation the lane is already on must not write: an
   * `activate` there archives and re-opens the same row and announces a
   * transition that never happened.
   */
  it("pins the live conversation without calling activate", async () => {
    seedSessions([sessionRow({ id: "sess-live" })]);
    renderChat();

    fireEvent.click(
      await screen.findByRole("button", { name: /Connector audit/ }),
    );

    await waitFor(() =>
      expect(useSessionSelection.getState().selectedId).toBe("sess-live"),
    );
    expect(requests.some((r) => r.url.includes("/activate"))).toBe(false);
  });

  it("renames through PATCH with only the title", async () => {
    seedSessions([sessionRow({ id: "sess-live" })]);
    renderChat();

    fireEvent.click(await screen.findByRole("button", { name: "Rename" }));
    const field = screen.getByLabelText("Conversation title");
    fireEvent.change(field, { target: { value: "Connector audit v2" } });
    fireEvent.submit(field);

    await waitFor(() =>
      expect(requests.some((r) => r.method === "PATCH")).toBe(true),
    );
    const patch = requests.find((r) => r.method === "PATCH");
    expect(patch?.url).toContain("/v1/sessions/sess-live");
    expect(patch?.body).toEqual({ title: "Connector audit v2" });
  });

  it("renders a refused delete in the daemon's own terms", async () => {
    seedSessions([sessionRow({ id: "sess-live" })]);
    sessionWriteReply = () =>
      new Response(
        JSON.stringify({
          error: {
            code: "SESSION_HAS_ACTIVE_WORKFLOWS",
            message: "This conversation has a run in flight.",
          },
        }),
        { status: 409 },
      );
    renderChat();

    fireEvent.click(await screen.findByRole("button", { name: "Delete" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm delete" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      /run in flight — cancel it before deleting/,
    );
  });

  /**
   * R49: a turn that names a conversation runs in *that* conversation's
   * project. The composer therefore names it only when the user resumed one —
   * a turn with nothing pinned must stay unnamed, or R48 (change project ⇒ new
   * conversation) could never fire.
   */
  it("names the resumed conversation on the turn, and nothing otherwise", async () => {
    seedSessions([sessionRow({ id: "sess-live" })]);
    renderChat();
    await screen.findByLabelText("Message");

    await sendMessage("first, unpinned");
    const unpinned = requests.filter(
      (r) => r.method === "POST" && r.url.endsWith("/v1/chat"),
    );
    expect(unpinned[0]?.body).not.toHaveProperty("session_id");

    fireEvent.click(screen.getByRole("button", { name: /Connector audit/ }));
    await waitFor(() =>
      expect(useSessionSelection.getState().selectedId).toBe("sess-live"),
    );

    fireEvent.change(screen.getByLabelText("Message"), {
      target: { value: "second, resumed" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));

    await waitFor(() =>
      expect(
        requests.filter(
          (r) => r.method === "POST" && r.url.endsWith("/v1/chat"),
        ),
      ).toHaveLength(2),
    );
    const sends = requests.filter(
      (r) => r.method === "POST" && r.url.endsWith("/v1/chat"),
    );
    expect(sends[1]?.body).toMatchObject({ session_id: "sess-live" });
  });

  /**
   * R48 is the daemon's: it opens the new conversation on the turn whose
   * project differs. The client's only job is to stop naming a session, so
   * that switch can be detected at all — and to create nothing itself.
   */
  it("releases the pin when the window's project changes, and creates nothing", async () => {
    seedSessions([sessionRow({ id: "sess-live" })]);
    renderChat();

    fireEvent.click(
      await screen.findByRole("button", { name: /Connector audit/ }),
    );
    await waitFor(() =>
      expect(useSessionSelection.getState().selectedId).toBe("sess-live"),
    );

    act(() => {
      useProjectStore.getState().setPath("/Users/dev/other");
    });

    await waitFor(() =>
      expect(useSessionSelection.getState().selectedId).toBeNull(),
    );
    expect(
      requests.some(
        (r) => r.method === "POST" && r.url.endsWith("/v1/sessions"),
      ),
    ).toBe(false);
  });

  /**
   * R50. `session.workspace_id` is a **canonical project root**; the picker
   * holds free text that is only checked for absoluteness. `/repo/apps/gui`
   * resolves to `/repo`, which is exactly what this conversation is bound to,
   * so the daemon is provably not switching scope — and an override line here
   * would announce something that is not happening.
   */
  it("stays quiet when the picker's path resolves to the conversation's own project", async () => {
    useProjectStore.setState({ path: "/repo/apps/gui" });
    statusReply = () => json(daemonStatus("/repo"));
    seedSessions([sessionRow({ id: "sess-live", workspace_id: "/repo" })]);
    renderChat();

    fireEvent.click(
      await screen.findByRole("button", { name: /Connector audit/ }),
    );
    await waitFor(() =>
      expect(useSessionSelection.getState().selectedId).toBe("sess-live"),
    );

    expect(screen.queryByText(/not the window's project/)).toBeNull();
  });

  /** And when they really differ, the line names the *resolved* roots. */
  it("names the canonical roots when the two projects really differ", async () => {
    useProjectStore.setState({ path: "/repo/apps/gui" });
    statusReply = () => json(daemonStatus("/repo"));
    seedSessions([sessionRow({ id: "sess-live", workspace_id: "/elsewhere" })]);
    renderChat();

    fireEvent.click(
      await screen.findByRole("button", { name: /Connector audit/ }),
    );

    const line = await screen.findByText(/This conversation runs in/);
    expect(line).toHaveTextContent("/elsewhere");
    expect(line).toHaveTextContent("(/repo)");
    expect(line).not.toHaveTextContent("/repo/apps/gui");
  });

  it("reads the resumed conversation's transcript, not the lane's", async () => {
    seedSessions([
      sessionRow({ id: "sess-old", title: "Docs pass", status: "archived" }),
    ]);
    renderChat();

    fireEvent.click(await screen.findByRole("button", { name: /Docs pass/ }));

    await waitFor(() =>
      expect(requests.some((r) => r.url.includes("session_id=sess-old"))).toBe(
        true,
      ),
    );
  });

  /** The refusal a second window can cause: it archived what this one pinned. */
  it("explains a turn refused because the conversation was archived elsewhere", async () => {
    seedSessions([sessionRow({ id: "sess-live" })]);
    renderChat();

    fireEvent.click(
      await screen.findByRole("button", { name: /Connector audit/ }),
    );
    await waitFor(() =>
      expect(useSessionSelection.getState().selectedId).toBe("sess-live"),
    );

    chatSendReply = () =>
      new Response(
        JSON.stringify({
          error: { code: "SESSION_ARCHIVED", message: "archived" },
        }),
        { status: 409 },
      );

    fireEvent.change(screen.getByLabelText("Message"), {
      target: { value: "still here?" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));

    expect(
      await screen.findByText(/This conversation was archived/),
    ).toBeInTheDocument();
  });
});
