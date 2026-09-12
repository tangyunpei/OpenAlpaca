/**
 * Message-body text → paragraphs with inline code spans (DESIGN_SPEC §3.10).
 *
 * The design's message body is a `<p>` of prose that may contain inline code
 * (`font-family:'IBM Plex Mono'; font-size:13px; background:#E7E3D9`). That is
 * the whole markup vocabulary of a chat row — no headings, no lists, no links —
 * so this is a deliberate two-rule parser rather than a markdown pipeline:
 * a full renderer would introduce sanitisation surface the transcript does not
 * need, and the artifact renderers (§3.25) are where rich markdown belongs.
 *
 * Rules:
 *   * a blank line separates paragraphs;
 *   * backtick pairs mark inline code; an unpaired backtick is literal text.
 */

export interface ProseSegment {
  text: string;
  code: boolean;
}

export interface ProseBlock {
  /** Stable within one parse — used as the React key. */
  key: number;
  segments: ProseSegment[];
}

/** Split one paragraph into alternating text / inline-code segments. */
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
      segments.push({ text: text.slice(index, open), code: false });
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
    segments.push({ text: text.slice(index), code: false });
  }
  return segments;
}

/** Parse a whole message body into renderable paragraphs. */
export function parseProse(text: string): ProseBlock[] {
  const paragraphs = text.split(/\n{2,}/);
  const blocks: ProseBlock[] = [];
  for (const [index, paragraph] of paragraphs.entries()) {
    const trimmed = paragraph.replace(/\s+$/, "");
    if (trimmed.trim() === "") continue;
    blocks.push({ key: index, segments: parseInlineCode(trimmed) });
  }
  return blocks;
}
