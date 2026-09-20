import { describe, expect, it } from "vitest";

import type {
  DaemonLlmStatus,
  ProviderEnabledResponse,
  ProviderInfo,
} from "@/lib/api/types";

import {
  effectiveModelNote,
  providerKeyLine,
  providerToggleToast,
} from "./models-copy";

const providerInfo = (over: Partial<ProviderInfo> = {}): ProviderInfo => ({
  enabled: true,
  key_selection_strategy: "round_robin",
  keys: [],
  requires_key: true,
  ...over,
});

const key = (id: string) =>
  ({
    id,
    masked_secret: "sk-…7f3a",
    tier: null,
    priority: "primary",
    source: "api_console",
    notes: null,
    status: "healthy",
    monthly_usage_usd: null,
  }) as ProviderInfo["keys"][number];

const toggled = (
  over: Partial<ProviderEnabledResponse> = {},
): ProviderEnabledResponse => ({
  id: "ollama",
  enabled: true,
  loaded: true,
  discovered_models: 3,
  discovery_error: null,
  warning: null,
  ...over,
});

/**
 * L1: the provider designed to hold no key read `0 keys · round_robin` — one
 * missing step away from working, beside an Add control that cannot supply it.
 */
describe("the key line under a provider", () => {
  it("says a local provider needs none", () => {
    expect(providerKeyLine(providerInfo({ requires_key: false }))).toBe(
      "no key needed · round_robin",
    );
  });

  it("still counts nothing for a provider that does need one", () => {
    expect(providerKeyLine(providerInfo())).toBe("0 keys · round_robin");
  });

  it("counts the keys a provider actually holds", () => {
    expect(providerKeyLine(providerInfo({ keys: [key("k1")] }))).toBe(
      "1 key · round_robin",
    );
    // A key written by hand for a keyless provider is reported, not hidden:
    // the line says what is there.
    expect(
      providerKeyLine(
        providerInfo({ requires_key: false, keys: [key("k1"), key("k2")] }),
      ),
    ).toBe("2 keys · round_robin");
  });
});

/**
 * L3: `[orchestrator] model` may name a model this install cannot serve —
 * every shipped agent template pins a Claude id. A picker that shows only that
 * id disagrees with every reply the daemon gives.
 */
describe("the note about the model that would really answer", () => {
  const llm = (over: Partial<DaemonLlmStatus> = {}): DaemonLlmStatus => ({
    default_model: "claude-haiku-4-5",
    default_model_routable: false,
    effective_default_model: "qwen3:8b",
    ...over,
  });

  it("names the substitution", () => {
    expect(effectiveModelNote(llm())).toBe(
      "configured: claude-haiku-4-5 — not available, using qwen3:8b",
    );
  });

  it("says nothing when the configured model is the one that answers", () => {
    expect(
      effectiveModelNote(
        llm({
          default_model_routable: true,
          effective_default_model: "claude-haiku-4-5",
        }),
      ),
    ).toBeNull();
  });

  it("says nothing routable exists, and where to fix it", () => {
    const note = effectiveModelNote(llm({ effective_default_model: null }));
    expect(note).toContain("no model is");
    expect(note).toContain("needs no key");
  });

  /**
   * M8: an install that never set a default sends `default_model: null`, not
   * `""`. "configured: null — not available" would be a sentence about a
   * setting that does not exist.
   */
  it("does not report an unconfigured default as an unavailable one", () => {
    expect(effectiveModelNote(llm({ default_model: null }))).toBe(
      "No chat model is configured — using qwen3:8b",
    );
    const nothing = effectiveModelNote(
      llm({ default_model: null, effective_default_model: null }),
    );
    expect(nothing).toContain("No chat model is configured");
    expect(nothing).toContain("needs no key");
    expect(nothing).not.toContain("null");
  });

  /** A daemon with no router, or one too old for the block, is not guessed at. */
  it("is silent about a daemon that does not say", () => {
    expect(effectiveModelNote(null)).toBeNull();
    expect(effectiveModelNote(undefined)).toBeNull();
  });
});

/**
 * L2: the enable's own answer about the catalogue. Switching a local provider
 * on and being told only `ollama on` leaves the one question that matters —
 * can the daemon see what I installed? — unanswered.
 */
describe("what a provider toggle says afterwards", () => {
  it("reports how many models the enable found", () => {
    expect(providerToggleToast(toggled())).toBe("ollama on — found 3 models");
    expect(providerToggleToast(toggled({ discovered_models: 1 }))).toBe(
      "ollama on — found 1 model",
    );
  });

  it("never leaves a zero to speak for itself", () => {
    expect(providerToggleToast(toggled({ discovered_models: 0 }))).toBe(
      "ollama on — it reported no models",
    );
    expect(
      providerToggleToast(
        toggled({
          discovered_models: 0,
          discovery_error: "connection refused",
        }),
      ),
    ).toBe("ollama on — its model list could not be read (connection refused)");
  });

  it("keeps the older facts: a disable, and a write that did not load", () => {
    expect(
      providerToggleToast(
        toggled({ enabled: false, loaded: false, discovered_models: 0 }),
      ),
    ).toBe("ollama off");
    expect(
      providerToggleToast(
        toggled({
          id: "anthropic",
          loaded: false,
          discovered_models: 0,
          warning: "No keys for Anthropic",
        }),
      ),
    ).toBe("anthropic on, but the daemon could not load it");
  });
});
