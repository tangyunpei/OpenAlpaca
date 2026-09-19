import { describe, expect, it } from "vitest";

import {
  parseInlineCode,
  parseProse,
  splitTableRow,
  type ProseBlock,
} from "./prose";

/** The paragraph text of a block, for the assertions that only want that. */
function paragraphText(block: ProseBlock | undefined): string {
  if (block?.kind !== "paragraph") throw new Error("not a paragraph");
  return block.segments.map((segment) => segment.text).join("");
}

describe("parseInlineCode (§3.10)", () => {
  it("splits backtick pairs into code segments", () => {
    expect(parseInlineCode("run `cargo tree` first")).toEqual([
      { text: "run ", code: false },
      { text: "cargo tree", code: true },
      { text: " first", code: false },
    ]);
  });

  it("treats an unpaired backtick as literal text", () => {
    expect(parseInlineCode("a ` b")).toEqual([{ text: "a ` b", code: false }]);
  });

  it("does not create an empty code span", () => {
    expect(parseInlineCode("a `` b")).toEqual([
      { text: "a ", code: false },
      { text: "``", code: false },
      { text: " b", code: false },
    ]);
  });
});

/**
 * G8 — the transcript printed `- **Research:** …` verbatim while the Library
 * rendered the same bytes properly. A completion report is written in
 * markdown; these are the parts of it a chat row shows.
 */
describe("emphasis (G8)", () => {
  it("reads **bold** and *italic*", () => {
    expect(parseInlineCode("**Research:** three are *stale*")).toEqual([
      { text: "Research:", code: false, strong: true },
      { text: " three are ", code: false },
      { text: "stale", code: false, em: true },
    ]);
  });

  it("leaves code spans alone — what is inside them is shown as written", () => {
    expect(parseInlineCode("run `a * b * c` now")).toEqual([
      { text: "run ", code: false },
      { text: "a * b * c", code: true },
      { text: " now", code: false },
    ]);
  });

  /**
   * This window reports on a machine where `task_id` is an ordinary word, so
   * underscores are never emphasis.
   */
  it("does not italicise the middle of an identifier", () => {
    expect(parseInlineCode("set default_max_tokens on the_provider")).toEqual([
      { text: "set default_max_tokens on the_provider", code: false },
    ]);
  });

  it("leaves a lone or spaced asterisk literal", () => {
    expect(parseInlineCode("2 * 3 = 6")).toEqual([
      { text: "2 * 3 = 6", code: false },
    ]);
    expect(parseInlineCode("a * b")).toEqual([{ text: "a * b", code: false }]);
    expect(parseInlineCode("5 stars *")).toEqual([
      { text: "5 stars *", code: false },
    ]);
  });
});

describe("parseProse", () => {
  it("splits on blank lines and drops empty paragraphs", () => {
    const blocks = parseProse("one\n\n\ntwo\n\n   \n");
    expect(blocks).toHaveLength(2);
    expect(paragraphText(blocks[0])).toBe("one");
    expect(paragraphText(blocks[1])).toBe("two");
  });

  it("keeps single newlines inside one paragraph", () => {
    const blocks = parseProse("one\ntwo");
    expect(blocks).toHaveLength(1);
    expect(paragraphText(blocks[0])).toBe("one\ntwo");
  });

  /** The shape a completion report actually arrives in. */
  it("reads a lead-in line followed by bullets as two blocks", () => {
    const blocks = parseProse(
      "Here is what I found:\n- **Research:** three connectors are stale\n- **Next:** re-run the audit",
    );

    expect(blocks).toHaveLength(2);
    expect(paragraphText(blocks[0])).toBe("Here is what I found:");
    const list = blocks[1];
    if (list?.kind !== "list") throw new Error("expected a list");
    expect(list.ordered).toBe(false);
    expect(list.items).toHaveLength(2);
    expect(list.items[0]?.[0]).toEqual({
      text: "Research:",
      code: false,
      strong: true,
    });
    expect(list.items[0]?.[1]?.text).toBe(" three connectors are stale");
  });

  it("reads a numbered list as an ordered one", () => {
    const blocks = parseProse("1. first\n2) second");
    const list = blocks[0];
    if (list?.kind !== "list") throw new Error("expected a list");
    expect(list.ordered).toBe(true);
    expect(list.items.map((item) => item[0]?.text)).toEqual([
      "first",
      "second",
    ]);
  });

  it("closes a list when prose resumes", () => {
    const blocks = parseProse("- one\n- two\nand that was that");
    expect(blocks.map((block) => block.kind)).toEqual(["list", "paragraph"]);
    expect(paragraphText(blocks[1])).toBe("and that was that");
  });

  it("gives every block its own key", () => {
    const keys = parseProse("intro\n- a\nafter\n\nlast").map(
      (block) => block.key,
    );
    expect(new Set(keys).size).toBe(keys.length);
  });
});

/**
 * T3 — what the transcript showed in the first real session on a local model:
 * a fenced code block as stray backticks, a table as raw `| a | b |` lines,
 * `## Heading` raw, `---` as three dashes, and an ordered list that restarted
 * at "1." after a code block.
 */
describe("fenced code (T3)", () => {
  it("takes a fenced block whole, with its language", () => {
    const blocks = parseProse(
      "try this:\n\n```rust\nlet x = 1;\n\nlet y = 2;\n```\n\nand then run it",
    );

    expect(blocks.map((block) => block.kind)).toEqual([
      "paragraph",
      "code",
      "paragraph",
    ]);
    const code = blocks[1];
    if (code?.kind !== "code") throw new Error("expected code");
    expect(code.language).toBe("rust");
    // Blank lines inside the fence belong to the block, not to the splitter.
    expect(code.text).toBe("let x = 1;\n\nlet y = 2;");
    expect(code.open).toBe(false);
  });

  it("leaves the block's own markup alone — it is shown as written", () => {
    const blocks = parseProse("```\n- **not** a list\n| not | a table |\n```");
    const code = blocks[0];
    if (code?.kind !== "code") throw new Error("expected code");
    expect(code.text).toBe("- **not** a list\n| not | a table |");
    expect(code.language).toBeNull();
  });

  it("accepts tildes, and a longer fence than the one that opened it", () => {
    const tildes = parseProse("~~~py\nprint(1)\n~~~");
    expect(tildes[0]?.kind).toBe("code");

    const longer = parseProse("```\nbody\n`````");
    const code = longer[0];
    if (code?.kind !== "code") throw new Error("expected code");
    expect(code.text).toBe("body");
    expect(code.open).toBe(false);
  });

  /**
   * The streaming half. Every delta re-parses the whole answer, so the moment
   * the opening fence lands the block has to *be* a block — showing three
   * backticks and then swapping them for a `<pre>` is the flicker.
   */
  it("renders a half-received fence as an open code block", () => {
    const opened = parseProse("here:\n\n```ts\nconst a =");
    expect(opened.map((block) => block.kind)).toEqual(["paragraph", "code"]);
    const code = opened[1];
    if (code?.kind !== "code") throw new Error("expected code");
    expect(code.open).toBe(true);
    expect(code.text).toBe("const a =");

    // Even the bare fence, one character into the block.
    const bare = parseProse("```");
    expect(bare[0]?.kind).toBe("code");
  });
});

describe("headings and thematic breaks (T3)", () => {
  it("reads # through #### as headings, with inline spans", () => {
    const blocks = parseProse(
      "# One\n## Two\n### Three\n#### Four\n##### Five is prose",
    );
    expect(blocks.map((block) => block.kind)).toEqual([
      "heading",
      "heading",
      "heading",
      "heading",
      "paragraph",
    ]);
    expect(
      blocks.map((block) => (block.kind === "heading" ? block.level : 0)),
    ).toEqual([1, 2, 3, 4, 0]);

    const emphasised = parseProse("## The `llm.toml` **rule**");
    const heading = emphasised[0];
    if (heading?.kind !== "heading") throw new Error("expected a heading");
    expect(heading.segments).toEqual([
      { text: "The ", code: false },
      { text: "llm.toml", code: true },
      { text: " ", code: false },
      { text: "rule", code: false, strong: true },
    ]);
  });

  it("needs a space after the hashes", () => {
    const blocks = parseProse("#nothashtag");
    expect(blocks[0]?.kind).toBe("paragraph");
  });

  it("reads ---, *** and ___ as a thematic break", () => {
    for (const rule of ["---", "***", "___", "-----"]) {
      const blocks = parseProse(`before\n${rule}\nafter`);
      expect(blocks.map((block) => block.kind)).toEqual([
        "paragraph",
        "rule",
        "paragraph",
      ]);
    }
  });

  it("does not mistake a bullet for a break", () => {
    const blocks = parseProse("- one\n- two");
    expect(blocks.map((block) => block.kind)).toEqual(["list"]);
  });
});

describe("tables (T3)", () => {
  it("reads a pipe table with its header", () => {
    const blocks = parseProse(
      "| Model | Context |\n|---|---:|\n| qwen3:8b | 8192 |\n| llama3 | 4096 |",
    );
    const table = blocks[0];
    if (table?.kind !== "table") throw new Error("expected a table");
    expect(table.header.map((cell) => cell[0]?.text)).toEqual([
      "Model",
      "Context",
    ]);
    expect(table.rows).toHaveLength(2);
    expect(table.rows[0]?.map((cell) => cell[0]?.text)).toEqual([
      "qwen3:8b",
      "8192",
    ]);
  });

  it("needs its delimiter row — a header alone is still prose", () => {
    const blocks = parseProse("| Model | Context |\nnot a delimiter");
    expect(blocks.map((block) => block.kind)).toEqual(["paragraph"]);
  });

  /** Half-received: the header and the rule have arrived, no rows yet. */
  it("renders a table whose rows have not streamed in yet", () => {
    const blocks = parseProse("| a | b |\n|---|---|");
    const table = blocks[0];
    if (table?.kind !== "table") throw new Error("expected a table");
    expect(table.rows).toEqual([]);
  });

  it("pads a short row rather than leaving it ragged", () => {
    const blocks = parseProse("| a | b | c |\n|---|---|---|\n| 1 | 2 |");
    const table = blocks[0];
    if (table?.kind !== "table") throw new Error("expected a table");
    expect(table.rows[0]).toHaveLength(3);
    expect(table.rows[0]?.[2]).toEqual([]);
  });

  it("keeps an escaped pipe inside its cell", () => {
    expect(splitTableRow("| a \\| b | c |")).toEqual(["a | b", "c"]);
  });
});

describe("ordered lists across an interleaved block (T3)", () => {
  it("resumes at the marker the model wrote, not at 1", () => {
    const blocks = parseProse(
      "1. first\n\n```\nsome code\n```\n\n2. second\n3. third",
    );
    expect(blocks.map((block) => block.kind)).toEqual(["list", "code", "list"]);
    const first = blocks[0];
    const resumed = blocks[2];
    if (first?.kind !== "list" || resumed?.kind !== "list")
      throw new Error("expected lists");
    expect(first.start).toBe(1);
    expect(resumed.start).toBe(2);
    expect(resumed.items.map((item) => item[0]?.text)).toEqual([
      "second",
      "third",
    ]);
  });

  it("leaves a bullet list's start at 1", () => {
    const blocks = parseProse("- a\n- b");
    const list = blocks[0];
    if (list?.kind !== "list") throw new Error("expected a list");
    expect(list.start).toBe(1);
  });
});

/**
 * P4 — the two gaps a local model hit in the very first live session after T3
 * shipped: `**`alpaca-fiber-notes`**` rendered with its asterisks showing, and
 * a `>` line rendered as a literal `>`.
 */
describe("emphasis around a code span (P4)", () => {
  it("reads a code span a bold run wraps", () => {
    expect(parseInlineCode("**`alpaca-fiber-notes`**")).toEqual([
      { text: "alpaca-fiber-notes", code: true, strong: true },
    ]);
  });

  it("reads a code span an italic run wraps", () => {
    expect(parseInlineCode("see *`llm.toml`* for it")).toEqual([
      { text: "see ", code: false },
      { text: "llm.toml", code: true, em: true },
      { text: " for it", code: false },
    ]);
  });

  it("carries the emphasis onto the words beside the span", () => {
    expect(parseInlineCode("**edit `llm.toml` first**")).toEqual([
      { text: "edit ", code: false, strong: true },
      { text: "llm.toml", code: true, strong: true },
      { text: " first", code: false, strong: true },
    ]);
  });

  /** The half that must not regress: a `*` inside a span is still literal. */
  it("still refuses to open emphasis inside a code span", () => {
    expect(parseInlineCode("run `a * b * c` now")).toEqual([
      { text: "run ", code: false },
      { text: "a * b * c", code: true },
      { text: " now", code: false },
    ]);
    // …including when the asterisks inside would otherwise pair with one
    // outside it.
    expect(parseInlineCode("`a * b` and * c")).toEqual([
      { text: "a * b", code: true },
      { text: " and * c", code: false },
    ]);
  });

  /** Streaming: the closing delimiters have not arrived yet. */
  it("leaves a half-received emphasis run literal", () => {
    expect(parseInlineCode("**`alpaca-fiber")).toEqual([
      { text: "**`alpaca-fiber", code: false },
    ]);
    expect(parseInlineCode("**`alpaca-fiber-notes`")).toEqual([
      { text: "**", code: false },
      { text: "alpaca-fiber-notes", code: true },
    ]);
  });
});

describe("blockquotes (P4)", () => {
  it("reads a one-line quote", () => {
    const blocks = parseProse("> the alpaca is not a llama");
    expect(blocks.map((block) => block.kind)).toEqual(["quote"]);
    const quote = blocks[0];
    if (quote?.kind !== "quote") throw new Error("expected a quote");
    expect(quote.segments.map((segment) => segment.text).join("")).toBe(
      "the alpaca is not a llama",
    );
  });

  it("joins a run of quoted lines into one block", () => {
    const blocks = parseProse("> one\n> two\n\nafter");
    expect(blocks.map((block) => block.kind)).toEqual(["quote", "paragraph"]);
    const quote = blocks[0];
    if (quote?.kind !== "quote") throw new Error("expected a quote");
    expect(quote.segments.map((segment) => segment.text).join("")).toBe(
      "one\ntwo",
    );
    expect(paragraphText(blocks[1])).toBe("after");
  });

  it("carries the inline set inside a quote", () => {
    const blocks = parseProse("> **note:** run `cargo test`");
    const quote = blocks[0];
    if (quote?.kind !== "quote") throw new Error("expected a quote");
    expect(quote.segments).toEqual([
      { text: "note:", code: false, strong: true },
      { text: " run ", code: false },
      { text: "cargo test", code: true },
    ]);
  });

  it("keeps a deeper marker as the quote's own text — one level only", () => {
    const blocks = parseProse("> > inner");
    const quote = blocks[0];
    if (quote?.kind !== "quote") throw new Error("expected a quote");
    expect(quote.segments.map((segment) => segment.text).join("")).toBe(
      "> inner",
    );
  });

  it("ends the quote at the first line without the marker", () => {
    const blocks = parseProse("> quoted\nnot quoted");
    expect(blocks.map((block) => block.kind)).toEqual(["quote", "paragraph"]);
    expect(paragraphText(blocks[1])).toBe("not quoted");
  });

  it("closes a paragraph and a list before it starts", () => {
    const blocks = parseProse("intro\n- a\n> quoted");
    expect(blocks.map((block) => block.kind)).toEqual([
      "paragraph",
      "list",
      "quote",
    ]);
  });

  /** Streaming: the marker has arrived and nothing after it has. */
  it("renders a quote that is still empty", () => {
    const blocks = parseProse(">");
    const quote = blocks[0];
    if (quote?.kind !== "quote") throw new Error("expected a quote");
    expect(quote.segments.map((segment) => segment.text).join("")).toBe("");
  });

  it("is not confused by a greater-than sign inside a sentence", () => {
    const blocks = parseProse("2 > 1 is true");
    expect(blocks.map((block) => block.kind)).toEqual(["paragraph"]);
  });
});
