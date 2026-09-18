import { describe, expect, it } from "vitest";

import { parseInlineCode, parseProse, type ProseBlock } from "./prose";

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
