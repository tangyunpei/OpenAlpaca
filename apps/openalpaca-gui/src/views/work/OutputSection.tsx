/**
 * The work detail's `Output` card (DESIGN_SPEC §5.2, §3.27 framed variant),
 * and under it, when the run wrote into its project, **Written in the
 * project** (storage design §13, R38).
 *
 * The rows are `GET /v1/artifacts?task_id=` — this run's files, with real ids,
 * kinds and stamps, so every row opens. `Output` lists the run's by-products
 * (`in_project=false`) — drafts, hand-offs, a file saved with no project —
 * which are what the run's count, its chips and a connector's delivery count
 * (R38). The files it wrote into the project (`in_project=true`) are the
 * project's own and are listed apart, one row each. `task.outcome.artifacts`
 * (free-form JSON the dispatcher writes) is the fallback for a run whose
 * by-products the list cannot answer for, and those entries carry no id, so
 * such a row states what the run reported without pretending to open it.
 *
 * A row's name is the agent's — a project file's is the path it chose — so it
 * is drawn through `lib/hidden-chars`, as the Library's row draws it: each
 * hidden character as a `<U+XXXX>` escape, with the count on a line of its own
 * under the name. A name with no hidden character draws as it is.
 */

import {
  EscapedText,
  FileBadge,
  languageFromName,
  SectionCard,
  SectionEmpty,
} from "@/components/ui";
import type { OutcomeArtifact } from "@/components/work/run-model";
import { cn } from "@/lib/cn";
import { escapeHiddenLine, hiddenNotice } from "@/lib/hidden-chars";

/** The design's own empty sentence for this card. */
export const OUTPUT_EMPTY =
  "Nothing produced yet. Files land here and in the Library as the run works.";

/** Output's sentence when the run's only files are in its project. */
export const OUTPUT_EMPTY_BESIDE_PROJECT =
  "Nothing else produced. The files this run wrote are in the project, below.";

/** The second card's title (R38's "written in the project"). */
export const PROJECT_FILES_TITLE = "Written in the project";

/** The second card's sentence when its list could not be read. */
export const PROJECT_FILES_UNREADABLE =
  "The files this run wrote into the project could not be listed.";

export interface OutputSectionProps {
  /** The run's by-products. */
  artifacts: readonly OutcomeArtifact[];
  /** `task.artifact_count` — by-products only (R38) — which can exceed the number of readable entries. */
  count: number;
  /** Present only once artifacts have ids to open. */
  onOpen?: (artifact: OutcomeArtifact) => void;
  /** Why the list is short or absent — a failed request, never a gap. */
  note?: string | null;
  /** The files the run wrote into its project, one row each. */
  projectFiles?: readonly OutcomeArtifact[];
  /** Their unpaged count; past `projectFiles.length` the card says how many more the Library holds. */
  projectTotal?: number;
  /** Why the project files could not be listed. */
  projectNote?: string | null;
}

const ROW =
  "flex w-full items-center gap-[10px] border-b border-line-hair-2 px-[16px] py-[11px] text-left last:border-b-0";

/** A row's name, escaped, with its hidden-character count under it. */
function RowName({ name }: { name: string }) {
  const escaped = escapeHiddenLine(name);
  const notice = hiddenNotice(escaped.hidden);
  return (
    <span className="min-w-0 flex-1">
      <span
        className={cn(
          "block text-base-plus text-ink",
          // A name holding an escape wraps: an ellipsis could cut off one
          // the count says is there.
          escaped.hidden > 0 ? "wrap-anywhere" : "truncate",
        )}
      >
        <EscapedText escaped={escaped} />
      </span>
      {notice !== null && (
        <span className="mt-[2px] block text-2xs-plus text-red-ink">
          {notice}
        </span>
      )}
    </span>
  );
}

function FileRows({
  artifacts,
  onOpen,
}: {
  artifacts: readonly OutcomeArtifact[];
  onOpen?: (artifact: OutcomeArtifact) => void;
}) {
  return (
    <div>
      {artifacts.map((artifact) => {
        const inner = (
          <>
            <FileBadge
              kind={artifact.kind}
              size={17}
              language={languageFromName(artifact.name)}
            />
            <RowName name={artifact.name} />
            {artifact.stamp !== null && (
              <span className="shrink-0 font-mono text-xs text-muted-fg">
                {artifact.stamp}
              </span>
            )}
          </>
        );

        if (onOpen === undefined || artifact.id === null) {
          return (
            <div key={artifact.name} className={ROW}>
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
              ROW,
              "cursor-pointer border-none bg-transparent hover:bg-sunken",
              "focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-blue",
            )}
          >
            {inner}
          </button>
        );
      })}
    </div>
  );
}

function CardNote({ text }: { text: string | null }) {
  if (text === null || text === "") return null;
  return (
    <p className="m-0 px-[16px] pt-[8px] pb-[12px] font-mono text-2xs-plus leading-[1.5] text-faint">
      {text}
    </p>
  );
}

export function OutputSection({
  artifacts,
  count,
  onOpen,
  note = null,
  projectFiles = [],
  projectTotal = projectFiles.length,
  projectNote = null,
}: OutputSectionProps) {
  const more = projectTotal - projectFiles.length;
  const showProject =
    projectFiles.length > 0 || (projectNote !== null && projectNote !== "");

  return (
    <>
      <SectionCard title="Output">
        {artifacts.length === 0 ? (
          <SectionEmpty note={note ?? undefined}>
            {count > 0
              ? `This run reported ${count} file${count === 1 ? "" : "s"}, but none of them can be listed.`
              : projectFiles.length > 0
                ? OUTPUT_EMPTY_BESIDE_PROJECT
                : OUTPUT_EMPTY}
          </SectionEmpty>
        ) : (
          <>
            <FileRows artifacts={artifacts} onOpen={onOpen} />
            <CardNote text={note} />
          </>
        )}
      </SectionCard>

      {showProject && (
        <SectionCard
          title={PROJECT_FILES_TITLE}
          action={
            <span className="font-mono text-xs text-muted-fg">
              {`${projectTotal} file${projectTotal === 1 ? "" : "s"}`}
            </span>
          }
        >
          {projectFiles.length === 0 ? (
            <SectionEmpty note={projectNote ?? undefined}>
              {PROJECT_FILES_UNREADABLE}
            </SectionEmpty>
          ) : (
            <>
              <FileRows artifacts={projectFiles} onOpen={onOpen} />
              <CardNote
                text={
                  more > 0
                    ? `${more} more are listed in the Library, under Results in the project.`
                    : null
                }
              />
              <CardNote text={projectNote} />
            </>
          )}
        </SectionCard>
      )}
    </>
  );
}
