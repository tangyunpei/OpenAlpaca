import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { DaemonStatus } from "@/lib/api/types";
import { useProjectStore } from "@/stores/project";
import { useUiStore } from "@/stores/ui";

import SettingsView from "./SettingsView";
import { extensionRow } from "./extension-fixture";

/**
 * Every server-backed hook is mocked so the sections render against a known
 * payload; the gap adapters that remain (the extension install flow) stay
 * real, because their unavailable branches are exactly what these tests are
 * checking.
 */
const query = (data: unknown) => ({ data, isPending: false, error: null });
const mutation = () => ({ mutate: vi.fn(), isPending: false });

/** `GET /v1/status`, mutable so one test can take the log path away. */
const DAEMON_STATUS: DaemonStatus = {
  home_root: "/Users/dev/.openalpaca",
  state_dir: "/Users/dev/.openalpaca/state",
  db_path: "/Users/dev/.openalpaca/state/openalpaca.db",
  project_root: null,
  started_at: "2026-08-27T12:00:00Z",
  uptime_secs: 4 * 86_400 + 2 * 3_600 + 61,
  schema_version: 39,
  log_path: "/Users/dev/.openalpaca/state/logs/daemon.log",
  upload_bytes: 2_621_440,
  produced_bytes: 1_048_576,
  retention: {
    log_max_session_bytes: 256 * 1024 * 1024,
    log_max_total_bytes: 2 * 1024 * 1024 * 1024,
    log_retention_days: 0,
  },
  sessions: { last_sweep: null, dropped_records: 0 },
  routing: { resume_enabled: false },
};
let daemonStatus: DaemonStatus = { ...DAEMON_STATUS };

/**
 * The moved-project offer (§4.8, P-12). `workspace` is mutable so one test can
 * make the chosen project a moved one; the mutation is a spy, because the whole
 * point of the card is that it never re-bases without being told to.
 */
const workspace = vi.hoisted(() => ({
  moved: null as {
    from: string;
    to: string;
    rows: {
      artifacts: number;
      sessions: number;
      tasks: number;
      memories: number;
    };
    activeTasks: number;
  } | null,
  pending: false,
  error: null as Error | null,
  rebase: vi.fn(),
  rebaseError: null as Error | null,
}));

vi.mock("@/hooks/useWorkspaces", () => ({
  useMovedProject: () => ({
    moved: workspace.moved,
    pending: workspace.pending,
    error: workspace.error,
  }),
  useRebaseWorkspace: () => ({
    mutate: workspace.rebase,
    isPending: false,
    error: workspace.rebaseError,
  }),
}));

vi.mock("@/hooks/useConnection", () => ({
  useConnectionStatus: () => ({
    info: null,
    health: undefined,
    socket: "connected",
    connected: true,
    instanceChip: "7f3a",
    endpoint: "127.0.0.1:51823",
    reconnect: vi.fn(),
  }),
  useDaemonStatus: () => query(daemonStatus),
}));

vi.mock("@/hooks/useTasks", () => ({
  useTasks: () => query([]),
}));

vi.mock("@/hooks/useUsage", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/useUsage")>()),
  useUsageSummary: () =>
    query({
      date: "2026-08-31",
      total_usd: 0.0184,
      by_provider: [
        { provider: "anthropic", usd: 0.0184, calls: 12, tokens: 41_000 },
      ],
      caps: { workflow_max_cost_usd: 5, agent_max_cost_usd: 1 },
    }),
}));

vi.mock("@/hooks/useSettings", () => ({
  useLlmSettings: () =>
    query({
      orchestrator: { model: "claude-sonnet-4-6", fallback_models: [] },
      providers: {
        anthropic: {
          enabled: true,
          key_selection_strategy: "round_robin",
          keys: [{ id: "k1" }],
        },
      },
    }),
  useModels: () =>
    query([
      {
        id: "claude-sonnet-4-6",
        provider: "anthropic",
        context_window: 200_000,
        input_price_per_million: 3,
        output_price_per_million: 15,
      },
    ]),
  useProviderUsage: () =>
    query([
      {
        provider: "anthropic",
        total_cost_usd: 1.5,
        total_tokens: 41_000,
        total_requests: 12,
        health: "healthy",
        external_usage: null,
      },
    ]),
  useSetProviderEnabled: () => mutation(),
}));

vi.mock("@/hooks/useOrchestrator", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/useOrchestrator")>()),
  useOrchestratorConfig: () =>
    query({
      model: "claude-sonnet-4-6",
      fallback_models: [],
      active_agents: 0,
      active_tasks: 0,
      daily_cost_usd: 0.0184,
    }),
  useUpdateOrchestratorConfig: () => mutation(),
}));

vi.mock("@/hooks/useConnectors", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/useConnectors")>()),
  useConnectors: () =>
    query([
      {
        id: "telegram",
        name: "Telegram",
        status: "connected",
        configured: true,
        source: "telegram",
        registered: true,
        messages_7d: 184,
      },
    ]),
  useUnwiredConnectors: () => [],
  useConnectorAction: () => mutation(),
}));

vi.mock("@/hooks/useSkills", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/useSkills")>()),
  useTools: () =>
    query([
      {
        name: "shell_execute",
        description: "Runs a shell command.",
        source: "builtin",
        origin: null,
        provides_capabilities: ["shell_execute"],
        requires_confirmation: true,
        invocations_today: 4,
        version: "1.0.0",
        author: "builtin",
      },
      {
        name: "github__create_issue",
        description: "Opens an issue.",
        source: "mcp",
        origin: { kind: "mcp", id: "github", enabled: true, state: "enabled" },
        provides_capabilities: ["github__create_issue"],
        requires_confirmation: false,
        invocations_today: 12,
        version: "1.4.0",
        author: "mcp:github",
      },
    ]),
  useSkillHealth: () =>
    query([
      {
        skill_id: "connector_audit",
        total_invocations: 9,
        clean_success_rate: 0.9,
        clean_success_rate_7d: 0.9,
        repair_rate: 0.1,
        repair_effectiveness: 1,
        degraded_rate: 0,
        avg_duration_ms: 1400,
        avg_cost_usd: 0.01,
        avg_rounds: 3,
        last_invoked_at: null,
        user_satisfaction_rate: null,
        feedback_count: 0,
        feedback_coverage: 0,
      },
    ]),
  useSkillCatalog: () =>
    query([
      {
        id: "connector_audit",
        name: "Connector Audit",
        description: "Check every connector's credentials",
        source: "file",
        origin: null,
        requires_capabilities: [],
        triggers: { slash: "audit", keywords: [] },
        schedule: null,
        invocations_today: 2,
        version: "0.2.0",
        author: "file:project",
      },
    ]),
}));

vi.mock("@/hooks/useExtensions", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/useExtensions")>()),
  useExtensions: () =>
    query([
      extensionRow({
        kind: "plugin",
        id: "notion",
        version: "0.2.0",
        state: "enabled",
        enabled: true,
        consent: "approved",
        tools: ["notion::search"],
      }),
      extensionRow({
        kind: "plugin",
        id: "risky",
        version: "0.1.0",
        state: "unapproved",
        reason: "never_seen",
        enabled: true,
        consent: "pending",
        declared: {
          capabilities: ["fs_write"],
          virtual_capabilities: [],
          types: { tool: true },
        },
      }),
    ]),
  useExtensionVerb: () => mutation(),
  useRemoveExtension: () => mutation(),
  useSetExtensionConfig: () => mutation(),
  useInstallPlugin: () => mutation(),
  useValidatePlugin: () => mutation(),
  useUpdatePlugin: () => mutation(),
  useAddMcpServer: () => mutation(),
  useUninstallExtension: () => mutation(),
}));

vi.mock("@/hooks/useAgents", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/useAgents")>()),
  useAgentTemplates: () =>
    query([
      {
        id: "review_agent",
        name: "Review agent",
        description: "Reviews work.",
        singleton: false,
        capabilities: [],
        denied_capabilities: [],
        temperature: 0.2,
        verbosity: "normal",
        fallback_models: [],
        require_confirmation_for: [],
        persona: "",
        body: "",
        run_count: 3,
        last_run_at: "2026-09-04T09:15:00.000Z",
      },
    ]),
  useAgentInstances: () => query([]),
}));

vi.mock("@/hooks/useSessions", () => ({
  useSessions: () =>
    query({
      sessions: [
        {
          id: "c1",
          lane_key: "local:gui",
          source: "gui",
          title: "Connector audit",
          workspace_id: null,
          status: "archived",
          message_count: 142,
          last_message_at: "2026-08-29T10:00:00Z",
          created_at: "2026-08-01T10:00:00Z",
          updated_at: "2026-08-29T10:00:00Z",
          ended_at: "2026-08-29T10:05:00Z",
          active_task_count: 0,
          interrupted_task_count: 0,
        },
      ],
      total: 1,
    }),
}));

vi.mock("@/hooks/useEventHistory", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/useEventHistory")>()),
  useEventHistory: () =>
    query({
      events: [
        {
          id: 1,
          timestamp: "2026-08-31T14:22:41Z",
          agent_id: "review_agent",
          task_id: null,
          event_type: "tool_invoked",
        },
      ],
      next_before: null,
    }),
}));

beforeEach(() => {
  useUiStore.setState({ settingsSectionId: "connection", toast: null });
  useProjectStore.setState({ path: null });
  daemonStatus = { ...DAEMON_STATUS };
  workspace.moved = null;
  workspace.pending = false;
  workspace.error = null;
  workspace.rebaseError = null;
  workspace.rebase.mockReset();
  localStorage.clear();
});

/** Nav items carry a trailing count, so match on the label prefix. */
async function open(label: string) {
  await userEvent.setup().click(
    screen.getByRole("button", {
      name: (accessibleName: string) => accessibleName.startsWith(label),
    }),
  );
}

describe("SettingsView (§2.5, §5.4)", () => {
  it("lists the eight sections with real counts and no invented zeroes", () => {
    render(<SettingsView />);
    const nav = screen.getByRole("navigation", { name: "Settings sections" });
    expect(nav).toHaveTextContent("Connection");
    expect(nav).toHaveTextContent("Models & keys1");
    expect(nav).toHaveTextContent("Extensions2");
    expect(nav).toHaveTextContent("Tools2");
    // Connection and Event log have no count in the design.
    expect(
      screen.getByRole("button", { name: "Connection" }),
    ).toHaveTextContent(/^Connection$/);
  });

  it("shows the daemon's identity, uptime, schema and store sizes from /v1/status", () => {
    render(<SettingsView />);
    expect(screen.getByText("Daemon connected")).toBeInTheDocument();
    expect(screen.getByText("127.0.0.1:51823")).toBeInTheDocument();
    // GAP-14's three: uptime, `Schema vNN`, and a log path worth copying.
    expect(screen.getByText("uptime 4d 02h")).toBeInTheDocument();
    expect(screen.getByText("v39")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy log path" })).toBeEnabled();
    expect(
      screen.queryByText(/Daemon status detail not yet available/),
    ).toBeNull();
    // §4.8's two numbers, never one.
    expect(screen.getByText("2.5 MB")).toBeInTheDocument();
    expect(screen.getByText("1.0 MB")).toBeInTheDocument();
  });

  it("copies the daemon log path the route reported", async () => {
    const user = userEvent.setup();
    render(<SettingsView />);

    await user.click(screen.getByRole("button", { name: "Copy log path" }));

    expect(await window.navigator.clipboard.readText()).toBe(
      "/Users/dev/.openalpaca/state/logs/daemon.log",
    );
    // The toast renders in the app shell, not this subtree; the store is where
    // the confirmation is observable from here.
    expect(useUiStore.getState().toast).toMatch(/Log path copied/);
  });

  /// A daemon the CLI did not start writes no `daemon.log`, and the route says
  /// `null`. The button goes inert with the reason — never a path to nothing.
  it("disables Copy log path when the daemon reports none", () => {
    daemonStatus = { ...DAEMON_STATUS, log_path: null };
    render(<SettingsView />);

    expect(
      screen.getByRole("button", { name: "Copy log path" }),
    ).toBeDisabled();
    expect(screen.getByText(/no daemon\.log/i)).toBeInTheDocument();
  });

  it("offers the provider's models and a switch that reaches the daemon", async () => {
    render(<SettingsView />);
    await open("Models & keys");
    expect(
      screen.getByRole("button", { name: "✓ claude-sonnet-4-6" }),
    ).toBeInTheDocument();
    // GAP-15 closed: the switch is live, and the note that said it was not is
    // gone with it.
    expect(
      screen.getByRole("switch", { name: "Enable anthropic" }),
    ).toBeEnabled();
    expect(
      screen.queryByText(/Provider enable\/disable not yet available/),
    ).toBeNull();
  });

  it("uses accurate extension copy and gates consent instead of drawing a switch", async () => {
    render(<SettingsView />);
    await open("Extensions");
    expect(screen.queryByText(/WASM/i)).toBeNull();
    expect(screen.getByText(/JSON-RPC/)).toBeInTheDocument();
    // `enabled` gets a switch; `unapproved` gets the approval gate, because a
    // switch would misrepresent it (§9.2).
    expect(screen.getByRole("switch", { name: "Enable notion" })).toBeEnabled();
    expect(screen.queryByRole("switch", { name: "Enable risky" })).toBeNull();
    expect(screen.getByRole("button", { name: "Approve" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Deny" })).toBeInTheDocument();
  });

  it("shows the tool catalog and names the skill health rows from the skill catalog", async () => {
    render(<SettingsView />);
    await open("Tools");
    expect(screen.getByText("shell_execute")).toBeInTheDocument();
    expect(screen.getByText("Connector Audit")).toBeInTheDocument();
    expect(screen.getByText(/connector_audit ·/)).toBeInTheDocument();
    expect(screen.queryByText(/Skill catalog not yet available/)).toBeNull();
  });

  it("renders stored conversations with their lifecycle state", async () => {
    render(<SettingsView />);
    await open("Conversations");
    expect(screen.getByText("142 messages · 29 Aug")).toBeInTheDocument();
    expect(screen.getByText("archived")).toBeInTheDocument();
  });

  it("categorises real event types onto the design's log tags", async () => {
    render(<SettingsView />);
    await open("Event log");
    expect(screen.getByText("tool")).toBeInTheDocument();
    expect(screen.getByText("tool_invoked · review_agent")).toBeInTheDocument();
  });

  it("toasts honestly instead of pretending an add flow exists", async () => {
    render(<SettingsView />);
    await open("Connectors");
    await open("Connect service");
    expect(useUiStore.getState().toast).toMatch(/no daemon route yet/);
  });

  /** Plan §4.7 item 2 — where the owner chooses the project the GUI sends. */
  it("takes a project path and keeps it", async () => {
    const user = userEvent.setup();
    render(<SettingsView />);

    await user.type(
      screen.getByLabelText("Project path"),
      "/Users/dev/openalpaca",
    );
    await user.click(screen.getByRole("button", { name: "Use" }));

    expect(useProjectStore.getState().path).toBe("/Users/dev/openalpaca");
    expect(useUiStore.getState().toast).toMatch(/Project set/);
  });

  it("refuses a relative path instead of sending one the daemon misreads", async () => {
    const user = userEvent.setup();
    render(<SettingsView />);

    await user.type(screen.getByLabelText("Project path"), "code/app");

    expect(screen.getByRole("button", { name: "Use" })).toBeDisabled();
    expect(screen.getByText(/An absolute path, please/)).toBeInTheDocument();
    expect(useProjectStore.getState().path).toBeNull();
  });

  it("clears back to no project", async () => {
    const user = userEvent.setup();
    useProjectStore.setState({ path: "/Users/dev/openalpaca" });
    render(<SettingsView />);

    await user.click(screen.getByRole("button", { name: "Clear" }));

    expect(useProjectStore.getState().path).toBeNull();
    expect(screen.getByText(/files go to the home store/)).toBeInTheDocument();
  });

  // ── The moved project (§4.8, P-12) ─────────────────────────────────────

  it("says nothing about a project that has not moved", () => {
    useProjectStore.setState({ path: "/Users/dev/openalpaca" });
    render(<SettingsView />);

    expect(
      screen.queryByRole("button", { name: "Re-base to this path" }),
    ).not.toBeInTheDocument();
  });

  it("offers a re-base for a moved project and never takes it alone", async () => {
    const user = userEvent.setup();
    useProjectStore.setState({ path: "/Users/dev/moved" });
    workspace.moved = {
      from: "/Users/dev/openalpaca",
      to: "/Users/dev/moved",
      rows: { artifacts: 12, sessions: 2, tasks: 3, memories: 7 },
      activeTasks: 0,
    };
    render(<SettingsView />);

    // The offer names the old root and every member, so the owner can see
    // exactly what would move.
    expect(screen.getByText("/Users/dev/openalpaca")).toBeInTheDocument();
    expect(
      screen.getByText(/12 artifacts, 2 conversations, 3 runs and 7 memories/),
    ).toBeInTheDocument();

    // Offering is not doing: the first click only asks.
    await user.click(
      screen.getByRole("button", { name: "Re-base to this path" }),
    );
    expect(workspace.rebase).not.toHaveBeenCalled();
    expect(
      screen.getByText("Re-base all four onto this path?"),
    ).toBeInTheDocument();

    // And a second thought takes it away without a request.
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(workspace.rebase).not.toHaveBeenCalled();

    await user.click(
      screen.getByRole("button", { name: "Re-base to this path" }),
    );
    await user.click(screen.getByRole("button", { name: "Re-base" }));
    expect(workspace.rebase).toHaveBeenCalledTimes(1);
    expect(workspace.rebase.mock.calls[0]?.[0]).toEqual({
      from: "/Users/dev/openalpaca",
      to: "/Users/dev/moved",
    });
  });

  it("says a run is still in flight rather than offering a re-base that would be refused", () => {
    useProjectStore.setState({ path: "/Users/dev/moved" });
    workspace.moved = {
      from: "/Users/dev/openalpaca",
      to: "/Users/dev/moved",
      rows: { artifacts: 1, sessions: 0, tasks: 1, memories: 0 },
      activeTasks: 2,
    };
    render(<SettingsView />);

    expect(
      screen.getByText(/2 run\(s\) there are still in flight/),
    ).toBeInTheDocument();
  });

  it("shows the daemon's own refusal instead of a generic failure", () => {
    useProjectStore.setState({ path: "/Users/dev/moved" });
    workspace.moved = {
      from: "/Users/dev/openalpaca",
      to: "/Users/dev/moved",
      rows: { artifacts: 1, sessions: 0, tasks: 0, memories: 0 },
      activeTasks: 0,
    };
    workspace.rebaseError = new Error(
      "/Users/dev/moved already has a store recorded against it",
    );
    render(<SettingsView />);

    expect(
      screen.getByText(/already has a store recorded against it/),
    ).toBeInTheDocument();
  });

  it("shows a destination refusal in the picker, naming the root the daemon named", () => {
    useProjectStore.setState({ path: "/Users/dev/mono/sub/proj" });
    workspace.moved = {
      from: "/Users/dev/openalpaca",
      to: "/Users/dev/mono/sub/proj",
      rows: { artifacts: 1, sessions: 0, tasks: 0, memories: 0 },
      activeTasks: 0,
    };
    workspace.rebaseError = new Error(
      "/Users/dev/mono/sub/proj is inside the project rooted at /Users/dev/mono, " +
        "so re-basing onto it would move everything onto /Users/dev/mono instead.",
    );
    render(<SettingsView />);

    expect(
      screen.getByText(/is inside the project rooted at \/Users\/dev\/mono/),
    ).toBeInTheDocument();
  });

  it("says it is checking rather than showing a project as settled", () => {
    useProjectStore.setState({ path: "/Users/dev/openalpaca" });
    workspace.pending = true;
    render(<SettingsView />);

    expect(
      screen.getByText(/Checking whether this project has moved/),
    ).toBeInTheDocument();
  });

  it("says a failed lookup failed", () => {
    useProjectStore.setState({ path: "/Users/dev/openalpaca" });
    workspace.error = new Error("daemon unreachable");
    render(<SettingsView />);

    expect(
      screen.getByText(/Could not check whether this project has moved/),
    ).toBeInTheDocument();
  });
});
