import { describe, expect, it } from "vitest";

import type { DaemonLlmStatus } from "@/lib/api/types";

import { effectiveModelNote } from "./model-availability";

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
 * The move out of `views/settings/models-copy.ts` changed no Settings copy:
 * with the default audience the two nothing-routable branches read exactly as
 * the banner always read them.
 */
describe("the Settings banner's wording, pinned", () => {
  it("keeps both nothing-routable sentences byte for byte", () => {
    expect(
      effectiveModelNote({
        default_model: null,
        default_model_routable: false,
        effective_default_model: null,
      }),
    ).toBe(
      "No chat model is configured, and none is available. Turn a provider on below; a local one needs no key.",
    );
    expect(
      effectiveModelNote({
        default_model: "claude-haiku-4-5",
        default_model_routable: false,
        effective_default_model: null,
      }),
    ).toBe(
      "configured: claude-haiku-4-5 — not available, and no model is. Turn a provider on below; a local one needs no key.",
    );
  });
});

/**
 * Away from Settings there is no "below": the chat first-run card names the
 * screen instead. Only the call to action moves — the facts do not.
 */
describe("the same note, drawn somewhere other than Settings", () => {
  const nothing = (default_model: string | null): DaemonLlmStatus => ({
    default_model,
    default_model_routable: false,
    effective_default_model: null,
  });

  it("names the screen in both nothing-routable branches", () => {
    expect(effectiveModelNote(nothing(null), "elsewhere")).toBe(
      "No chat model is configured, and none is available. Open Settings → Models & keys and turn a provider on; a local one needs no key.",
    );
    expect(effectiveModelNote(nothing("claude-haiku-4-5"), "elsewhere")).toBe(
      "configured: claude-haiku-4-5 — not available, and no model is. Open Settings → Models & keys and turn a provider on; a local one needs no key.",
    );
  });

  it("leaves every branch with a model in it byte-identical", () => {
    const substituted: DaemonLlmStatus = {
      default_model: "claude-haiku-4-5",
      default_model_routable: false,
      effective_default_model: "qwen3:8b",
    };
    expect(effectiveModelNote(substituted, "elsewhere")).toBe(
      effectiveModelNote(substituted),
    );
    const unconfigured: DaemonLlmStatus = {
      ...substituted,
      default_model: null,
    };
    expect(effectiveModelNote(unconfigured, "elsewhere")).toBe(
      effectiveModelNote(unconfigured),
    );
    expect(effectiveModelNote(null, "elsewhere")).toBeNull();
    expect(effectiveModelNote(undefined, "elsewhere")).toBeNull();
  });
});
