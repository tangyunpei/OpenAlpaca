/**
 * Hidden characters on the approval surface, shown as `<U+XXXX>`.
 *
 * A confirmation card shows the person who approves or denies a tool call
 * the exact values that will run: the tool's arguments, the tool's name (an
 * MCP `<server>__<tool>` or plugin `<plugin>::<tool>` name is whatever its
 * third party listed) and the agent asking. `JSON.stringify` escapes the C0
 * controls inside a string and passes everything else through, so a bidi
 * override (U+202E), a zero-width space (U+200B), a C1 control or a tag
 * character reaches the card raw — and can make the card read differently
 * from what runs; so does a blank that is not the plain space (a braille
 * blank, a no-break or ideographic space), a run of which wraps into empty
 * lines between what is read and what runs. Every such character is
 * **shown**, never dropped: dropped,
 * `/srv/allow` + U+200B + `ed` would read as a path it is not, and the person
 * would not be told anything was there. `hiddenNotice` says how many.
 *
 * This is the GUI's copy of `openalpaca_core::utils::text`'s escape
 * (`needs_escape`, `escape_hidden`, `needs_escape_in_line`,
 * `escape_hidden_line`, `hidden_notice`), which the chat connectors and the
 * CLI show. One table, `hidden-chars.json` beside this file, serves both
 * languages: the Rust side's predicates are its own, pinned to Unicode, and
 * a test that reads this file holds them equal to every range, case, line
 * case and notice in it; this module derives its sets from the table, and
 * its own test holds it to the same entries.
 *
 * - **The arguments** take `escapeHidden`: `\n` and `\t` stay, because the
 *   pretty-printed JSON is laid out in them, and `JSON.stringify` has already
 *   written out every C0 control character (U+0000–U+001F) inside a string —
 *   the card's bare-string shortcut (`formatToolArguments`: a lone
 *   `command`, `path`, `url`, …) steps aside for a value holding one, so a
 *   value's own line break or tab never reaches the card as layout. DEL and
 *   the C1 controls (U+007F–U+009F), which JSON leaves raw, are escaped here
 *   like any other member of the set.
 * - **A one-line field** — the tool's name, the agent's name — takes
 *   `escapeHiddenLine`, which also shows a newline and a tab: a name has no
 *   layout of its own, and a line break in it would draw lines the card does
 *   not have.
 * - **The daemon-authored `preview`** is not escaped. It is OpenAlpaca's own
 *   prose, composed to be read, and the daemon strips it of control and
 *   format characters before any surface sees it; the values that decide
 *   what runs are the arguments beside it, which this escapes.
 *
 * Display only: nothing sent to the daemon, stored or matched changes — the
 * card, the composer's answer and every POST body read the raw values. The
 * GUI does not cut an approval's text, so there is no cut here: each surface
 * that shows it wraps a run too long for its line anywhere
 * (`overflow-wrap: anywhere`) rather than letting it run past an edge that
 * clips it. Where an escape is drawn as a highlighted span — the card, the
 * composer's bar, the run card, the resolution row — the span is never
 * split across two lines. The palette's Approve row and the Always-allow
 * toast take the plain-text form, which wraps like any other text and can
 * break inside an escape (every character stays on screen); the run's
 * Event log, which is not an approval surface, cuts its one-line rows with
 * an ellipsis.
 */

import table from "./hidden-chars.json";

/** One inclusive `[first, last]` range of code points. */
type Range = readonly [number, number];

/** The table's hex bounds as numbers, refusing anything it should not hold. */
function parseRanges(raw: readonly (readonly string[])[]): Range[] {
  return raw.map((pair) => {
    const [first, last] = pair;
    if (
      pair.length !== 2 ||
      first === undefined ||
      last === undefined ||
      !/^[0-9A-F]{4,6}$/.test(first) ||
      !/^[0-9A-F]{4,6}$/.test(last)
    ) {
      throw new Error(`hidden-chars.json: not a hex range: ${String(pair)}`);
    }
    const range: Range = [parseInt(first, 16), parseInt(last, 16)];
    if (range[0] > range[1]) {
      throw new Error(`hidden-chars.json: an empty range: ${String(pair)}`);
    }
    return range;
  });
}

/** Sorted, with touching and overlapping ranges joined — for `contains`. */
function merged(ranges: readonly Range[]): Range[] {
  const sorted = [...ranges].sort((a, b) => a[0] - b[0]);
  const out: [number, number][] = [];
  for (const [first, last] of sorted) {
    const previous = out[out.length - 1];
    if (previous !== undefined && first <= previous[1] + 1) {
      previous[1] = Math.max(previous[1], last);
    } else {
      out.push([first, last]);
    }
  }
  return out;
}

function contains(ranges: readonly Range[], codePoint: number): boolean {
  let low = 0;
  let high = ranges.length - 1;
  while (low <= high) {
    const middle = (low + high) >> 1;
    const range = ranges[middle];
    if (range === undefined) return false;
    if (codePoint < range[0]) high = middle - 1;
    else if (codePoint > range[1]) low = middle + 1;
    else return true;
  }
  return false;
}

const ARGUMENT_SET = merged(parseRanges(table.ranges));
const LINE_SET = merged([
  ...parseRanges(table.ranges),
  ...parseRanges(table.line_adds),
]);

/**
 * A code point the arguments show as `<U+XXXX>`: Unicode's control, format
 * and default-ignorable code points, every space but the plain one (U+2028
 * and U+2029 among them), U+2800 BRAILLE PATTERN BLANK and U+FFFC (which
 * macOS draws as an empty cell), but not `\n` and `\t` (Rust's
 * `needs_escape`).
 */
export function needsEscape(codePoint: number): boolean {
  return contains(ARGUMENT_SET, codePoint);
}

/**
 * A code point a one-line field shows as `<U+XXXX>`: every `needsEscape`
 * one, and `\n` and `\t` (Rust's `needs_escape_in_line`).
 */
export function needsEscapeInLine(codePoint: number): boolean {
  return contains(LINE_SET, codePoint);
}

/** A run of text as written, or one escaped character. */
export type ShownPiece =
  | { readonly kind: "text"; readonly text: string }
  | {
      readonly kind: "escape";
      /** `<U+202E>`: what is shown in the character's place. */
      readonly text: string;
      readonly codePoint: number;
    };

/** What `escapeHidden` or `escapeHiddenLine` made of a text. */
export interface Escaped {
  /** In order, so a renderer can set the escapes apart from the text. */
  readonly pieces: readonly ShownPiece[];
  /** The pieces joined: the plain-text form, for a surface that takes a string. */
  readonly text: string;
  /** How many characters were escaped — what `hiddenNotice` is given. */
  readonly hidden: number;
}

/** `<U+XXXX>`: uppercase hex, zero-padded to at least four digits. */
function escapeForm(codePoint: number): string {
  return `<U+${codePoint.toString(16).toUpperCase().padStart(4, "0")}>`;
}

function escapeWhere(
  text: string,
  shownEscaped: (codePoint: number) => boolean,
): Escaped {
  const pieces: ShownPiece[] = [];
  let run = "";
  let hidden = 0;
  // By code point: an astral tag character is one escape, not two halves.
  for (const character of text) {
    const codePoint = character.codePointAt(0) ?? 0;
    if (!shownEscaped(codePoint)) {
      run += character;
      continue;
    }
    if (run !== "") pieces.push({ kind: "text", text: run });
    run = "";
    pieces.push({ kind: "escape", text: escapeForm(codePoint), codePoint });
    hidden += 1;
  }
  if (run !== "") pieces.push({ kind: "text", text: run });
  return {
    pieces,
    text: pieces.map((piece) => piece.text).join(""),
    hidden,
  };
}

/**
 * The arguments for a person deciding whether they run: every
 * `needsEscape` character as `<U+XXXX>`, everything else as it is,
 * right-to-left letters included (Rust's `escape_hidden`).
 */
export function escapeHidden(text: string): Escaped {
  return escapeWhere(text, needsEscape);
}

/**
 * A one-line field — a tool's name, an agent's name: every
 * `needsEscapeInLine` character as `<U+XXXX>`, newlines and tabs included
 * (Rust's `escape_hidden_line`).
 */
export function escapeHiddenLine(text: string): Escaped {
  return escapeWhere(text, needsEscapeInLine);
}

/**
 * The one line a card shows directly under what it escaped, or `null` when
 * nothing was: word for word what every other surface prints (Rust's
 * `hidden_notice`, held to the table's `notices` on both sides).
 */
export function hiddenNotice(hidden: number): string | null {
  if (hidden <= 0) return null;
  if (hidden === 1) {
    return "Note: this request contains 1 hidden character, shown as <U+…>.";
  }
  return `Note: this request contains ${hidden} hidden characters, shown as <U+…>.`;
}

/**
 * What a surface that shows part of a pending request says beside it — the
 * composer's bar and the palette's Approve row, which answer the request,
 * and the run card, which sends the person to it — or `null` when nothing
 * was escaped. `hidden` is the whole request's count, the card's own, and
 * the words say where the rest is shown: the card is where every escape is
 * drawn. (The CLI's `tasks confirmations list` does the same under its
 * table, pointing at `--format json`.)
 */
export function hiddenOnCard(hidden: number): string | null {
  if (hidden <= 0) return null;
  return hidden === 1
    ? "its card shows 1 hidden character"
    : `its card shows ${hidden} hidden characters`;
}
