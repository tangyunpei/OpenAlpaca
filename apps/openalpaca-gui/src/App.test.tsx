/**
 * The assembled frame: the rail, the four real views, the overlays and the one
 * global key listener (DESIGN_SPEC §2, §4.5).
 *
 * This is the integration seam, so nothing below the transports is doubled —
 * the real views mount over a stubbed `fetch` and a stubbed Tauri discovery
 * command. The event socket is left closed (`connectEvents={false}`); the
 * event → cache bridge has its own unit tests.
 */

import { QueryClient } from "@tanstack/react-query";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { resetConnection } from "@/lib/connection";
import { QueryProvider } from "@/lib/query-provider";
import { useConfirmationStore } from "@/stores/confirmation";
import { useUiStore } from "@/stores/ui";

import { AppFrame } from "./App";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => ({
    baseUrl: "http://127.0.0.1:9999",
    token: "test-token",
    instanceId: "7f3a1122",
  })),
}));

function json(payload: unknown): Response {
  return new Response(JSON.stringify(payload), { status: 200 });
}

/** Every route the four views touch, answered with an empty-but-valid body. */
function installFetch() {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: unknown) => {
      const url = String(input);
      if (url.includes("/v1/chat/history")) {
        return json({ messages: [], total: 0, lane_key: "user:gui" });
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
      if (url.includes("/v1/health")) {
        return json({
          status: "ok",
          version: "0.1.0",
          instance_id: "7f3a1122",
        });
      }
      if (url.includes("/v1/settings/llm")) return json({ providers: {} });
      if (url.includes("/v1/conversations")) {
        return json({ conversations: [], total: 0 });
      }
      // tasks, models, connectors, plugins, skills, templates, usage — all lists
      return json([]);
    }),
  );
}

const initialUi = useUiStore.getState();

function renderFrame() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return render(
    <QueryProvider client={client} connectEvents={false}>
      <AppFrame />
    </QueryProvider>,
  );
}

beforeEach(() => {
  resetConnection();
  installFetch();
  useConfirmationStore.setState({ pending: null });
  useUiStore.setState({
    ...initialUi,
    view: "chat",
    paletteOpen: false,
    pickerOpen: false,
    panelArtifactId: null,
    workOpen: true,
    dense: false,
    toast: null,
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("AppFrame", () => {
  it("mounts the rail beside the real chat view", async () => {
    renderFrame();

    const rail = screen.getByRole("navigation", { name: "Primary" });
    expect(within(rail).getByRole("button", { name: "Chat" })).toHaveAttribute(
      "aria-current",
      "page",
    );
    // The chat pane's own header, not the rail's nav item.
    expect(
      await screen.findByRole("heading", { name: "Chat", level: 1 }),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("Message")).toBeInTheDocument();
  });

  it("swaps in each real view without unmounting the rail", async () => {
    renderFrame();
    await screen.findByRole("heading", { name: "Chat", level: 1 });

    fireEvent.click(screen.getByRole("button", { name: "Work" }));
    expect(
      await screen.findByRole("heading", { name: "Work", level: 1 }),
    ).toBeInTheDocument();
    expect(screen.queryByLabelText("Message")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Library" }));
    expect(await screen.findByRole("region", { name: "Library" })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(
      await screen.findByRole("region", { name: "Settings" }),
    ).toBeTruthy();

    expect(
      screen.getByRole("navigation", { name: "Primary" }),
    ).toBeInTheDocument();
  });

  it("opens the real palette from ⌘K and closes it from Escape", async () => {
    renderFrame();
    await screen.findByRole("heading", { name: "Chat", level: 1 });

    fireEvent.keyDown(window, { key: "k", metaKey: true });
    const dialog = await screen.findByRole("dialog", {
      name: "Command palette",
    });
    // A row from the real catalogue, not a placeholder.
    expect(
      within(dialog).getByRole("option", { name: /Work — all runs/ }),
    ).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("opens the palette from the rail's ⌘K button", async () => {
    renderFrame();
    await screen.findByRole("heading", { name: "Chat", level: 1 });

    fireEvent.click(screen.getByRole("button", { name: /Command/ }));
    expect(
      await screen.findByRole("dialog", { name: "Command palette" }),
    ).toBeInTheDocument();
  });

  it("binds the palette's shortcuts globally, with the palette shut (§5.6)", async () => {
    renderFrame();
    await screen.findByRole("heading", { name: "Chat", level: 1 });

    fireEvent.keyDown(window, { key: "2", metaKey: true });
    expect(useUiStore.getState().view).toBe("work");

    // Exactly once — the palette must not mount a second listener.
    expect(useUiStore.getState().dense).toBe(false);
    fireEvent.keyDown(window, { key: "d", metaKey: true, shiftKey: true });
    expect(useUiStore.getState().dense).toBe(true);
  });

  it("renders the toast slot from the store's single slot", async () => {
    renderFrame();
    await screen.findByRole("heading", { name: "Chat", level: 1 });

    expect(screen.queryByRole("status")).toBeNull();
    useUiStore.getState().showToast("Connector audit paused");
    expect(await screen.findByRole("status")).toHaveTextContent(
      "Connector audit paused",
    );
  });

  it("routes a published confirmation to the key ladder and the palette", async () => {
    const approve = vi.fn();
    const deny = vi.fn();
    renderFrame();
    await screen.findByRole("heading", { name: "Chat", level: 1 });

    useConfirmationStore.setState({
      pending: { toolName: "shell_execute", runId: "b41c8e02", approve, deny },
    });

    // The palette grows the Approve row the chat lane's block earns it.
    fireEvent.keyDown(window, { key: "k", metaKey: true });
    expect(
      await screen.findByRole("option", {
        name: /Approve pending shell_execute/,
      }),
    ).toBeInTheDocument();

    // Enter belongs to the palette's own list while it is open.
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(deny).not.toHaveBeenCalled();

    fireEvent.keyDown(window, { key: "Enter" });
    expect(approve).toHaveBeenCalledTimes(1);

    fireEvent.keyDown(window, { key: "Escape" });
    expect(deny).toHaveBeenCalledTimes(1);
  });
});
