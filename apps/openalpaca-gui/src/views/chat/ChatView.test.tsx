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
}

let requests: RecordedRequest[] = [];

function json(payload: unknown): Response {
  return new Response(JSON.stringify(payload), { status: 200 });
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
    });

    if (url.includes("/v1/chat/history")) {
      return json({ messages: [], total: 0, lane_key: "user:gui" });
    }
    if (url.includes("/v1/chat/confirmations/")) {
      return new Response("", { status: 200 });
    }
    if (url.includes("/v1/chat")) {
      return json({ stream_id: "stream-1", lane_key: "user:gui" });
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
  resetConnection();
  useUiStore.setState({ ...initialUi, model: null, view: "chat" });
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
