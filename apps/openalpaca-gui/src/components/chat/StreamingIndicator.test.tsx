/**
 * V9 — the reasoning box's scroll behaviour.
 *
 * The box is capped at 88px with its own scrollbar, so once a model has been
 * thinking for a few seconds what it shows is the *oldest* lines. A comment
 * used to credit `overflow-anchor` for keeping it on the newest ones, and
 * nothing set that property. This is the behaviour it claimed.
 *
 * jsdom has no layout, so the three scroll metrics are installed by hand —
 * which is also the only way to say "the reader has scrolled up" in a test.
 */

import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ReasoningPanel, isPinnedToBottom } from "./StreamingIndicator";

/** The scroll metrics jsdom will not compute, plus a settable `scrollTop`. */
function withScrollMetrics(
  el: HTMLElement,
  {
    scrollHeight,
    clientHeight,
  }: { scrollHeight: number; clientHeight: number },
): { get scrollTop(): number; set scrollTop(value: number) } {
  let scrollTop = 0;
  Object.defineProperty(el, "scrollHeight", {
    configurable: true,
    get: () => scrollHeight,
  });
  Object.defineProperty(el, "clientHeight", {
    configurable: true,
    get: () => clientHeight,
  });
  Object.defineProperty(el, "scrollTop", {
    configurable: true,
    get: () => scrollTop,
    set: (value: number) => {
      scrollTop = value;
    },
  });
  return {
    get scrollTop() {
      return scrollTop;
    },
    set scrollTop(value: number) {
      scrollTop = value;
    },
  };
}

function panel(text: string) {
  const result = render(<ReasoningPanel text={text} />);
  const box = result.container.firstElementChild as HTMLElement;
  return { ...result, box };
}

describe("isPinnedToBottom", () => {
  it("allows a couple of pixels of sub-pixel slack", () => {
    expect(
      isPinnedToBottom({ scrollHeight: 200, scrollTop: 112, clientHeight: 88 }),
    ).toBe(true);
    expect(
      isPinnedToBottom({ scrollHeight: 200, scrollTop: 109, clientHeight: 88 }),
    ).toBe(true);
    expect(
      isPinnedToBottom({ scrollHeight: 200, scrollTop: 40, clientHeight: 88 }),
    ).toBe(false);
  });

  it("treats a box too short to scroll as pinned", () => {
    expect(
      isPinnedToBottom({ scrollHeight: 40, scrollTop: 0, clientHeight: 88 }),
    ).toBe(true);
  });
});

describe("ReasoningPanel (S2, V9)", () => {
  it("follows the newest text while it is pinned to the bottom", () => {
    const { box, rerender } = panel("first thought");
    const scroll = withScrollMetrics(box, {
      scrollHeight: 400,
      clientHeight: 88,
    });

    rerender(<ReasoningPanel text={"first thought\nsecond thought"} />);

    expect(scroll.scrollTop).toBe(400);
  });

  it("leaves a reader who scrolled up where they are", () => {
    const { box, rerender } = panel("first thought");
    const scroll = withScrollMetrics(box, {
      scrollHeight: 400,
      clientHeight: 88,
    });

    // The reader drags back to read something the model said earlier.
    scroll.scrollTop = 20;
    fireEvent.scroll(box);

    rerender(<ReasoningPanel text={"first thought\nsecond thought"} />);

    expect(scroll.scrollTop).toBe(20);
  });

  it("re-pins when the reader scrolls back to the bottom", () => {
    const { box, rerender } = panel("first thought");
    const scroll = withScrollMetrics(box, {
      scrollHeight: 400,
      clientHeight: 88,
    });

    scroll.scrollTop = 20;
    fireEvent.scroll(box);
    scroll.scrollTop = 312;
    fireEvent.scroll(box);

    rerender(<ReasoningPanel text={"first thought\nsecond thought"} />);

    expect(scroll.scrollTop).toBe(400);
  });

  it("still renders the reasoning it was given", () => {
    const { box } = panel("they want the capital of France");
    expect(box.textContent).toBe("they want the capital of France");
  });
});
