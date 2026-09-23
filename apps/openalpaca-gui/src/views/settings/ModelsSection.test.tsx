import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import { vi } from "vitest";

import { ApiError } from "@/lib/http";
import { useUiStore } from "@/stores/ui";

import { ModelsSection } from "./ModelsSection";

/**
 * The provider switch (GAP-15). The mutation is a double that calls back, so
 * what these check is the section's own contract: which call it makes, what it
 * says when the daemon refuses, and that a refusal is *said* rather than left
 * as a moved switch.
 */
const state = vi.hoisted(() => ({
  providers: {} as Record<string, unknown>,
  calls: [] as Array<{ provider: string; enabled: boolean }>,
  fail: null as Error | null,
  /** What the daemon answers with; null means "loaded, no warning". */
  result: null as {
    id: string;
    enabled: boolean;
    loaded: boolean;
    discovered_models: number;
    discovery_error: string | null;
    warning: string | null;
  } | null,
  /** How many times `POST /v1/models/refresh` was asked for, and its answer. */
  refreshes: 0,
  refreshResult: [] as Array<Record<string, unknown>>,
  refreshFail: null as Error | null,
  /** `GET /v1/status`'s `llm` block (L3). */
  daemonLlm: null as {
    default_model: string;
    default_model_routable: boolean;
    effective_default_model: string | null;
  } | null,
  /** `GET /v1/models` — the catalogue the chips are drawn from. */
  models: [] as Array<Record<string, unknown>>,
  /** `GET /v1/settings/llm`'s `orchestrator` half, off the same file read. */
  llmOrchestrator: { model: "claude-haiku-4-5", fallback_models: [] } as {
    model: string;
    fallback_models: string[];
  },
  /**
   * `GET /v1/orchestrator/config` — `undefined` is the cold cache the model
   * picker can be clicked through, because it is the one read here that goes
   * to the DB under the single mutex.
   */
  orchestrator: undefined as
    { model: string; fallback_models: string[] } | undefined,
  /** Every `PUT /v1/orchestrator/config` body the section sent. */
  writes: [] as Array<{ model: string; fallback_models: string[] }>,
  /** Every `PUT /v1/settings/llm` body the key form sent. */
  upserts: [] as Array<{ provider: string; key: Record<string, unknown> }>,
  /** Every `PUT /v1/settings/llm/keys/priority` body. */
  priorities: [] as Array<{
    provider: string;
    key_id: string;
    priority: string;
  }>,
  /** Every `DELETE /v1/settings/llm/keys/{provider}/{key_id}`. */
  removals: [] as Array<{ provider: string; keyId: string }>,
}));

vi.mock("@/hooks/useSettings", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/useSettings")>()),
  useLlmSettings: () => ({
    data: {
      orchestrator: state.llmOrchestrator,
      providers: state.providers,
    },
    isPending: false,
    error: null,
  }),
  useModels: () => ({ data: state.models, isPending: false, error: null }),
  useProviderUsage: () => ({ data: [], isPending: false, error: null }),
  useSetProviderEnabled: () => ({
    isPending: false,
    mutate: (
      input: { provider: string; enabled: boolean },
      options?: {
        onSuccess?: (row: {
          id: string;
          enabled: boolean;
          loaded: boolean;
          discovered_models: number;
          discovery_error: string | null;
          warning: string | null;
        }) => void;
        onError?: (error: Error) => void;
      },
    ) => {
      state.calls.push(input);
      if (state.fail !== null) options?.onError?.(state.fail);
      else
        options?.onSuccess?.(
          state.result ?? {
            id: input.provider,
            enabled: input.enabled,
            loaded: input.enabled,
            discovered_models: input.enabled ? 1 : 0,
            discovery_error: null,
            warning: null,
          },
        );
    },
  }),
  useUpsertKey: () => ({
    isPending: false,
    mutate: (input: { provider: string; key: Record<string, unknown> }) => {
      state.upserts.push(input);
    },
  }),
  useValidateKey: () => ({ isPending: false, mutate: () => undefined }),
  useSetKeyPriority: () => ({
    isPending: false,
    mutate: (input: { provider: string; key_id: string; priority: string }) => {
      state.priorities.push(input);
    },
  }),
  useRemoveKey: () => ({
    isPending: false,
    mutate: (input: { provider: string; keyId: string }) => {
      state.removals.push(input);
    },
  }),
  useRefreshModels: () => ({
    isPending: false,
    mutate: (
      _input: undefined,
      options?: {
        onSuccess?: (rows: Array<Record<string, unknown>>) => void;
        onError?: (error: Error) => void;
      },
    ) => {
      state.refreshes += 1;
      if (state.refreshFail !== null) options?.onError?.(state.refreshFail);
      else options?.onSuccess?.(state.refreshResult);
    },
  }),
}));

vi.mock("@/hooks/useConnection", () => ({
  useDaemonStatus: () => ({
    data: { llm: state.daemonLlm },
    isPending: false,
    error: null,
  }),
}));

vi.mock("@/hooks/useUsage", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/useUsage")>()),
  useUsageSummary: () => ({
    data: {
      date: "2026-09-08",
      total_usd: 0.03,
      by_provider: [
        { provider: "anthropic", usd: 0.03, calls: 4, tokens: 41_000 },
      ],
      caps: { workflow_max_cost_usd: 5, agent_max_cost_usd: 1 },
    },
    isPending: false,
    error: null,
  }),
}));

vi.mock("@/hooks/useOrchestrator", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/useOrchestrator")>()),
  useOrchestratorConfig: () => ({
    data: state.orchestrator,
    isPending: false,
    error: null,
  }),
  useUpdateOrchestratorConfig: () => ({
    isPending: false,
    mutate: (input: { model: string; fallback_models: string[] }) => {
      state.writes.push(input);
    },
  }),
}));

const provider = (enabled: boolean, requiresKey = true) => ({
  enabled,
  key_selection_strategy: "round_robin",
  keys: [],
  requires_key: requiresKey,
});

beforeEach(() => {
  state.providers = { anthropic: provider(true), ollama: provider(false) };
  state.calls = [];
  state.fail = null;
  state.result = null;
  state.refreshes = 0;
  state.refreshResult = [];
  state.refreshFail = null;
  state.daemonLlm = null;
  state.models = [];
  state.llmOrchestrator = {
    model: "claude-haiku-4-5",
    fallback_models: ["claude-sonnet-4-6"],
  };
  state.orchestrator = {
    model: "claude-haiku-4-5",
    fallback_models: ["claude-sonnet-4-6"],
  };
  state.writes = [];
  state.upserts = [];
  state.priorities = [];
  state.removals = [];
  useUiStore.setState({ toast: null });
});

describe("the provider switch", () => {
  it("is live, and asks for the opposite of what the row shows", async () => {
    render(<ModelsSection />);

    const ollama = screen.getByRole("switch", { name: "Enable ollama" });
    expect(ollama).toBeEnabled();
    expect(ollama).toHaveAttribute("aria-checked", "false");

    await userEvent.click(ollama);

    expect(state.calls).toEqual([{ provider: "ollama", enabled: true }]);
    expect(useUiStore.getState().toast).toBe("ollama on — found 1 model");
  });

  it("turns a live provider off", async () => {
    render(<ModelsSection />);

    await userEvent.click(
      screen.getByRole("switch", { name: "Enable anthropic" }),
    );

    expect(state.calls).toEqual([{ provider: "anthropic", enabled: false }]);
    expect(useUiStore.getState().toast).toBe("anthropic off");
  });

  it("says what the 409 means rather than repeating the daemon's sentence", async () => {
    state.fail = new ApiError(
      "'anthropic' serves the default model 'claude-haiku-4-5'",
      409,
      "PROVIDER_IS_DEFAULT",
    );
    render(<ModelsSection />);

    await userEvent.click(
      screen.getByRole("switch", { name: "Enable anthropic" }),
    );

    expect(useUiStore.getState().toast).toMatch(
      /serves the model you chat with — pick a different model first/,
    );
  });

  it("says inline when the bit was written but the provider did not load", async () => {
    // R60: `200 {enabled: true, loaded: false}` — the write happened, the
    // router has nothing. The switch stays on because the file says so, and
    // the row has to say the rest out loud rather than leave it in the log.
    state.result = {
      id: "anthropic",
      enabled: true,
      loaded: false,
      discovered_models: 0,
      discovery_error: null,
      warning: "No keys for Anthropic, cannot register provider",
    };
    state.providers = { anthropic: provider(false) };
    render(<ModelsSection />);

    await userEvent.click(
      screen.getByRole("switch", { name: "Enable anthropic" }),
    );

    expect(screen.getByText(/on, but not loaded/i).textContent).toMatch(
      /No keys for Anthropic/,
    );
  });

  it("carries no such note when the provider did load", async () => {
    render(<ModelsSection />);

    await userEvent.click(
      screen.getByRole("switch", { name: "Enable ollama" }),
    );

    expect(screen.queryByText(/on, but not loaded/i)).toBeNull();
  });

  it("no longer carries the gap note that said the switch was dead", () => {
    render(<ModelsSection />);

    expect(
      screen.queryByText(/Provider enable\/disable not yet available/),
    ).toBeNull();
  });
});

/**
 * Picking a model writes the whole `[orchestrator]` pair, so a `fallback_models`
 * this section did not have is a `fallback_models` the daemon erases
 * (`[]` → `None`). The chips come from `GET /v1/models`; the chain from
 * `GET /v1/orchestrator/config`, the one read here that goes to the DB under
 * the single mutex — so the row can be clickable while that one is still cold.
 */
describe("picking a chat model", () => {
  const catalogue = [
    {
      id: "claude-haiku-4-5",
      provider: "anthropic",
      context_window: 200000,
      input_price_per_million: 1,
      output_price_per_million: 5,
    },
    {
      id: "claude-sonnet-4-6",
      provider: "anthropic",
      context_window: 200000,
      input_price_per_million: 3,
      output_price_per_million: 15,
    },
  ];

  it("keeps the configured chain when the orchestrator query is cold", async () => {
    state.models = catalogue;
    state.orchestrator = undefined;
    render(<ModelsSection />);

    await userEvent.click(
      screen.getByRole("button", { name: "claude-sonnet-4-6" }),
    );

    expect(state.writes).toEqual([
      {
        model: "claude-sonnet-4-6",
        fallback_models: ["claude-sonnet-4-6"],
      },
    ]);
  });

  it("prefers the orchestrator's own chain when it has loaded", async () => {
    state.models = catalogue;
    state.orchestrator = {
      model: "claude-haiku-4-5",
      fallback_models: ["a", "b"],
    };
    render(<ModelsSection />);

    await userEvent.click(
      screen.getByRole("button", { name: "claude-sonnet-4-6" }),
    );

    expect(state.writes[0]?.fallback_models).toEqual(["a", "b"]);
  });
});

/**
 * The daemon's `warning` rides on the toggle's response and on nothing else, so
 * after a reload an enabled-but-unloaded provider had only the word `active`
 * beside it. The catalogue is the durable evidence.
 */
describe("an enabled provider the router loaded nothing from", () => {
  it("says so instead of reading active", () => {
    state.providers = { anthropic: provider(true) };
    state.models = [];
    render(<ModelsSection />);

    expect(screen.getByText("On, but no models loaded")).toBeInTheDocument();
    expect(screen.queryByText("active")).toBeNull();
    expect(screen.getByText("on")).toBeInTheDocument();
  });

  it("reads active once a model of its own is in the catalogue", () => {
    state.providers = { anthropic: provider(true) };
    state.models = [
      {
        id: "claude-haiku-4-5",
        provider: "anthropic",
        context_window: 200000,
        input_price_per_million: 1,
        output_price_per_million: 5,
      },
    ];
    render(<ModelsSection />);

    expect(screen.queryByText("On, but no models loaded")).toBeNull();
    expect(screen.getByText("active")).toBeInTheDocument();
  });

  it("accuses nobody while the catalogue is still loading", () => {
    state.providers = { anthropic: provider(true) };
    state.models = undefined as unknown as Array<Record<string, unknown>>;
    render(<ModelsSection />);

    expect(screen.queryByText("On, but no models loaded")).toBeNull();
    expect(screen.getByText("active")).toBeInTheDocument();
  });

  it("leaves a provider that is off alone", () => {
    state.providers = { ollama: provider(false) };
    state.models = [];
    render(<ModelsSection />);

    expect(screen.queryByText("On, but no models loaded")).toBeNull();
    expect(screen.getByText("off")).toBeInTheDocument();
  });
});

/**
 * The local-model story on this screen (L1/L2/L3): a provider that needs no
 * key, a catalogue that can be re-asked, and a default model that may not be
 * the one answering.
 */
describe("a provider that needs no key", () => {
  const installed = {
    id: "qwen3:8b",
    provider: "ollama",
    context_window: 262_144,
    input_price_per_million: 0,
    output_price_per_million: 0,
    supports_tools: true,
  };

  it("says so instead of counting the keys it does not have", () => {
    state.providers = { ollama: provider(true, false) };
    state.models = [installed];
    render(<ModelsSection />);

    expect(screen.getByText(/no key needed/)).toBeInTheDocument();
    expect(screen.queryByText(/0 keys/)).toBeNull();
  });

  it("can be switched on with no key, and is told what that found", async () => {
    state.providers = { ollama: provider(false, false) };
    state.result = {
      id: "ollama",
      enabled: true,
      loaded: true,
      discovered_models: 3,
      discovery_error: null,
      warning: null,
    };
    render(<ModelsSection />);

    const toggle = screen.getByRole("switch", { name: "Enable ollama" });
    expect(toggle).toBeEnabled();
    await userEvent.click(toggle);

    expect(state.calls).toEqual([{ provider: "ollama", enabled: true }]);
    expect(useUiStore.getState().toast).toBe("ollama on — found 3 models");
  });

  it("lists its installed models and lets one become the chat model", async () => {
    state.providers = { ollama: provider(true, false) };
    state.models = [installed];
    render(<ModelsSection />);

    await userEvent.click(screen.getByRole("button", { name: "qwen3:8b" }));

    expect(state.writes).toEqual([
      { model: "qwen3:8b", fallback_models: ["claude-sonnet-4-6"] },
    ]);
  });
});

/**
 * `useRefreshModels` existed and was imported by no view — so a model pulled
 * after boot could not reach the picker without restarting the daemon.
 */
describe("refreshing the catalogue", () => {
  it("asks the daemon and reports what came back", async () => {
    state.refreshResult = [
      { id: "qwen3:8b", provider: "ollama" },
      { id: "llama3.1", provider: "ollama" },
    ];
    render(<ModelsSection />);

    await userEvent.click(
      screen.getByRole("button", { name: /refresh models/i }),
    );

    expect(state.refreshes).toBe(1);
    expect(useUiStore.getState().toast).toBe("2 models in the catalogue");
  });

  it("says an empty answer is empty rather than saying nothing", async () => {
    state.refreshResult = [];
    render(<ModelsSection />);

    await userEvent.click(
      screen.getByRole("button", { name: /refresh models/i }),
    );

    expect(useUiStore.getState().toast).toMatch(/No models/);
  });

  it("surfaces a refusal", async () => {
    state.refreshFail = new Error("daemon unreachable");
    render(<ModelsSection />);

    await userEvent.click(
      screen.getByRole("button", { name: /refresh models/i }),
    );

    expect(useUiStore.getState().toast).toBe(
      "Could not refresh — daemon unreachable",
    );
  });
});

/**
 * L3: the shipped templates pin Claude ids, so on a local-only install the
 * ladder answers with something else. Showing only the configured id is a
 * silent substitution.
 */
describe("when the configured model is not the one that answers", () => {
  it("names both", () => {
    state.daemonLlm = {
      default_model: "claude-haiku-4-5",
      default_model_routable: false,
      effective_default_model: "qwen3:8b",
    };
    render(<ModelsSection />);

    expect(
      screen.getByText(
        "configured: claude-haiku-4-5 — not available, using qwen3:8b",
      ),
    ).toBeInTheDocument();
  });

  it("says nothing when the configured model is routable", () => {
    state.daemonLlm = {
      default_model: "claude-haiku-4-5",
      default_model_routable: true,
      effective_default_model: "claude-haiku-4-5",
    };
    render(<ModelsSection />);

    expect(screen.queryByText(/not available/)).toBeNull();
  });
});

/**
 * GAP-08c's other half: the design's `41k tok today` per provider. It used to
 * be `ProviderUsageSummary.total_tokens`, which is *lifetime* — the same
 * number under a heading that said today.
 */
describe("per-provider usage (GAP-08c, T50)", () => {
  it("reports today's tokens and spend for a provider that ran", () => {
    render(<ModelsSection />);

    expect(screen.getByText(/41k tok today/)).toBeInTheDocument();
    expect(screen.getByText(/\$0\.0300/)).toBeInTheDocument();
  });

  /** A provider with no calls today says so — it does not borrow a lifetime figure. */
  it("says a provider has not been called today", () => {
    render(<ModelsSection />);

    expect(screen.getByText(/No calls today/)).toBeInTheDocument();
  });

  it("makes no claim that per-provider counts are lifetime totals", () => {
    render(<ModelsSection />);

    expect(screen.queryByText(/lifetime/i)).toBeNull();
  });
});

/**
 * D-F: the header's `Add provider` was a toast saying the key editor did not
 * exist. There is no provider to add — the three are compiled in — so the
 * control is `Add key`, and it opens the form.
 */
describe("adding a key", () => {
  it("opens the form from the card header, where the old toast was", async () => {
    render(<ModelsSection />);

    await userEvent.click(screen.getByRole("button", { name: "Add key" }));

    expect(screen.getByRole("radiogroup", { name: "Provider" })).toBeVisible();
    expect(screen.getByRole("radio", { name: "anthropic" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(screen.getByLabelText("API key")).toBeInTheDocument();
    expect(
      screen.queryByText(
        "Adding a provider needs the key editor, which is not built yet",
      ),
    ).toBeNull();
    expect(useUiStore.getState().toast).toBeNull();
    expect(screen.queryByRole("button", { name: "Add provider" })).toBeNull();
  });

  it("opens on the row's own provider from that row's Add key", async () => {
    state.providers = {
      anthropic: provider(true),
      ollama: provider(false, false),
    };
    render(<ModelsSection />);

    await userEvent.click(
      screen.getByRole("button", { name: "Add key for ollama" }),
    );

    expect(screen.getByRole("radio", { name: "ollama" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    // Ollama needs no key: the form says so instead of asking for one.
    expect(screen.queryByLabelText("API key")).toBeNull();
    expect(screen.getByText(/ollama needs no API key/)).toBeInTheDocument();
  });

  it("turns a switched-off provider on through the section's own switch path", async () => {
    state.providers = { anthropic: provider(false) };
    render(<ModelsSection />);

    await userEvent.click(
      screen.getByRole("button", { name: "Add key for anthropic" }),
    );
    expect(screen.getByText(/anthropic is switched off/)).toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", { name: "Turn anthropic on" }),
    );

    // The same mutation, and the same toast, as the row's switch.
    expect(state.calls).toEqual([{ provider: "anthropic", enabled: true }]);
    expect(useUiStore.getState().toast).toBe("anthropic on — found 1 model");
    expect(state.upserts).toEqual([]);
  });

  it("closes from the header button it was opened with", async () => {
    render(<ModelsSection />);

    await userEvent.click(screen.getByRole("button", { name: "Add key" }));
    await userEvent.click(screen.getByRole("button", { name: "Add key" }));

    expect(screen.queryByRole("radiogroup", { name: "Provider" })).toBeNull();
  });
});

/**
 * The keys a provider already holds. `Make primary` and `Remove` are the two
 * writes; the priority body is `key_id` while the upsert body is `id`, and a
 * test is the only thing that keeps the two straight.
 */
describe("a provider's stored keys", () => {
  const stored = (
    id: string,
    priority: "primary" | "fallback",
    source = "api_console",
  ) => ({
    id,
    masked_secret: `sk-ant-…${id.slice(-4)}`,
    tier: null,
    priority,
    source,
    notes: null,
    status: "healthy",
    monthly_usage_usd: null,
  });

  function twoKeys() {
    state.providers = {
      anthropic: {
        ...provider(true),
        keys: [
          stored("anthropic_1700000001", "primary"),
          // Written by the CLI, which sends display labels.
          stored("anthropic_1700000002", "fallback", "API Console"),
        ],
      },
    };
  }

  it("lists each key with its masked secret, priority, source and health", () => {
    twoKeys();
    render(<ModelsSection />);

    const list = screen.getByRole("list", { name: "anthropic keys" });
    const rows = list.querySelectorAll("li");
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent(
      "sk-ant-…0001 · primary · API Console · healthy",
    );
    expect(rows[1]).toHaveTextContent(
      "sk-ant-…0002 · fallback · API Console · healthy",
    );
  });

  it("makes a fallback key primary with `key_id`, not `id`", async () => {
    twoKeys();
    render(<ModelsSection />);

    // Only the key that is not primary offers it.
    expect(
      screen.queryByRole("button", {
        name: "Make anthropic_1700000001 primary",
      }),
    ).toBeNull();
    await userEvent.click(
      screen.getByRole("button", { name: "Make anthropic_1700000002 primary" }),
    );

    expect(state.priorities).toEqual([
      {
        provider: "anthropic",
        key_id: "anthropic_1700000002",
        priority: "primary",
      },
    ]);
    expect(state.priorities[0]).not.toHaveProperty("id");
  });

  it("removes a key only on the second click", async () => {
    twoKeys();
    render(<ModelsSection />);

    await userEvent.click(
      screen.getByRole("button", { name: "Remove anthropic_1700000002" }),
    );
    expect(state.removals).toEqual([]);
    await userEvent.click(
      screen.getByRole("button", {
        name: "Confirm removing anthropic_1700000002",
      }),
    );

    expect(state.removals).toEqual([
      { provider: "anthropic", keyId: "anthropic_1700000002" },
    ]);
  });

  it("disarms when the click goes elsewhere, and deletes nothing", async () => {
    twoKeys();
    render(<ModelsSection />);

    await userEvent.click(
      screen.getByRole("button", { name: "Remove anthropic_1700000002" }),
    );
    await userEvent.click(document.body);

    expect(
      screen.getByRole("button", { name: "Remove anthropic_1700000002" }),
    ).toHaveTextContent("Remove");
    await userEvent.click(
      screen.getByRole("button", { name: "Remove anthropic_1700000002" }),
    );
    expect(state.removals).toEqual([]);
  });

  it("disarms one key when another key's Remove is pressed", async () => {
    twoKeys();
    render(<ModelsSection />);

    await userEvent.click(
      screen.getByRole("button", { name: "Remove anthropic_1700000001" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Remove anthropic_1700000002" }),
    );

    expect(state.removals).toEqual([]);
    expect(
      screen.getByRole("button", { name: "Remove anthropic_1700000001" }),
    ).toBeInTheDocument();
  });

  it("draws no key rows for a keyless provider that holds none", () => {
    state.providers = { ollama: provider(true, false) };
    render(<ModelsSection />);

    expect(screen.queryByRole("list", { name: "ollama keys" })).toBeNull();
    expect(screen.getByText("no key needed · round_robin")).toBeInTheDocument();
  });
});
