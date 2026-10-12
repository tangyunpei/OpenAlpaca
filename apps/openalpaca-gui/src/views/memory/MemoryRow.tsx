/**
 * One row of the Memory list — a note, or a search hit with its excerpt.
 *
 * The row is the Library row's shape (§3.30): a title line and a mono subline,
 * the raised treatment when selected. Its model is built by two pure
 * functions, one per source, so a list item and a search hit cannot drift into
 * describing the same note two ways.
 */

import { Tag } from "@/components/ui";
import type { MemoryListItem, MemorySearchResult } from "@/lib/api/memories";
import { cn } from "@/lib/cn";
import { relativeTime } from "@/views/library/format";

import { originLabel, statusTag } from "./copy";

export interface MemoryRowModel {
  id: string;
  title: string;
  /** `kind · who wrote it · when` — any part may be absent. */
  subtitle: string;
  /** The matching lines of a search hit, as plain text. */
  excerpt: string | null;
  /** Short words worth a tag: a derived status, `conflict`. */
  tags: string[];
}

export function listRow(
  item: MemoryListItem,
  now: Date = new Date(),
): MemoryRowModel {
  return {
    id: item.id,
    title: item.title,
    subtitle: [
      item.kind,
      originLabel(item.origin),
      relativeTime(item.updated_at, now),
    ]
      .filter((part) => part !== "")
      .join(" · "),
    excerpt: null,
    tags: [statusTag(item.effective_status)].filter(
      (tag): tag is string => tag !== null,
    ),
  };
}

export function searchRow(result: MemorySearchResult): MemoryRowModel {
  const tags = [statusTag(result.effective_status)].filter(
    (tag): tag is string => tag !== null,
  );
  if (result.conflict) tags.push("conflict");
  return {
    id: result.id,
    title: result.title,
    subtitle: [result.kind, originLabel(result.origin), result.heading ?? ""]
      .filter((part) => part !== "")
      .join(" · "),
    excerpt: result.excerpt.trim() === "" ? null : result.excerpt.trim(),
    tags,
  };
}

export interface MemoryRowProps {
  row: MemoryRowModel;
  active: boolean;
  onSelect: (id: string) => void;
}

export function MemoryRow({ row, active, onSelect }: MemoryRowProps) {
  return (
    <div
      className={cn(
        "rounded-xl px-[10px] py-[9px]",
        active
          ? "border border-line-popover bg-raised"
          : "border border-transparent bg-transparent hover:bg-muted-2",
      )}
    >
      <span className="flex items-center gap-[6px]">
        <button
          type="button"
          aria-current={active ? "true" : undefined}
          onClick={() => onSelect(row.id)}
          className="min-w-0 flex-1 cursor-pointer truncate border-none bg-transparent p-0 text-left text-base-plus font-medium text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue"
        >
          {row.title}
        </button>
        {row.tags.map((tag) => (
          <Tag
            key={tag}
            value={tag}
            tone={tag === "conflict" ? "warn" : "neutral"}
          />
        ))}
      </span>
      {row.excerpt !== null && (
        <span className="mt-[3px] line-clamp-2 block text-sm leading-[1.5] text-secondary">
          {row.excerpt}
        </span>
      )}
      <span className="mt-[3px] block truncate font-mono text-2xs-plus text-muted-fg">
        {row.subtitle}
      </span>
    </div>
  );
}
