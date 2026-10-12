import { describe, expect, it } from "vitest";

import table from "@/lib/hidden-chars.json";

import { suggestionHidden } from "./suggestion-hidden";

/**
 * Parity with the Rust count (`suggestion_hidden`), which the chat preview
 * and `openalpaca memory suggestions` read: both are held to the shared
 * table's worked cases. Each title and the reason there holds a tab or a line
 * break, which the one-line escape counts and the texts' does not, so a field
 * counted with the wrong escape fails; and in the first case each of the five
 * fields holds a different number of hidden characters (1 to 5), so a field
 * read in place of another, or left out, fails too.
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
          changes: c.changes,
          target: { title: c.target_title, excerpt: c.excerpt },
        }),
      ).toBe(c.hidden);
    }
  });
});
