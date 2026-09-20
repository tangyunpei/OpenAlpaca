/**
 * `CommandPalette` (DESIGN_SPEC §3.33, §2.6, §4.4).
 *
 * The overlay is the design's own: `rgba(30,29,27,.28)` at z-50, dismissed only
 * when the click lands on the overlay itself, with a 560px dialog pinned 120px
 * from the top. Escape is not handled here — `useGlobalKeys` owns the ordered
 * escape ladder and the palette is its first rung, and the shortcuts the rows
 * advertise are bound once at the app root, not here, so a key cannot fire two
 * listeners while the palette is open.
 *
 * Two departures from the export, both required by the brief:
 *
 *  * **Filtering and keyboard navigation are real.** §3.33 says the export
 *    implements neither and prescribes a substring match over `group + label`;
 *    ↑/↓/Home/End move the selection and ↵ runs it.
 *  * **The `Find <filename>` rows are a query.** `GET /v1/artifacts?q=` is
 *    asked for whatever was typed and one row is appended per hit, opening that
 *    artifact in the Library on the Preview tab. No text, no rows — the palette
 *    never invents a filename to offer.
 *
 * `pendingConfirmation` defaults to the confirmation the chat lane publishes;
 * without one the `Approve` row simply does not exist.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { Scrim } from "@/components/ui";
import { useArtifacts } from "@/hooks/useArtifacts";
import { cn } from "@/lib/cn";
import { useUiStore } from "@/stores/ui";

import { filterCommands, shortcutLabel, type Command } from "./commands";
import {
  useCommandCatalog,
  type PendingConfirmation,
} from "./useCommandCatalog";

export interface CommandPaletteProps {
  /**
   * Overrides the confirmation the chat lane publishes. Left unset, the
   * palette reads the published one.
   */
  pendingConfirmation?: PendingConfirmation | null;
}

/** Mounted only while `paletteOpen`, so its queries do not run at rest. */
export function CommandPalette({ pendingConfirmation }: CommandPaletteProps) {
  const open = useUiStore((s) => s.paletteOpen);
  if (!open) return null;
  return <PaletteDialog pendingConfirmation={pendingConfirmation} />;
}

/** Enough hits to be useful without pushing the commands off the dialog. */
const FIND_LIMIT = 5;

function PaletteDialog({ pendingConfirmation }: CommandPaletteProps) {
  const setPaletteOpen = useUiStore((s) => s.setPaletteOpen);
  const openArtifact = useUiStore((s) => s.openArtifact);

  const commands = useCommandCatalog(pendingConfirmation);

  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  const needle = query.trim();
  const artifacts = useArtifacts(
    { q: needle, limit: FIND_LIMIT },
    { enabled: needle !== "" },
  );

  const results = useMemo(() => {
    const matched = filterCommands(commands, query);
    if (needle === "") return matched;
    const finds: Command[] = (artifacts.data?.artifacts ?? []).map(
      (artifact) => ({
        id: `find.${artifact.id}`,
        group: "Find",
        label: artifact.name,
        run: () => {
          setPaletteOpen(false);
          // Sets the view, the selection and the Preview tab in one action.
          openArtifact(artifact.id);
        },
      }),
    );
    return [...matched, ...finds];
  }, [commands, query, needle, artifacts.data, openArtifact, setPaletteOpen]);

  // A shrinking result set must never leave the cursor past the end.
  useEffect(() => {
    setActiveIndex((index) =>
      results.length === 0 ? 0 : Math.min(index, results.length - 1),
    );
  }, [results.length]);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const runCommand = useCallback((command: Command) => {
    command.run();
  }, []);

  const onKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      if (results.length === 0) return;
      if (event.key === "ArrowDown") {
        event.preventDefault();
        setActiveIndex((index) => (index + 1) % results.length);
        return;
      }
      if (event.key === "ArrowUp") {
        event.preventDefault();
        setActiveIndex(
          (index) => (index - 1 + results.length) % results.length,
        );
        return;
      }
      if (event.key === "Home") {
        event.preventDefault();
        setActiveIndex(0);
        return;
      }
      if (event.key === "End") {
        event.preventDefault();
        setActiveIndex(results.length - 1);
        return;
      }
      if (event.key === "Enter") {
        event.preventDefault();
        const command = results[activeIndex];
        if (command !== undefined) runCommand(command);
      }
    },
    [activeIndex, results, runCommand],
  );

  return (
    <Scrim
      variant="veil"
      zIndex={50}
      closeOnSelfOnly
      onClose={() => setPaletteOpen(false)}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
        onKeyDown={onKeyDown}
        className="w-palette overflow-hidden rounded-4xl border border-line-popover bg-raised shadow-dialog"
      >
        <div className="flex items-center gap-[10px] border-b border-line-hair px-[16px] py-[14px]">
          <span aria-hidden className="font-mono text-sm text-faint">
            ⌘K
          </span>
          <input
            ref={inputRef}
            type="text"
            role="combobox"
            aria-expanded
            aria-controls="oa-palette-list"
            aria-activedescendant={
              results[activeIndex] === undefined
                ? undefined
                : `oa-palette-${results[activeIndex].id}`
            }
            aria-label="Run a command"
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              setActiveIndex(0);
            }}
            placeholder="Run a command, steer a task, find an artifact…"
            className="min-w-0 flex-1 border-none bg-transparent font-sans text-lg text-ink outline-none placeholder:text-faint"
          />
          <span aria-hidden className="font-mono text-xs text-faint">
            esc
          </span>
        </div>

        <div id="oa-palette-list" role="listbox" className="p-[8px]">
          {results.map((command, index) => (
            <PaletteRow
              key={command.id}
              command={command}
              active={index === activeIndex}
              onHover={() => setActiveIndex(index)}
              onRun={() => runCommand(command)}
            />
          ))}

          {results.length === 0 && (
            <p className="m-0 px-[10px] py-[9px] text-md text-muted-fg">
              {artifacts.isLoading
                ? "Searching the library…"
                : "No commands or files match that."}
            </p>
          )}

          {artifacts.error !== null && (
            <p className="mt-[4px] mb-0 border-t border-line-hair px-[10px] pt-[9px] font-mono text-2xs-plus text-faint">
              The library could not be searched — {artifacts.error.message}
            </p>
          )}
        </div>
      </div>
    </Scrim>
  );
}

interface PaletteRowProps {
  command: Command;
  active: boolean;
  onHover: () => void;
  onRun: () => void;
}

function PaletteRow({ command, active, onHover, onRun }: PaletteRowProps) {
  const shortcut = shortcutLabel(command);
  return (
    <button
      type="button"
      role="option"
      id={`oa-palette-${command.id}`}
      aria-selected={active}
      onMouseEnter={onHover}
      onClick={onRun}
      className={cn(
        "flex w-full cursor-pointer items-center gap-[11px] rounded-lg border-none px-[10px] py-[9px] text-left",
        "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue",
        active ? "bg-muted-2" : "bg-transparent",
      )}
    >
      <span className="w-[66px] shrink-0 font-mono text-2xs tracking-tag text-muted-fg uppercase">
        {command.group}
      </span>
      <span className="flex-1 text-md text-ink">{command.label}</span>
      {shortcut !== null && (
        <span className="font-mono text-xs text-faint">{shortcut}</span>
      )}
    </button>
  );
}
