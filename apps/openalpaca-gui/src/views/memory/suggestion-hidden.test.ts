import { describe, expect, it } from "vitest";

import table from "@/lib/hidden-chars.json";

import { suggestionHidden } from "./suggestion-hidden";

/**
 * Parity with the Rust count (`suggestion_hidden`), which the chat preview
 * and `openalpaca memory suggestions` read: both are held to the shared
 * table's worked cases.
 */
describe("suggestionHidden — the shared table's suggestion cases", () => {
  it("counts every case as Rust counts it", () => {
    expect(table.suggestion_cases.length).toBeGreaterThan(0);
    for (const c of table.suggestion_cases) {
      expect(
        suggestionHidden({
          title: c.title,
          reason: c.reason,
          knowledge: c.knowledge,
          target: { title: c.target_title, excerpt: c.excerpt },
        }),
      ).toBe(c.hidden);
    }
  });
});
