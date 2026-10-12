import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { escapeHidden } from "@/lib/hidden-chars";

import { EscapedText } from "./EscapedText";

describe("EscapedText", () => {
  /**
   * What tells a real escape from the eight characters `<U+202E>` typed into
   * a value is its styling alone — both are text. Every surface that draws
   * an escape draws it through this span, so the span's look is pinned here:
   * the red ink on the red tint the manual calls "highlighted", in mono, and
   * never split across two lines.
   */
  it("sets each escape apart in a styled span and leaves the rest as text", () => {
    const { container } = render(
      <p>
        <EscapedText
          escaped={escapeHidden("rm \u{202E}fdp\u{200B} <U+0041>")}
        />
      </p>,
    );
    const chips = Array.from(container.querySelectorAll("[data-hidden-char]"));

    expect(chips.map((chip) => chip.getAttribute("data-hidden-char"))).toEqual([
      "U+202E",
      "U+200B",
    ]);
    for (const chip of chips) {
      expect(chip).toHaveClass(
        "bg-red-tint",
        "text-red-ink",
        "font-mono",
        "whitespace-nowrap",
      );
    }
    // The typed `<U+0041>` is text, not a span.
    expect(container.textContent).toBe("rm <U+202E>fdp<U+200B> <U+0041>");
  });
});
