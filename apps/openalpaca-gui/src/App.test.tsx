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
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { resetConnection } from "@/lib/connection";
import { createQueryClient } from "@/lib/query-client";
import { qk } from "@/lib/query-keys";
import { QueryProvider } from "@/lib/query-provider";
import { useConfirmationStore } from "@/stores/confirmation";
import { useProjectStore } from "@/stores/project";
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
      // A status body the Connection panel can read. `[]` used to stand in
      // for it and only passed because nothing cached it before Settings
      // mounted; the Memory view (a project-root read) now does.
      if (url.includes("/v1/status")) {
        return json({
          home_root: "/Users/dev/.openalpaca",
          state_dir: "/Users/dev/.openalpaca/state",
          db_path: "/Users/dev/.openalpaca/state/openalpaca.db",
          project_root: null,
          started_at: "2026-09-24T09:00:00Z",
          uptime_secs: 60,
          schema_version: 1,
          log_path: null,
          upload_bytes: 0,
          produced_bytes: 0,
          retention: {
            log_max_session_bytes: 268435456,
            log_max_total_bytes: 2147483648,
            log_retention_days: 0,
          },
          sessions: { last_sweep: null, dropped_records: 0 },
          routing: { resume_enabled: false },
        });
      }
      if (url.includes("/v1/sessions")) {
        return json({ sessions: [], total: 0 });
      }
      if (url.includes("/v1/memories/recently-learned")) {
        return json({ items: [] });
      }
      if (url.includes("/v1/memories")) {
        return json({ items: [], next_cursor: null });
      }
      // tasks, models, connectors, plugins, skills, templates, usage — all lists
      return json([]);
    }),
  );
}

const initialUi = useUiStore.getState();

function renderFrame(
  client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  }),
) {
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

    fireEvent.click(screen.getByRole("button", { name: "Memory" }));
    expect(await screen.findByRole("region", { name: "Memory" })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(
      await screen.findByRole("region", { name: "Settings" }),
    ).toBeTruthy();

    expect(
      screen.getByRole("navigation", { name: "Primary" }),
    ).toBeInTheDocument();
  });

  /**
   * Memory redesign G30 (owner decision): Memory is the fifth top-level view,
   * after Library and before Settings — in the rail's list with the other
   * three, while Settings keeps its place at the foot.
   */
  it("puts Memory in the rail after Library, with Settings at the foot", async () => {
    renderFrame();
    await screen.findByRole("heading", { name: "Chat", level: 1 });

    const rail = screen.getByRole("navigation", { name: "Primary" });
    const views = within(rail)
      .getAllByRole("button")
      .map((button) => button.textContent?.replace(/\d+$/, "").trim())
      .filter((name) =>
        ["Chat", "Work", "Library", "Memory", "Settings"].includes(name ?? ""),
      );
    expect(views).toEqual(["Chat", "Work", "Library", "Memory", "Settings"]);

    fireEvent.click(within(rail).getByRole("button", { name: "Memory" }));
    expect(useUiStore.getState().view).toBe("memory");
    expect(
      within(rail).getByRole("button", { name: "Memory" }),
    ).toHaveAttribute("aria-current", "page");
  });

  it("reaches Memory from ⌘4, like the other views' shortcuts", async () => {
    renderFrame();
    await screen.findByRole("heading", { name: "Chat", level: 1 });

    fireEvent.keyDown(window, { key: "4", metaKey: true });
    expect(useUiStore.getState().view).toBe("memory");
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
      pending: {
        toolName: "shell_execute",
        hidden: 0,
        count: 1,
        runId: "b41c8e02",
        approve,
        deny,
      },
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

/**
 * R22 (and R14): a project re-pointed from somewhere else — `openalpaca
 * store rebase`, another window — leaves this window holding a root nothing
 * records any more, and its turns would be filed under no project. The id is
 * what survives the move (ST-13), so the window looks it up and follows,
 * whatever view is open.
 */
describe("The window's project (R22)", () => {
  const OLD_ROOT = "/Users/dev/AlpacaLink";
  const NEW_ROOT = "/Users/dev/work/AlpacaLink";
  const PROJECT_ID = "6f1c2a3b-4d5e-4f60-8a7b-9c0d1e2f3a4b";

  /** The daemon after the re-point: the old root resolves to nothing. */
  function rePointedElsewhere(listed: unknown[]): void {
    const base = globalThis.fetch;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: unknown, init?: RequestInit) => {
        const url = String(input);
        if (url.includes("/v1/workspaces") && !url.includes("path=")) {
          return json(listed);
        }
        const answer = await base(input as RequestInfo, init);
        if (!url.includes("/v1/status")) return answer;
        const held = new Headers(init?.headers).get("x-workspace-path");
        const body = (await answer.json()) as Record<string, unknown>;
        return json({
          ...body,
          project_root: held === NEW_ROOT ? NEW_ROOT : null,
        });
      }),
    );
  }

  afterEach(() => {
    useProjectStore.setState({ path: null, id: null });
  });

  it("follows its project to the root it was re-pointed to, and says so once", async () => {
    rePointedElsewhere([
      {
        id: PROJECT_ID,
        root: NEW_ROOT,
        name: "-Users-dev-work-AlpacaLink",
        missing: false,
      },
    ]);
    useProjectStore.setState({ path: OLD_ROOT, id: PROJECT_ID });
    renderFrame();

    await waitFor(() => expect(useProjectStore.getState().path).toBe(NEW_ROOT));
    expect(useProjectStore.getState().id).toBe(PROJECT_ID);
    expect(useUiStore.getState().toast).toBe(
      `This window's project was re-pointed to ${NEW_ROOT}: its turns go there now.`,
    );
  });

  it("follows even when a list read before the re-point is still cached", async () => {
    // The Projects card read the list moments before `openalpaca store
    // rebase` ran elsewhere: with the app's own 30 s staleTime that list is
    // still fresh, and it records the project at the old root. Only a list
    // read since the root stopped resolving may be believed (residual X8).
    rePointedElsewhere([
      {
        id: PROJECT_ID,
        root: NEW_ROOT,
        name: "-Users-dev-work-AlpacaLink",
        missing: false,
      },
    ]);
    useProjectStore.setState({ path: OLD_ROOT, id: PROJECT_ID });
    const client = createQueryClient();
    client.setQueryData(qk.workspaces.list(), [
      {
        id: PROJECT_ID,
        root: OLD_ROOT,
        name: "-Users-dev-AlpacaLink",
        missing: false,
      },
    ]);
    renderFrame(client);

    await waitFor(() => expect(useProjectStore.getState().path).toBe(NEW_ROOT));
    expect(useProjectStore.getState().id).toBe(PROJECT_ID);
    expect(useUiStore.getState().toast).toBe(
      `This window's project was re-pointed to ${NEW_ROOT}: its turns go there now.`,
    );
  });

  it("leaves a root an earlier build stored alone where it is: it has no id to follow", async () => {
    rePointedElsewhere([
      {
        id: PROJECT_ID,
        root: NEW_ROOT,
        name: "-Users-dev-work-AlpacaLink",
        missing: false,
      },
    ]);
    useProjectStore.setState({ path: OLD_ROOT, id: null });
    renderFrame();
    await screen.findByRole("heading", { name: "Chat", level: 1 });
    await waitFor(() =>
      expect(
        vi
          .mocked(globalThis.fetch)
          .mock.calls.some(([input]) => String(input).includes("/v1/status")),
      ).toBe(true),
    );

    expect(useProjectStore.getState().path).toBe(OLD_ROOT);
    expect(useUiStore.getState().toast).toBeNull();
    expect(
      vi
        .mocked(globalThis.fetch)
        .mock.calls.some(
          ([input]) =>
            String(input).includes("/v1/workspaces") &&
            !String(input).includes("path="),
        ),
    ).toBe(false);
  });

  it("keeps a root whose project is listed nowhere, and moves nothing", async () => {
    rePointedElsewhere([]);
    useProjectStore.setState({ path: OLD_ROOT, id: PROJECT_ID });
    renderFrame();
    await screen.findByRole("heading", { name: "Chat", level: 1 });

    await waitFor(() =>
      expect(
        vi
          .mocked(globalThis.fetch)
          .mock.calls.some(
            ([input]) =>
              String(input).includes("/v1/workspaces") &&
              !String(input).includes("path="),
          ),
      ).toBe(true),
    );
    expect(useProjectStore.getState().path).toBe(OLD_ROOT);
    expect(useUiStore.getState().toast).toBeNull();
  });
});
