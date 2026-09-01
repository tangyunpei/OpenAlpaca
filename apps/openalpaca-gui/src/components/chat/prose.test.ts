import { describe, expect, it } from "vitest";

import { parseInlineCode, parseProse } from "./prose";

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

describe("parseProse", () => {
  it("splits on blank lines and drops empty paragraphs", () => {
    const blocks = parseProse("one\n\n\ntwo\n\n   \n");
    expect(blocks).toHaveLength(2);
    expect(blocks[0]?.segments[0]?.text).toBe("one");
    expect(blocks[1]?.segments[0]?.text).toBe("two");
  });

  it("keeps single newlines inside one paragraph", () => {
    const blocks = parseProse("one\ntwo");
    expect(blocks).toHaveLength(1);
    expect(blocks[0]?.segments[0]?.text).toBe("one\ntwo");
  });
});
