import { describe, expect, it } from "vitest";

import table from "./hidden-chars.json";
import {
  escapeHidden,
  escapeHiddenLine,
  hiddenNotice,
  hiddenOnCard,
  needsEscape,
  needsEscapeInLine,
} from "./hidden-chars";

/**
 * The shared table's ranges, parsed here on their own: the module's parse,
 * merge and search are what is under test, so the reference must not be
 * built by them.
 */
function bounds(ranges: readonly (readonly string[])[]): [number, number][] {
  return ranges.map((pair) => {
    const [first, last] = pair;
    if (first === undefined || last === undefined) {
      throw new Error(`not a [first, last] pair: ${JSON.stringify(pair)}`);
    }
    return [parseInt(first, 16), parseInt(last, 16)];
  });
}

function inAny(ranges: readonly [number, number][], codePoint: number) {
  return ranges.some(
    ([first, last]) => first <= codePoint && codePoint <= last,
  );
}

/** Every Unicode scalar value: every code point but the surrogates. */
function* scalarValues(): Generator<number> {
  for (let codePoint = 0; codePoint <= 0x10ffff; codePoint += 1) {
    if (codePoint >= 0xd800 && codePoint <= 0xdfff) continue;
    yield codePoint;
  }
}

const hex = (codePoint: number) =>
  `U+${codePoint.toString(16).toUpperCase().padStart(4, "0")}`;

/**
 * Parity with the Rust side. `hidden-chars.json` is the one table:
 * `openalpaca_core::utils::text`'s tests hold `needs_escape`,
 * `needs_escape_in_line`, `escape_hidden`, `escape_hidden_line` and
 * `hidden_notice` to it, and these hold the GUI's copy to the same file —
 * so a drift on either side fails a test on that side.
 */
describe("hidden-chars — the shared table (parity with Rust)", () => {
  it("escapes in the arguments exactly the table's ranges, over every scalar value", () => {
    const ranges = bounds(table.ranges);
    const wrong: string[] = [];
    for (const codePoint of scalarValues()) {
      if (needsEscape(codePoint) !== inAny(ranges, codePoint)) {
        wrong.push(hex(codePoint));
        if (wrong.length > 8) break;
      }
    }
    expect(wrong).toEqual([]);
  });

  it("escapes in a one-line field exactly ranges plus line_adds, over every scalar value", () => {
    const ranges = [...bounds(table.ranges), ...bounds(table.line_adds)];
    const wrong: string[] = [];
    for (const codePoint of scalarValues()) {
      if (needsEscapeInLine(codePoint) !== inAny(ranges, codePoint)) {
        wrong.push(hex(codePoint));
        if (wrong.length > 8) break;
      }
    }
    expect(wrong).toEqual([]);
  });

  it("shows every one of the table's cases as Rust shows it", () => {
    expect(table.cases.length).toBeGreaterThan(0);
    for (const { input, shown, hidden } of table.cases) {
      const escaped = escapeHidden(input);
      expect({ input, shown: escaped.text, hidden: escaped.hidden }).toEqual({
        input,
        shown,
        hidden,
      });
    }
  });

  it("shows every one of the table's line cases as Rust shows it", () => {
    expect(table.line_cases.length).toBeGreaterThan(0);
    for (const { input, shown, hidden } of table.line_cases) {
      const escaped = escapeHiddenLine(input);
      expect({ input, shown: escaped.text, hidden: escaped.hidden }).toEqual({
        input,
        shown,
        hidden,
      });
    }
  });

  it("says the table's notice, word for word, and none when nothing was escaped", () => {
    expect(table.notices.length).toBeGreaterThan(0);
    for (const { hidden, text } of table.notices) {
      expect(hiddenNotice(hidden)).toBe(text);
    }
    expect(hiddenNotice(7)).toBe(
      "Note: this request contains 7 hidden characters, shown as <U+…>.",
    );
  });
});

describe("hidden-chars — what an approval surface shows", () => {
  it("shows an override, a zero-width space, a C1 control, a CR and a tag by their code points", () => {
    const escaped = escapeHidden(
      "\u{202E}gpj.exe /srv/allow\u{200B}ed a\u{9B}31m done\r tag\u{E0041}",
    );
    expect(escaped.text).toBe(
      "<U+202E>gpj.exe /srv/allow<U+200B>ed a<U+009B>31m done<U+000D> tag<U+E0041>",
    );
    expect(escaped.hidden).toBe(5);
  });

  it("makes each escape a piece of its own, carrying its code point", () => {
    const escaped = escapeHidden("rm \u{202E}fdp\u{200B}");
    expect(escaped.pieces).toEqual([
      { kind: "text", text: "rm " },
      { kind: "escape", text: "<U+202E>", codePoint: 0x202e },
      { kind: "text", text: "fdp" },
      { kind: "escape", text: "<U+200B>", codePoint: 0x200b },
    ]);
    expect(escaped.pieces.map((piece) => piece.text).join("")).toBe(
      escaped.text,
    );
  });

  it("leaves text with nothing hidden as it is, right-to-left letters included", () => {
    const escaped = escapeHidden("日本語 שלום ls -la");
    expect(escaped).toEqual({
      pieces: [{ kind: "text", text: "日本語 שלום ls -la" }],
      text: "日本語 שלום ls -la",
      hidden: 0,
    });
    expect(escapeHidden("")).toEqual({ pieces: [], text: "", hidden: 0 });
  });

  it("keeps pretty-printed JSON's own newlines and indentation, which JSON.stringify does not escape the rest of", () => {
    const pretty = JSON.stringify(
      { c1: "\u{9B}31m", path: "/srv/allow\u{200B}ed", run: "\u{202E}gpj.exe" },
      null,
      2,
    );
    // What the escape is for: JSON escapes C0 inside a string, not these.
    expect(pretty).toContain("\u{202E}");
    expect(pretty).toContain("\u{9B}");

    const escaped = escapeHidden(pretty);
    expect(escaped.text.split("\n")).toEqual([
      "{",
      '  "c1": "<U+009B>31m",',
      '  "path": "/srv/allow<U+200B>ed",',
      '  "run": "<U+202E>gpj.exe"',
      "}",
    ]);
    expect(escaped.hidden).toBe(3);
  });

  /**
   * A surface that shows part of a pending request — the composer's bar, the
   * palette's Approve row, the run card — counts the whole request and points
   * at the card, where every escape is drawn.
   */
  it("points at the card with the whole request's count, and says nothing when nothing was escaped", () => {
    expect(hiddenOnCard(0)).toBeNull();
    expect(hiddenOnCard(1)).toBe("its card shows 1 hidden character");
    expect(hiddenOnCard(4)).toBe("its card shows 4 hidden characters");
  });

  it("shows a newline and a tab in a one-line field, and keeps them as layout in the arguments", () => {
    expect(escapeHiddenLine("read\nArguments:\t{}")).toMatchObject({
      text: "read<U+000A>Arguments:<U+0009>{}",
      hidden: 2,
    });
    expect(escapeHidden("read\nArguments:\t{}")).toMatchObject({
      text: "read\nArguments:\t{}",
      hidden: 0,
    });
  });
});
