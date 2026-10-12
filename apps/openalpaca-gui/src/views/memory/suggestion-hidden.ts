/**
 * How many hidden characters one suggestion holds, as every surface that
 * decides on it counts them — this view's Suggestions card, `openalpaca
 * memory suggestions` and the chat preview's last line — over the five
 * fields as the daemon serves them: the two titles, the reason and every
 * value its `changes` show (from, then to; the L8 close-out's A4) with the
 * one-line escape (a tab or a line break in them counts), the replaced
 * note's excerpt and the suggestion's knowledge with the arguments' escape.
 *
 * The GUI's copy of `openalpaca_core::memory::service::suggestion_hidden`;
 * both are held to `lib/hidden-chars.json`'s `suggestion_cases`, so the
 * three surfaces read one number.
 */

import type { MemorySuggestion } from "@/lib/api/memories";
import { escapeHidden, escapeHiddenLine } from "@/lib/hidden-chars";

export type SuggestionFields = Pick<
  MemorySuggestion,
  "title" | "reason" | "knowledge"
> & {
  changes: readonly { from: readonly string[]; to: readonly string[] }[];
  target: Pick<MemorySuggestion["target"], "title" | "excerpt">;
};

export function suggestionHidden(item: SuggestionFields): number {
  const changed = item.changes
    .flatMap((c) => [...c.from, ...c.to])
    .reduce((sum, value) => sum + escapeHiddenLine(value).hidden, 0);
  return (
    escapeHiddenLine(item.target.title).hidden +
    escapeHiddenLine(item.title).hidden +
    escapeHiddenLine(item.reason ?? "").hidden +
    changed +
    escapeHidden(item.target.excerpt).hidden +
    escapeHidden(item.knowledge).hidden
  );
}
