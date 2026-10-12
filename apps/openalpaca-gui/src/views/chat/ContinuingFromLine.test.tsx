import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ContinuingFromLine } from "./ContinuingFromLine";

describe("ContinuingFromLine (design 11 §5, CQ-7)", () => {
  it("names the previous conversation's topic, escaped, and when it ended", () => {
    const { container } = render(
      <ContinuingFromLine
        continuedFrom={{ session_id: "s-prev", topic: "Lisbon \u{202E}trip" }}
        endedAt="2026-09-28 11:00:00"
      />,
    );
    expect(container.textContent).toBe(
      "Continuing from: Lisbon <U+202E>trip (ended 28 Sep)",
    );
    expect(
      container.querySelector('[data-hidden-char="U+202E"]'),
    ).not.toBeNull();
  });

  it("names a previous conversation that left no state, and no date it does not know", () => {
    const { container } = render(
      <ContinuingFromLine
        continuedFrom={{ session_id: "s-prev", topic: "" }}
        endedAt={null}
      />,
    );
    expect(container.textContent).toBe(
      "Continuing from: the previous conversation",
    );
  });
});
