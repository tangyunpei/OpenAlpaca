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
    warning: string | null;
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
            warning: null,
          },
        );
    },
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

const provider = (enabled: boolean) => ({
  enabled,
  key_selection_strategy: "round_robin",
  keys: [],
});

beforeEach(() => {
  state.providers = { anthropic: provider(true), ollama: provider(false) };
  state.calls = [];
  state.fail = null;
  state.result = null;
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
    expect(useUiStore.getState().toast).toBe("ollama on");
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
