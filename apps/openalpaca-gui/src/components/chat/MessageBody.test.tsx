/**
 * `MessageBody`'s mapping from `parseProse`'s blocks onto elements.
 *
 * The parser has its own tests (`prose.test.ts`); these are the two P4 cases
 * where *what the parser says* and *what the DOM shows* could still disagree —
 * an emphasis run wrapping a code span, where the renderer used to pick one of
 * the two and drop the other, and the blockquote element itself.
 */

import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { MessageBody } from "./MessageBody";

describe("MessageBody — emphasis around a code span (P4)", () => {
  it("wraps the <code> in the emphasis element instead of losing one", () => {
    const { container } = render(
      <MessageBody text="the file is **`alpaca-fiber-notes`**" />,
    );
    const code = container.querySelector("code");
    expect(code).not.toBeNull();
    expect(code?.textContent).toBe("alpaca-fiber-notes");
    expect(code?.closest("strong")).not.toBeNull();
    // The asterisks the owner saw are gone.
    expect(container.textContent).toBe("the file is alpaca-fiber-notes");
  });
});

describe("MessageBody — blockquotes (P4)", () => {
  it("renders a quoted run as one <blockquote>, not a literal '>'", () => {
    const { container } = render(
      <MessageBody text={"> the alpaca is not a llama\n> nor a camel"} />,
    );
    const quotes = container.querySelectorAll("blockquote");
    expect(quotes).toHaveLength(1);
    expect(quotes[0]?.textContent).toBe(
      "the alpaca is not a llama\nnor a camel",
    );
    expect(screen.queryByText(/^>/)).toBeNull();
  });

  it("renders the inline set inside the quote", () => {
    const { container } = render(
      <MessageBody text="> **note:** run `cargo test`" />,
    );
    const quote = container.querySelector("blockquote");
    expect(quote?.querySelector("strong")?.textContent).toBe("note:");
    expect(quote?.querySelector("code")?.textContent).toBe("cargo test");
  });
});
