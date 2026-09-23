import { describe, expect, it } from "vitest";

import type { ProviderEnabledResponse, ProviderInfo } from "@/lib/api/types";

import { providerKeyLine, providerToggleToast } from "./models-copy";

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
    ).toBe(
      "ollama on, but it could not be reached (connection refused) — start it and press Refresh models",
    );
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
