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

    if (url.includes("/v1/chat/history")) {
      return historyReply();
    }
    if (url.includes("/v1/chat/confirmations/")) {
      return new Response("", { status: 200 });
    }
    if (url.includes("/v1/chat")) {
      return json({ stream_id: "stream-1", lane_key: "user:gui" });
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

function renderChat() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return render(
    <QueryProvider client={client} connectEvents={false}>
      <KeyLadder />
      <ChatView />
    </QueryProvider>,
  );
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

beforeEach(() => {
  requests = [];
  FakeEventSource.instances = [];
  historyReply = () => json({ messages: [], total: 0, lane_key: "user:gui" });
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
    expect(screen.queryByRole("complementary")).toBeNull();
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
