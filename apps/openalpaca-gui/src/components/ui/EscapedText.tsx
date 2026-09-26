/**
 * Text a tool, a model or a third party wrote, as an approval surface shows
 * it: what `lib/hidden-chars` escaped, with each `<U+XXXX>` set apart in a
 * styled span. The span is what tells a real escape from the eight
 * characters `<U+202E>` typed into a value, which are drawn as the plain
 * text they are — on the chat connectors and at the CLI only the notice's
 * count can tell the two apart.
 *
 * Plain text either way: every piece is a React text child, never HTML.
 */

import type { Escaped } from "@/lib/hidden-chars";

export interface EscapedTextProps {
  /** From `escapeHidden` (arguments) or `escapeHiddenLine` (a name). */
  escaped: Escaped;
}

export function EscapedText({ escaped }: EscapedTextProps) {
  return (
    <>
      {escaped.pieces.map((piece, index) =>
        piece.kind === "text" ? (
          piece.text
        ) : (
          <span
            key={index}
            data-hidden-char={piece.text.slice(1, -1)}
            className="rounded-xs bg-red-tint px-[2px] font-mono whitespace-nowrap text-red-ink"
          >
            {piece.text}
          </span>
        ),
      )}
    </>
  );
}
