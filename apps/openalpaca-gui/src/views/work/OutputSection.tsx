/**
 * The work detail's `Output` card (DESIGN_SPEC §5.2, §3.27 framed variant).
 *
 * The rows come from `task.outcome.artifacts` — free-form JSON the dispatcher
 * writes, the only link between a run and what it produced. There is no
 * artifact resource (GAP-04): entries carry no stable id, no kind and no
 * content route, so a row states what the run reported and does not pretend to
 * open it. When ids appear, `onOpen` is wired and the rows become buttons.
 */

import {
  FileBadge,
  languageFromName,
  SectionCard,
  SectionEmpty,
} from "@/components/ui";
import type { OutcomeArtifact } from "@/components/work/run-model";
import { cn } from "@/lib/cn";
import { GAPS, gapNote } from "@/lib/unavailable";

/** The design's own empty sentence for this card. */
export const OUTPUT_EMPTY =
  "Nothing produced yet. Files land here and in the Library as the run works.";

const OUTPUT_NOTE = `${gapNote(GAPS["GAP-04"])} — ${GAPS["GAP-04"].missingApi}. Proposed: ${GAPS["GAP-04"].proposedEndpoint}`;

export interface OutputSectionProps {
  artifacts: readonly OutcomeArtifact[];
  /** `task.artifact_count`, which can exceed the number of readable entries. */
  count: number;
  /** Present only once artifacts have ids to open. */
  onOpen?: (artifact: OutcomeArtifact) => void;
}

export function OutputSection({
  artifacts,
  count,
  onOpen,
}: OutputSectionProps) {
  if (artifacts.length === 0) {
    return (
      <SectionCard title="Output">
        <SectionEmpty note={count > 0 ? OUTPUT_NOTE : undefined}>
          {count > 0
            ? `This run reported ${count} file${count === 1 ? "" : "s"}, but none of them can be listed.`
            : OUTPUT_EMPTY}
        </SectionEmpty>
      </SectionCard>
    );
  }

  const rowClass =
    "flex w-full items-center gap-[10px] border-b border-line-hair-2 px-[16px] py-[11px] text-left last:border-b-0";

  return (
    <SectionCard title="Output">
      <div>
        {artifacts.map((artifact) => {
          const inner = (
            <>
              <FileBadge
                kind={artifact.kind}
                size={17}
                language={languageFromName(artifact.name)}
              />
              <span className="min-w-0 flex-1 truncate text-base-plus text-ink">
                {artifact.name}
              </span>
              {artifact.stamp !== null && (
                <span className="shrink-0 font-mono text-xs text-muted-fg">
                  {artifact.stamp}
                </span>
              )}
            </>
          );

          if (onOpen === undefined || artifact.id === null) {
            return (
              <div key={artifact.name} className={rowClass}>
                {inner}
              </div>
            );
          }
          return (
            <button
              key={artifact.id}
              type="button"
              onClick={() => onOpen(artifact)}
              className={cn(
                rowClass,
                "cursor-pointer border-none bg-transparent hover:bg-sunken",
                "focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-blue",
              )}
            >
              {inner}
            </button>
          );
        })}
      </div>
      <p className="m-0 px-[16px] pt-[8px] pb-[12px] font-mono text-2xs-plus leading-[1.5] text-faint">
        {OUTPUT_NOTE}
      </p>
    </SectionCard>
  );
}
