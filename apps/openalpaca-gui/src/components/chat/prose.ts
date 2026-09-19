/**
 * Message-body text → the markdown a model actually writes (DESIGN_SPEC §3.10).
 *
 * The design's message body is a `<p>` of prose that may contain inline code.
 * That was taken literally, and the transcript rendered everything else
 * verbatim — until a completion report arrived written the way models write
 * them, and the chat showed `- **Research:** three connectors are stale.`
 * asterisks and all, beside a Library preview that rendered the same bytes
 * properly (G8). A live session on a local model then showed the rest of the
 * gap (T3): a skill answer's fenced code block as stray backticks, a table as
 * raw `| a | b |` lines, `## Heading` as a literal hash, `---` as three
 * dashes, and an ordered list that restarted at "1." after a code block.
 *
 * This is still **not** a markdown pipeline, and it deliberately takes no
 * dependency: it is a small parser over a fixed vocabulary, producing
 * structure the renderer maps to elements. There is no HTML anywhere in it and
 * therefore no sanitisation surface — `marked` + DOMPurify remain the artifact
 * renderers' business (§3.25), where links, images and raw HTML belong.
 *
 * The vocabulary:
 *   * a fenced block (```` ``` ```` or `~~~`, three or more) is code, shown as
 *     written, with its info string as the language label;
 *   * `#`…`####` at the head of a line is a heading;
 *   * `---`, `***` or `___` alone on a line is a thematic break;
 *   * a `|`-delimited row followed by a `|---|---|` row is a table;
 *   * a run of `> ` lines is a blockquote — one level, no nesting;
 *   * a blank line separates paragraphs;
 *   * a run of `- `, `* `, `+ ` or `1. ` lines is a list, ordered by its first
 *     marker and **starting at that marker's number**, so a list resumed after
 *     a code block counts on from where it left off;
 *   * backtick pairs are inline code, and an unpaired backtick is literal;
 *   * `**bold**` and `*italic*` are emphasis, and either may wrap a code span
 *     (``**`alpaca-fiber-notes`**``, which models write constantly).
 *
 * It parses **incrementally, and never throws**, because it runs on every
 * delta of a streaming answer: a fence that has been opened and not yet closed
 * is a code block already (so the backticks are never shown), and a table
 * header whose delimiter row has not arrived is still a paragraph (so a
 * half-received table reads as the text it currently is).
 *
 * Two deliberate omissions. `_underscores_` are **not** emphasis: this is a
 * window onto a machine where `task_id` and `default_max_tokens` are ordinary
 * words, and italicising the middle of an identifier is worse than printing an
 * underscore. And an emphasis delimiter must hug its text (`*a*`, never `* a`),
 * which is what keeps a line of bullets from reading as one long italic.
 */

export interface ProseSegment {
  text: string;
  code: boolean;
  /** `**bold**`. Absent rather than `false` — most segments are neither. */
  strong?: boolean;
  /** `*italic*`. */
  em?: boolean;
}

/** `#` … `####`. Deeper hashes are a paragraph: a chat row has no h5. */
export type HeadingLevel = 1 | 2 | 3 | 4;

export type ProseBlock =
  | { kind: "paragraph"; key: number; segments: ProseSegment[] }
  | {
      kind: "list";
      key: number;
      /** `1.` / `1)` rather than a bullet. */
      ordered: boolean;
      /**
       * The first marker's number, for an ordered list. `1` for a bullet list
       * and for an ordered one that starts there — but a list resumed after an
       * interleaved block says so, and renders `<ol start>` (T3).
       */
      start: number;
      items: ProseSegment[][];
    }
  | {
      kind: "heading";
      key: number;
      level: HeadingLevel;
      segments: ProseSegment[];
    }
  | {
      kind: "code";
      key: number;
      /** The fence's info string, lower-cased, or `null` when it had none. */
      language: string | null;
      /** The block's lines, exactly as written. Never parsed for inline spans. */
      text: string;
      /** The closing fence has not arrived — the answer is still streaming. */
      open: boolean;
    }
  | { kind: "rule"; key: number }
  | {
      kind: "quote";
      key: number;
      /**
       * The quoted lines, joined with `\n` and parsed for the inline set. One
       * level only: a `>` inside a quote is part of its text, not a nesting.
       */
      segments: ProseSegment[];
    }
  | {
      kind: "table";
      key: number;
      /** The header row's cells. */
      header: ProseSegment[][];
      /** Body rows, each padded or trimmed to the header's width. */
      rows: ProseSegment[][][];
    };

/** `- item`, `* item`, `+ item`. */
const BULLET = /^\s*[-*+]\s+(.*)$/;
/** `1. item`, `1) item`. */
const NUMBERED = /^\s*(\d+)[.)]\s+(.*)$/;
/** ` ```rust ` / `~~~` — three or more of one char, with an optional info string. */
const FENCE = /^\s{0,3}(`{3,}|~{3,})\s*([A-Za-z0-9_+#.-]*)\s*$/;
/** `## Heading`. */
const HEADING = /^\s{0,3}(#{1,4})\s+(.*?)\s*#*\s*$/;
/** `---`, `***`, `___`. */
const RULE = /^\s{0,3}(-{3,}|\*{3,}|_{3,})\s*$/;
/** `> quoted` — the one optional space after the marker is the marker's. */
const QUOTE = /^\s{0,3}>\s?(.*)$/;
/** `|---|:--:|` — the row that turns the line above it into a table header. */
const TABLE_DELIMITER = /^\s*\|?(\s*:?-+:?\s*\|)+(\s*:?-+:?\s*)?\|?\s*$/;

/**
 * `**bold**` and `*italic*`, each hugging its text.
 *
 * `\S(?:[^*]*\S)?` is the hug: no leading or trailing space inside the
 * delimiters, so `* one` at the head of a line is a bullet the block parser
 * has already taken, never the start of an emphasis run that swallows the
 * paragraph.
 */
const EMPHASIS = /\*\*(\S(?:[^*]*\S)?)\*\*|\*(\S(?:[^*]*\S)?)\*/g;

/** What an enclosing emphasis run puts on every segment inside it. */
type Emphasis = Pick<ProseSegment, "strong" | "em">;

/**
 * Split a run into code and plain segments, carrying `style` onto both.
 *
 * Code wins over emphasis *inside* it: the whole point of a backtick span is
 * that what is inside it is shown as written, so `` `a * b` `` is not italic
 * anything. It does not win over an emphasis run that **wraps** it — see
 * `parseInlineCode`.
 */
function splitCode(text: string, style: Emphasis): ProseSegment[] {
  const segments: ProseSegment[] = [];
  let index = 0;

  while (index < text.length) {
    const open = text.indexOf("`", index);
    if (open < 0) break;
    const close = text.indexOf("`", open + 1);
    // An unpaired backtick is just a character.
    if (close < 0) break;

    if (open > index) {
      segments.push({ text: text.slice(index, open), code: false, ...style });
    }
    const code = text.slice(open + 1, close);
    // "``" is an empty span; render the literal backticks instead.
    if (code === "") {
      segments.push({ text: "``", code: false, ...style });
    } else {
      segments.push({ text: code, code: true, ...style });
    }
    index = close + 1;
  }

  if (index < text.length) {
    segments.push({ text: text.slice(index), code: false, ...style });
  }
  return segments;
}

/**
 * The text with every code span — backticks and all — blanked out, in place.
 *
 * Same length, so every index into it is an index into the original. It exists
 * so emphasis can be matched *around* code spans without ever being matched
 * *inside* one: a `*` in `` `a * b` `` is gone from the masked copy, and a
 * ``**`x`**`` still reads as bold because the run's own delimiters are not.
 *
 * The scan is `splitCode`'s, pair for pair, so the two agree on what a span
 * is — an empty ``` `` ``` is literal in both and is left alone here.
 */
function maskCodeSpans(text: string): string {
  let masked = "";
  let index = 0;

  while (index < text.length) {
    const open = text.indexOf("`", index);
    if (open < 0) break;
    const close = text.indexOf("`", open + 1);
    if (close < 0) break;

    masked += text.slice(index, open);
    const span = text.slice(open, close + 1);
    masked += close === open + 1 ? span : "\0".repeat(span.length);
    index = close + 1;
  }

  return masked + text.slice(index);
}

/**
 * Split one paragraph into code, emphasis and plain segments.
 *
 * Emphasis is matched over a copy with the code spans blanked out, so a run
 * may **wrap** one (``**`alpaca-fiber-notes`**`` was rendering with its
 * asterisks showing, P4) while an asterisk *inside* a span still cannot open
 * one. The delimiters themselves are always real characters — a match can
 * therefore never begin or end in the middle of a code span, which is what
 * makes it safe to re-split each piece on the original text.
 */
export function parseInlineCode(text: string): ProseSegment[] {
  const masked = maskCodeSpans(text);
  const segments: ProseSegment[] = [];
  let index = 0;

  EMPHASIS.lastIndex = 0;
  let match = EMPHASIS.exec(masked);
  while (match !== null) {
    if (match.index > index) {
      segments.push(...splitCode(text.slice(index, match.index), {}));
    }
    const strong = match[1];
    const inner =
      strong === undefined
        ? { start: match.index + 1, length: (match[2] ?? "").length }
        : { start: match.index + 2, length: strong.length };
    segments.push(
      ...splitCode(
        text.slice(inner.start, inner.start + inner.length),
        strong === undefined ? { em: true } : { strong: true },
      ),
    );
    index = match.index + match[0].length;
    match = EMPHASIS.exec(masked);
  }

  if (index < text.length) {
    segments.push(...splitCode(text.slice(index), {}));
  }
  return segments;
}

/** The list item this line is, or `null` when it is prose. */
function listItem(
  line: string,
): { ordered: boolean; start: number; text: string } | null {
  const numbered = NUMBERED.exec(line);
  if (numbered !== null) {
    return {
      ordered: true,
      start: Number.parseInt(numbered[1] ?? "1", 10) || 1,
      text: numbered[2] ?? "",
    };
  }
  const bullet = BULLET.exec(line);
  if (bullet !== null)
    return { ordered: false, start: 1, text: bullet[1] ?? "" };
  return null;
}

/**
 * One table row's cells.
 *
 * A `\|` is a literal pipe, not a separator — otherwise a cell can never hold
 * one. The outer empties a leading or trailing `|` produces are dropped; an
 * interior empty cell is kept, because it is a cell.
 */
export function splitTableRow(line: string): string[] {
  const cells: string[] = [];
  let cell = "";
  for (let index = 0; index < line.length; index += 1) {
    const char = line[index];
    if (char === "\\" && line[index + 1] === "|") {
      cell += "|";
      index += 1;
      continue;
    }
    if (char === "|") {
      cells.push(cell);
      cell = "";
      continue;
    }
    cell += char;
  }
  cells.push(cell);
  if (cells.length > 0 && (cells[0] ?? "").trim() === "") cells.shift();
  if (cells.length > 0 && (cells.at(-1) ?? "").trim() === "") cells.pop();
  return cells.map((text) => text.trim());
}

/** A line that could be a table row — it has an unescaped `|`. */
function looksLikeTableRow(line: string): boolean {
  return /(^|[^\\])\|/.test(line);
}

/**
 * The same markdown, reduced to its words (P3).
 *
 * For the surfaces that are a *summary* rather than a body — DESIGN_SPEC
 * §3.12 draws the run-report card as one paragraph — where rendering the
 * markup would be wrong and printing it raw (`**Basics**`, `## Summary`,
 * backticks) is what the owner saw. It is the parser's own vocabulary
 * flattened, not a regex strip, so it strips exactly what the transcript
 * renders and nothing else: an asterisk that was never emphasis survives.
 *
 * Blocks keep their line breaks — a 500-character run summary is regularly
 * three short sections — and the caller renders with `whitespace-pre-line`.
 */
export function plainText(markdown: string): string {
  const lines: string[] = [];
  for (const block of parseProse(markdown)) {
    switch (block.kind) {
      case "paragraph":
      case "heading":
      case "quote":
        lines.push(segmentText(block.segments));
        break;
      case "list":
        for (const item of block.items) lines.push(segmentText(item));
        break;
      case "code":
        lines.push(block.text);
        break;
      case "table":
        for (const row of [block.header, ...block.rows]) {
          lines.push(row.map(segmentText).join(" · "));
        }
        break;
      case "rule":
        // A rule is punctuation, and punctuation with no text is nothing.
        break;
      default: {
        // Exhaustive by construction: a block kind added to `parseProse` and
        // not to this switch is a type error here, rather than content the
        // run-report card silently drops. Nothing throws — this file never
        // does, and it is unreachable anyway.
        const unhandled: never = block;
        void unhandled;
      }
    }
  }
  return lines
    .map((line) => line.trim())
    .filter((line) => line !== "")
    .join("\n");
}

function segmentText(segments: readonly ProseSegment[]): string {
  return segments.map((segment) => segment.text).join("");
}

/** Parse a whole message body into renderable blocks. */
export function parseProse(text: string): ProseBlock[] {
  const blocks: ProseBlock[] = [];
  let key = 0;

  let paragraph: string[] = [];
  let items: string[] = [];
  let ordered = false;
  let listStart = 1;

  const flushParagraph = (): void => {
    const body = paragraph.join("\n").replace(/\s+$/, "");
    paragraph = [];
    if (body.trim() === "") return;
    blocks.push({
      kind: "paragraph",
      key: key++,
      segments: parseInlineCode(body),
    });
  };
  const flushList = (): void => {
    if (items.length === 0) return;
    const body = items;
    items = [];
    blocks.push({
      kind: "list",
      key: key++,
      ordered,
      start: listStart,
      items: body.map((item) => parseInlineCode(item)),
    });
  };
  const flushAll = (): void => {
    flushList();
    flushParagraph();
  };

  const lines = text.split("\n");
  let index = 0;

  while (index < lines.length) {
    const line = lines[index] ?? "";

    // ── fenced code ────────────────────────────────────────────────────────
    const fence = FENCE.exec(line);
    if (fence !== null) {
      flushAll();
      const marker = fence[1] ?? "```";
      const language = (fence[2] ?? "").toLowerCase();
      const body: string[] = [];
      let closed = false;
      index += 1;
      while (index < lines.length) {
        const inner = lines[index] ?? "";
        const closing = FENCE.exec(inner);
        // A closing fence is the same character, at least as long, and bare.
        if (
          closing !== null &&
          (closing[2] ?? "") === "" &&
          (closing[1] ?? "").startsWith(marker[0] ?? "`") &&
          (closing[1] ?? "").length >= marker.length
        ) {
          closed = true;
          index += 1;
          break;
        }
        body.push(inner);
        index += 1;
      }
      blocks.push({
        kind: "code",
        key: key++,
        language: language === "" ? null : language,
        text: body.join("\n"),
        // Still streaming: the answer stops mid-block and the closing fence is
        // simply not here yet. Rendered as code all the same — showing the
        // opening backticks and then swapping them for a block is the flicker
        // T3 is about.
        open: !closed,
      });
      continue;
    }

    // ── table ──────────────────────────────────────────────────────────────
    const next = lines[index + 1];
    if (
      looksLikeTableRow(line) &&
      next !== undefined &&
      TABLE_DELIMITER.test(next)
    ) {
      flushAll();
      const header = splitTableRow(line);
      index += 2;
      const rows: string[][] = [];
      while (index < lines.length) {
        const row = lines[index] ?? "";
        if (row.trim() === "" || !looksLikeTableRow(row)) break;
        const cells = splitTableRow(row);
        // Padded, not dropped: a row the model cut short is still a row, and a
        // ragged `rows[i][j]` would be the renderer's problem instead.
        while (cells.length < header.length) cells.push("");
        rows.push(cells.slice(0, header.length));
        index += 1;
      }
      blocks.push({
        kind: "table",
        key: key++,
        header: header.map((cell) => parseInlineCode(cell)),
        rows: rows.map((row) => row.map((cell) => parseInlineCode(cell))),
      });
      continue;
    }

    // ── heading ────────────────────────────────────────────────────────────
    const heading = HEADING.exec(line);
    if (heading !== null) {
      flushAll();
      blocks.push({
        kind: "heading",
        key: key++,
        level: (heading[1] ?? "#")
          .length as HeadingLevel satisfies HeadingLevel,
        segments: parseInlineCode(heading[2] ?? ""),
      });
      index += 1;
      continue;
    }

    // ── thematic break ─────────────────────────────────────────────────────
    // After the list check would be wrong (`- - -` is not this), but before it
    // is right: `---` matches no list marker, because a bullet needs a space
    // and some text after it.
    if (RULE.test(line)) {
      flushAll();
      blocks.push({ kind: "rule", key: key++ });
      index += 1;
      continue;
    }

    // ── blockquote ─────────────────────────────────────────────────────────
    // One level, and a run of `>` lines is one quote: a model writing a quoted
    // paragraph writes several. A line without the marker ends it, so the
    // block never swallows the answer that follows — and a quote still being
    // streamed is simply a shorter quote.
    const quote = QUOTE.exec(line);
    if (quote !== null) {
      flushAll();
      const quoted: string[] = [quote[1] ?? ""];
      index += 1;
      while (index < lines.length) {
        const more = QUOTE.exec(lines[index] ?? "");
        if (more === null) break;
        quoted.push(more[1] ?? "");
        index += 1;
      }
      blocks.push({
        kind: "quote",
        key: key++,
        segments: parseInlineCode(quoted.join("\n").replace(/\s+$/, "")),
      });
      continue;
    }

    // ── blank line ─────────────────────────────────────────────────────────
    if (line.trim() === "") {
      flushAll();
      index += 1;
      continue;
    }

    // ── list / prose ───────────────────────────────────────────────────────
    // A run of prose lines, and a run of list items, each flushed when the
    // other starts — a report's "Findings:" line and the bullets under it are
    // one paragraph to a blank-line splitter and two blocks here.
    const item = listItem(line);
    if (item === null) {
      flushList();
      paragraph.push(line);
      index += 1;
      continue;
    }
    if (items.length === 0) {
      ordered = item.ordered;
      // T3: a list that resumes after a code block counts on from its own
      // first marker, instead of every run restarting at 1.
      listStart = item.start;
    }
    flushParagraph();
    items.push(item.text);
    index += 1;
  }

  flushAll();
  return blocks;
}
