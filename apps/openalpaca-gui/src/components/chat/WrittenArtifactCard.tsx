/**
 * `WrittenArtifactCard` — a file an agent just wrote, shown inline in the
 * transcript.
 *
 * The same house shape as `RunReportCard` (§3.12): a status strip over a body,
 * with the eyebrow saying what happened and the right-hand mono segment
 * carrying the identity. Deliberately smaller than an `ArtifactCard` (§3.13):
 * an `artifact_written` frame carries the name, kind and version and *no
 * content*, so this card links to the file rather than previewing lines it does
 * not have. Everything it shows came off the frame.
 */

import { FileBadge, type FileKind } from "@/components/ui";

export interface WrittenArtifactCardProps {
  name: string;
  kind: FileKind;
  language?: string | null;
  version: number;
  /** `13:41`, or `null` when the frame's stamp is unreadable. */
  time: string | null;
  /** Opens the file in the Library. */
  onOpen: () => void;
}

export function WrittenArtifactCard({
  name,
  kind,
  language = null,
  version,
  time,
  onOpen,
}: WrittenArtifactCardProps) {
  return (
    <section className="mb-[26px] overflow-hidden rounded-3xl border border-line bg-raised">
      <header className="flex items-center gap-[9px] border-b border-line-hair bg-sunken px-[14px] py-[10px]">
        <span
          aria-hidden
          className="block h-[6px] w-[6px] shrink-0 rounded-full bg-blue"
        />
        <span className="font-mono text-2xs-plus tracking-eyebrow text-blue uppercase">
          File written
          {time !== null && ` · ${time}`}
        </span>
        <span className="ml-auto font-mono text-xs text-faint">v{version}</span>
      </header>

      <div className="flex items-center gap-[10px] px-[14px] py-[12px]">
        <FileBadge kind={kind} size={19} language={language} />
        <span className="min-w-0 flex-1 truncate text-md font-medium">
          {name}
        </span>
        <button
          type="button"
          onClick={onOpen}
          className="shrink-0 cursor-pointer rounded-md border border-line bg-transparent px-[9px] py-[4px] font-sans text-sm-plus leading-[normal] font-medium text-secondary hover:bg-muted-2 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue"
        >
          Open in Library
        </button>
      </div>
    </section>
  );
}
