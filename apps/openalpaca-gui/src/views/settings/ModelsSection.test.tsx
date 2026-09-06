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
}));

vi.mock("@/hooks/useSettings", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/useSettings")>()),
  useLlmSettings: () => ({
    data: {
      orchestrator: { model: "claude-haiku-4-5", fallback_models: [] },
      providers: state.providers,
    },
    isPending: false,
    error: null,
  }),
  useModels: () => ({ data: [], isPending: false, error: null }),
  useProviderUsage: () => ({ data: [], isPending: false, error: null }),
  useSetProviderEnabled: () => ({
    isPending: false,
    mutate: (
      input: { provider: string; enabled: boolean },
      options?: {
        onSuccess?: (row: { id: string; enabled: boolean }) => void;
        onError?: (error: Error) => void;
      },
    ) => {
      state.calls.push(input);
      if (state.fail !== null) options?.onError?.(state.fail);
      else options?.onSuccess?.({ id: input.provider, enabled: input.enabled });
    },
  }),
}));

vi.mock("@/hooks/useOrchestrator", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/useOrchestrator")>()),
  useOrchestratorConfig: () => ({
    data: { model: "claude-haiku-4-5", fallback_models: [] },
    isPending: false,
    error: null,
  }),
  useUpdateOrchestratorConfig: () => ({ isPending: false, mutate: vi.fn() }),
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

  it("no longer carries the gap note that said the switch was dead", () => {
    render(<ModelsSection />);

    expect(
      screen.queryByText(/Provider enable\/disable not yet available/),
    ).toBeNull();
  });
});
