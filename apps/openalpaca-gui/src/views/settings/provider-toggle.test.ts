import { describe, expect, it } from "vitest";

import { withProviderEnabled } from "@/hooks/useSettings";
import type { LlmSettingsResponse } from "@/lib/api/types";
import { ApiError } from "@/lib/http";

import { providerToggleErrorCopy } from "./provider-toggle";

const settings: LlmSettingsResponse = {
  orchestrator: { model: "claude-haiku-4-5", fallback_models: [] },
  providers: {
    anthropic: {
      enabled: true,
      key_selection_strategy: "round_robin",
      keys: [],
    },
    ollama: { enabled: false, key_selection_strategy: "round_robin", keys: [] },
  },
};

describe("withProviderEnabled", () => {
  it("flips one provider and leaves the rest of the payload alone", () => {
    const next = withProviderEnabled(settings, "ollama", true);

    expect(next.providers.ollama?.enabled).toBe(true);
    expect(next.providers.anthropic).toBe(settings.providers.anthropic);
    expect(next.orchestrator).toBe(settings.orchestrator);
    expect(settings.providers.ollama?.enabled).toBe(false);
  });

  it("returns the payload untouched for a provider it does not hold", () => {
    expect(withProviderEnabled(settings, "groq", true)).toBe(settings);
  });
});

describe("providerToggleErrorCopy", () => {
  const apiError = (code: string, message: string, status = 409) =>
    new ApiError(message, status, code);

  it("tells the owner what to do about the default model's provider", () => {
    const copy = providerToggleErrorCopy(
      "anthropic",
      apiError("PROVIDER_IS_DEFAULT", "'anthropic' serves the default model"),
    );

    expect(copy).toContain("anthropic");
    expect(copy).toMatch(/pick a different model/i);
  });

  it("says the true thing when the default model places nowhere", () => {
    const copy = providerToggleErrorCopy(
      "anthropic",
      apiError(
        "DEFAULT_MODEL_UNRESOLVED",
        "the default model 'my-local-thing' does not name any provider",
      ),
      "my-local-thing",
    );

    // The refusal is not about anthropic serving anything — saying so would be
    // false — it is about the default model naming no provider at all.
    expect(copy).toContain("my-local-thing");
    expect(copy).toMatch(/does not resolve to any provider/i);
    expect(copy).not.toMatch(/anthropic serves/i);
  });

  it("still names the fix when it does not know the default model id", () => {
    const copy = providerToggleErrorCopy(
      "anthropic",
      apiError("DEFAULT_MODEL_UNRESOLVED", "x"),
    );

    expect(copy).toMatch(/does not resolve to any provider/i);
    expect(copy).toMatch(/Settings . Models/i);
  });

  it("renders its own sentence for every code the route can answer with", () => {
    expect(
      providerToggleErrorCopy("groq", apiError("PROVIDER_NOT_FOUND", "x", 404)),
    ).toMatch(/does not know a provider called groq/);
    expect(
      providerToggleErrorCopy(
        "ollama",
        apiError("LLM_NOT_CONFIGURED", "x", 503),
      ),
    ).toMatch(/No LLM router is configured/);
    expect(
      providerToggleErrorCopy(
        "ollama",
        apiError("DISK_WRITE_FAILED", "x", 500),
      ),
    ).toMatch(/could not write llm.toml/);
  });

  it("falls back to the daemon's own message rather than inventing one", () => {
    expect(providerToggleErrorCopy("ollama", new Error("boom"))).toBe(
      "ollama — boom",
    );
  });
});
