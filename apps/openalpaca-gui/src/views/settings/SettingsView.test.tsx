import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useUiStore } from "@/stores/ui";

import SettingsView from "./SettingsView";

/**
 * Every server-backed hook is mocked so the sections render against a known
 * payload; the *unbacked* hooks (`useDaemonStatusDetail`, `useToolCatalog`,
 * `usePluginInstall`) stay real, because their unavailable branches are exactly
 * what these tests are checking.
 */
const query = (data: unknown) => ({ data, isPending: false, error: null });
const mutation = () => ({ mutate: vi.fn(), isPending: false });

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
}));

vi.mock("@/hooks/useTasks", () => ({
  useTasks: () => query([]),
}));

vi.mock("@/hooks/useUsage", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/useUsage")>()),
  useTodaySpend: () =>
    query({
      date: "2026-08-31",
      costUsd: 0.0184,
      tokensIn: 30_000,
      tokensOut: 11_000,
      requests: 12,
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
}));

vi.mock("@/hooks/useOrchestrator", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/useOrchestrator")>()),
  useOrchestratorConfig: () =>
    query({
      model: "claude-sonnet-4-6",
      fallback_models: [],
      active_agents: 0,
      active_tasks: 0,
      daily_cost_usd: 0,
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
      },
    ]),
  useUnwiredConnectors: () => [],
  useConnectorAction: () => mutation(),
}));

vi.mock("@/hooks/useSkills", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/useSkills")>()),
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
}));

vi.mock("@/hooks/usePlugins", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/usePlugins")>()),
  usePlugins: () =>
    query([
      {
        name: "notion",
        version: "0.2.0",
        status: "running",
        tools: ["notion_search"],
        connector: null,
        provider: null,
        models: [],
      },
      {
        name: "risky",
        version: "0.1.0",
        status: "waiting-approval",
        tools: [],
        connector: null,
        provider: null,
        models: [],
      },
    ]),
  usePluginAction: () => mutation(),
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
      },
    ]),
  useAgentInstances: () => query([]),
}));

vi.mock("@/hooks/useConversations", () => ({
  useConversations: () =>
    query({
      conversations: [
        {
          id: "c1",
          lane_key: "local:gui",
          source: "gui",
          title: "Connector audit",
          message_count: 142,
          last_message_at: "2026-08-29T10:00:00Z",
          created_at: "2026-08-01T10:00:00Z",
          updated_at: "2026-08-29T10:00:00Z",
          summary: "",
          summary_version: 1,
          last_summarized_message_id: 100,
        },
      ],
    }),
}));

vi.mock("@/hooks/useEventHistory", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/useEventHistory")>()),
  useEventHistory: () =>
    query([
      {
        id: 1,
        timestamp: "2026-08-31T14:22:41Z",
        agent_id: "review_agent",
        event_type: "tool_invoked",
      },
    ]),
}));

beforeEach(() => {
  useUiStore.setState({ settingsSectionId: "connection", toast: null });
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
    expect(nav).toHaveTextContent("Plugins2");
    // Connection and Event log have no count in the design.
    expect(
      screen.getByRole("button", { name: "Connection" }),
    ).toHaveTextContent(/^Connection$/);
  });

  it("shows the daemon's identity and names what /v1/health cannot serve", () => {
    render(<SettingsView />);
    expect(screen.getByText("Daemon connected")).toBeInTheDocument();
    expect(screen.getByText("127.0.0.1:51823")).toBeInTheDocument();
    expect(
      screen.getByText(/Daemon status detail not yet available/),
    ).toHaveTextContent("GET /v1/status");
    expect(
      screen.getByRole("button", { name: "Copy log path" }),
    ).toBeDisabled();
  });

  it("offers the provider's models and disables the switch it cannot flip", async () => {
    render(<SettingsView />);
    await open("Models & keys");
    expect(
      screen.getByRole("button", { name: "✓ claude-sonnet-4-6" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("switch", { name: "Enable anthropic" }),
    ).toBeDisabled();
    expect(
      screen.getByText(/Provider enable\/disable not yet available/),
    ).toBeInTheDocument();
  });

  it("uses accurate plugin copy and the daemon's own status words", async () => {
    render(<SettingsView />);
    await open("Plugins");
    expect(screen.queryByText(/WASM/i)).toBeNull();
    expect(screen.getByText(/JSON-RPC/)).toBeInTheDocument();
    // `running` gets a switch; `waiting-approval` gets the approval gate.
    expect(screen.getByRole("switch", { name: "Enable notion" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Approve" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Deny" })).toBeInTheDocument();
  });

  it("shows the skill health the daemon serves and names the missing catalog", async () => {
    render(<SettingsView />);
    await open("Skills");
    expect(screen.getByText("connector_audit")).toBeInTheDocument();
    expect(
      screen.getByText(/Tool and skill catalog not yet available/),
    ).toHaveTextContent("GET /v1/tools");
  });

  it("renders conversations with their compaction state", async () => {
    render(<SettingsView />);
    await open("Conversations");
    expect(screen.getByText("142 messages · 29 Aug")).toBeInTheDocument();
    expect(screen.getByText("compacted")).toBeInTheDocument();
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
});
