/**
 * `decideChatModel` — the table G9's rule is easiest to read as.
 *
 * The end-to-end half lives in `ChatView.test.tsx` (what the picker says, what
 * the send carries); this pins the cases that are about *not* acting: a
 * catalogue that has not answered, a daemon with no `llm` block, and the
 * self-replacement that would otherwise loop.
 */

import { describe, expect, it } from "vitest";

import type { ModelEntry } from "@/lib/api/types";

import { decideChatModel, modelReplacedToast } from "./chat-model";

function catalogue(...ids: string[]): ModelEntry[] {
  return ids.map((id) => ({
    id,
    provider: id.startsWith("qwen") ? "ollama" : "anthropic",
    context_window: 40960,
    input_price_per_million: 0,
    output_price_per_million: 0,
    supports_tools: true,
  }));
}

describe("decideChatModel", () => {
  it("seeds an empty store from the effective default", () => {
    expect(
      decideChatModel({
        held: null,
        effective: "qwen3:8b",
        models: catalogue("qwen3:8b"),
      }),
    ).toEqual({ action: "seed", model: "qwen3:8b" });
  });

  it("seeds nothing when the daemon can route nothing", () => {
    expect(
      decideChatModel({ held: null, effective: null, models: catalogue() }),
    ).toEqual({ action: "keep" });
  });

  it("seeds nothing while the status is unknown", () => {
    // `undefined` is "not answered, or a daemon with no `llm` block" — never
    // an invitation to guess.
    expect(
      decideChatModel({
        held: null,
        effective: undefined,
        models: catalogue("qwen3:8b"),
      }),
    ).toEqual({ action: "keep" });
  });

  it("leaves a held model that is in the catalogue alone", () => {
    expect(
      decideChatModel({
        held: "qwen3:8b",
        effective: "llama3:70b",
        models: catalogue("qwen3:8b", "llama3:70b"),
      }),
    ).toEqual({ action: "keep" });
  });

  it("replaces a held model that left the catalogue", () => {
    expect(
      decideChatModel({
        held: "claude-sonnet-4-6",
        effective: "qwen3:8b",
        models: catalogue("qwen3:8b"),
      }),
    ).toEqual({
      action: "replace",
      model: "qwen3:8b",
      previous: "claude-sonnet-4-6",
    });
  });

  it("clears a held model when nothing is routable", () => {
    expect(
      decideChatModel({
        held: "claude-sonnet-4-6",
        effective: null,
        models: catalogue(),
      }),
    ).toEqual({
      action: "replace",
      model: null,
      previous: "claude-sonnet-4-6",
    });
  });

  it("does not act on a catalogue that has not answered", () => {
    // A models fetch that failed is not evidence the model is gone, and
    // clearing on it would take the composer's label away for a transport
    // blip.
    expect(
      decideChatModel({
        held: "claude-sonnet-4-6",
        effective: "qwen3:8b",
        models: undefined,
      }),
    ).toEqual({ action: "keep" });
  });

  it("does not replace a model with itself", () => {
    // A catalogue that does not list what the daemon says it would route is
    // a daemon disagreeing with itself; replacing would loop on every render.
    expect(
      decideChatModel({
        held: "qwen3:8b",
        effective: "qwen3:8b",
        models: catalogue("llama3:70b"),
      }),
    ).toEqual({ action: "keep" });
  });
});

describe("modelReplacedToast", () => {
  it("names the new model, its provider and the one that went", () => {
    expect(modelReplacedToast("qwen3:8b", "ollama", "claude-sonnet-4-6")).toBe(
      "Chat model → qwen3:8b (ollama) — claude-sonnet-4-6 is no longer available",
    );
  });

  it("says so plainly when there is nothing to fall back to", () => {
    expect(modelReplacedToast(null, null, "claude-sonnet-4-6")).toBe(
      "claude-sonnet-4-6 is no longer available — no model is routable",
    );
  });
});
