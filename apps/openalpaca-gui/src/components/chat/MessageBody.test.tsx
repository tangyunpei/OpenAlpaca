/**
 * `MessageBody`'s mapping from `parseProse`'s blocks onto elements.
 *
 * The parser has its own tests (`prose.test.ts`); these are the two P4 cases
 * where *what the parser says* and *what the DOM shows* could still disagree —
 * an emphasis run wrapping a code span, where the renderer used to pick one of
 * the two and drop the other, and the blockquote element itself — and the
 * `wrapAnywhere` a text someone decides on is drawn with.
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

/** Every kind of block the renderer draws, each holding an escape. */
const DECIDED = [
  "A paragraph <U+202E> here.",
  "",
  "## A heading <U+200B>",
  "",
  "- a list item <U+2800>",
  "",
  "```sh",
  "curl https://example.com/<U+200B>" + "x".repeat(200),
  "```",
  "",
  "| key <U+202E> | value |",
  "| --- | --- |",
  "| a | b<U+200B> |",
].join("\n");

describe("MessageBody — wrapAnywhere (a text someone decides on)", () => {
  /**
   * A candidate note's text is escaped and counted before the owner
   * confirms it (`lib/hidden-chars`), so no block may put an escape out of
   * sight: a code block wraps instead of scrolling sideways (behind a
   * scrollbar macOS does not draw), a table header may wrap, and every
   * block wraps a run with no break opportunity anywhere.
   */
  it("wraps every block, a code block and a table header included", () => {
    const { container } = render(<MessageBody text={DECIDED} wrapAnywhere />);
    const pre = container.querySelector("pre");
    expect(pre?.className).toContain("whitespace-pre-wrap");
    expect(pre?.className).toContain("wrap-anywhere");
    for (const th of Array.from(container.querySelectorAll("th"))) {
      expect(th.className).not.toContain("whitespace-nowrap");
      expect(th.className).toContain("wrap-anywhere");
    }
    for (const selector of ["p", "h2", "ul", "table"]) {
      const block = container.querySelector(selector);
      const wrapped = block?.closest(".wrap-anywhere");
      expect(wrapped, selector).not.toBeNull();
    }
  });

  it("leaves a transcript's blocks as they were", () => {
    const { container } = render(<MessageBody text={DECIDED} />);
    expect(container.querySelector("pre")?.className).not.toContain(
      "whitespace-pre-wrap",
    );
    expect(container.querySelector("th")?.className).toContain(
      "whitespace-nowrap",
    );
    expect(container.querySelector(".wrap-anywhere")).toBeNull();
  });

  /**
   * A row wider than its header: a transcript drops the extra cells, as
   * GitHub's tables do; a text someone decides on keeps every one.
   */
  it("keeps a row's cells past its header only for a text someone decides on", () => {
    const wide = "| a |\n|---|\n| b | c<U+202E> |";
    const decided = render(<MessageBody text={wide} wrapAnywhere />);
    expect(
      Array.from(decided.container.querySelectorAll("td")).map(
        (cell) => cell.textContent,
      ),
    ).toEqual(["b", "c<U+202E>"]);
    expect(decided.container.querySelectorAll("th")).toHaveLength(2);
    decided.unmount();

    const transcript = render(<MessageBody text={wide} />);
    expect(
      Array.from(transcript.container.querySelectorAll("td")).map(
        (cell) => cell.textContent,
      ),
    ).toEqual(["b"]);
  });
});
