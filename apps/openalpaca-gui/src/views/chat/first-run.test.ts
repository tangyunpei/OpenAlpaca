/**
 * D-G: "setup completed" is derived from `llm.effective_default_model`, and
 * the three states are told apart with `=== undefined` / `=== null` — a falsy
 * test would flash "no model" at every cold start, before the daemon answers.
 */

import { describe, expect, it } from "vitest";

import { firstRunBody, firstRunState } from "./first-run";

describe("whether this install can answer", () => {
  it("is unknown while the status has not answered", () => {
    expect(firstRunState(undefined)).toBe("unknown");
  });

  it("is no-model only on the daemon's explicit null", () => {
    expect(firstRunState(null)).toBe("no-model");
  });

  it("is ready for any string, the empty one included", () => {
    expect(firstRunState("llama3.2:3b")).toBe("ready");
    // M8: the daemon serialises `null`, never `""`, for "nothing". Only an
    // explicit `null` is the daemon saying no.
    expect(firstRunState("")).toBe("ready");
  });
});

describe("what the card says", () => {
  it("uses the daemon's own reading, with the screen named", () => {
    expect(
      firstRunBody({
        default_model: null,
        default_model_routable: false,
        effective_default_model: null,
      }),
    ).toBe(
      "No chat model is configured, and none is available. Open Settings → Models & keys and turn a provider on; a local one needs no key.",
    );
  });

  it("falls back to its own sentence when that reading has nothing to say", () => {
    const fallback =
      "No chat model is available. Open Settings → Models & keys and turn a provider on; a local one needs no key.";
    // A contradictory block: routable, yet nothing effective.
    expect(
      firstRunBody({
        default_model: "claude-haiku-4-5",
        default_model_routable: true,
        effective_default_model: null,
      }),
    ).toBe(fallback);
    expect(firstRunBody(null)).toBe(fallback);
    expect(firstRunBody(undefined)).toBe(fallback);
  });
});
