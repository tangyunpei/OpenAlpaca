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
