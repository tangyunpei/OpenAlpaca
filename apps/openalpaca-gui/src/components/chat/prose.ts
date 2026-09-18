/**
 * Message-body text → paragraphs, lists and inline spans (DESIGN_SPEC §3.10).
 *
 * The design's message body is a `<p>` of prose that may contain inline code.
 * That was taken literally, and the transcript rendered everything else
 * verbatim — until a completion report arrived written the way models write
 * them, and the chat showed `- **Research:** three connectors are stale.`
 * asterisks and all, beside a Library preview that rendered the same bytes
 * properly (G8).
 *
 * This is still **not** a markdown pipeline. It stays a small parser over a
 * fixed vocabulary, producing structure the renderer maps to elements, so
 * there is no HTML anywhere and no sanitisation surface: `marked` + DOMPurify
 * remain the artifact renderers' business (§3.25), where headings, links and
 * tables actually belong.
 *
 * The vocabulary:
 *   * a blank line separates paragraphs;
 *   * a run of `- `, `* `, `+ ` or `1. ` lines is a list, ordered by its first
 *     marker;
 *   * backtick pairs are inline code, and an unpaired backtick is literal;
 *   * `**bold**` and `*italic*` are emphasis.
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

export type ProseBlock =
  | { kind: "paragraph"; key: number; segments: ProseSegment[] }
  | {
      kind: "list";
      key: number;
      /** `1.` / `1)` rather than a bullet. */
      ordered: boolean;
      items: ProseSegment[][];
    };

/** `- item`, `* item`, `+ item`. */
const BULLET = /^\s*[-*+]\s+(.*)$/;
/** `1. item`, `1) item`. */
const NUMBERED = /^\s*\d+[.)]\s+(.*)$/;

/**
 * `**bold**` and `*italic*`, each hugging its text.
 *
 * `\S(?:[^*]*\S)?` is the hug: no leading or trailing space inside the
 * delimiters, so `* one` at the head of a line is a bullet the block parser
 * has already taken, never the start of an emphasis run that swallows the
 * paragraph.
 */
const EMPHASIS = /\*\*(\S(?:[^*]*\S)?)\*\*|\*(\S(?:[^*]*\S)?)\*/g;

/** Split a plain (already code-free) run into emphasis segments. */
function splitEmphasis(text: string): ProseSegment[] {
  const segments: ProseSegment[] = [];
  let index = 0;

  EMPHASIS.lastIndex = 0;
  let match = EMPHASIS.exec(text);
  while (match !== null) {
    if (match.index > index) {
      segments.push({ text: text.slice(index, match.index), code: false });
    }
    const strong = match[1];
    if (strong !== undefined) {
      segments.push({ text: strong, code: false, strong: true });
    } else {
      segments.push({ text: match[2] ?? "", code: false, em: true });
    }
    index = match.index + match[0].length;
    match = EMPHASIS.exec(text);
  }

  if (index < text.length) {
    segments.push({ text: text.slice(index), code: false });
  }
  return segments;
}

/**
 * Split one paragraph into code, emphasis and plain segments.
 *
 * Code wins: the whole point of a backtick span is that what is inside it is
 * shown as written, so `` `a * b` `` is not italic anything.
 */
export function parseInlineCode(text: string): ProseSegment[] {
  const segments: ProseSegment[] = [];
  let index = 0;

  while (index < text.length) {
    const open = text.indexOf("`", index);
    if (open < 0) break;
    const close = text.indexOf("`", open + 1);
    // An unpaired backtick is just a character.
    if (close < 0) break;

    if (open > index) {
      segments.push(...splitEmphasis(text.slice(index, open)));
    }
    const code = text.slice(open + 1, close);
    // "``" is an empty span; render the literal backticks instead.
    if (code === "") {
      segments.push({ text: "``", code: false });
    } else {
      segments.push({ text: code, code: true });
    }
    index = close + 1;
  }

  if (index < text.length) {
    segments.push(...splitEmphasis(text.slice(index)));
  }
  return segments;
}

/** The list item this line is, or `null` when it is prose. */
function listItem(line: string): { ordered: boolean; text: string } | null {
  const numbered = NUMBERED.exec(line);
  if (numbered !== null) return { ordered: true, text: numbered[1] ?? "" };
  const bullet = BULLET.exec(line);
  if (bullet !== null) return { ordered: false, text: bullet[1] ?? "" };
  return null;
}

/** Parse a whole message body into renderable blocks. */
export function parseProse(text: string): ProseBlock[] {
  const blocks: ProseBlock[] = [];
  let key = 0;

  for (const chunk of text.split(/\n{2,}/)) {
    // A run of prose lines, and a run of list items, each flushed when the
    // other starts — a report's "Findings:" line and the bullets under it are
    // one paragraph to the splitter above and two blocks here.
    let paragraph: string[] = [];
    let items: string[] = [];
    let ordered = false;

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
        items: body.map((item) => parseInlineCode(item)),
      });
    };

    for (const line of chunk.split("\n")) {
      const item = listItem(line);
      if (item === null) {
        flushList();
        paragraph.push(line);
        continue;
      }
      if (items.length === 0) ordered = item.ordered;
      flushParagraph();
      items.push(item.text);
    }
    flushList();
    flushParagraph();
  }

  return blocks;
}
