/**
 * The ⌘K command catalogue (DESIGN_SPEC §3.33, §4.4 "Palette").
 *
 * The design ships eight rows. Seven of them are static and built here. The
 * eighth — `Find <filename>` — is a *query*: the palette asks
 * `GET /v1/artifacts?q=` for what the reader typed and appends a row per hit,
 * so those rows depend on the typed text and cannot be part of a fixed
 * catalogue.
 *
 * Two rows are conditional on live state and simply do not exist without it:
 *   `Steer …`    needs a run in `GET /v1/tasks?status=active`
 *   `Approve …`  needs a pending tool confirmation, which the chat view owns
 *
 * §5.6 records that every shortcut except ⌘K is display-only in the export and
 * says to wire them for real. `Shortcut` is therefore a real matcher, not a
 * string: `useCommandShortcuts` binds it. The one exception is `Approve`, whose
 * `↵` is already bound globally by `useGlobalKeys` — it carries a display label
 * and no matcher, so the two bindings cannot fight over the same key.
 */

import { escapeHiddenLine, hiddenOnCard } from "@/lib/hidden-chars";

/** A key combination the palette can both draw and match. */
export interface Shortcut {
  /** `event.key`, compared case-insensitively. */
  key: string;
  /** ⌘ on macOS, Ctrl elsewhere — the design writes both as ⌘. */
  meta?: boolean;
  shift?: boolean;
}

export type CommandGroup = "Run" | "Steer" | "Approve" | "Go" | "View" | "Find";

export interface Command {
  id: string;
  group: CommandGroup;
  label: string;
  /** Bound for real by `useCommandShortcuts`. */
  shortcut?: Shortcut;
  /** Drawn instead of a derived label when the key is bound elsewhere. */
  shortcutLabel?: string;
  run: () => void;
}

export function shortcutLabel(command: Command): string | null {
  if (command.shortcutLabel !== undefined) return command.shortcutLabel;
  const shortcut = command.shortcut;
  if (shortcut === undefined) return null;
  const modifier = shortcut.meta === true ? "⌘" : "";
  const shift = shortcut.shift === true ? "⇧" : "";
  return `${modifier}${shift}${shortcut.key.toUpperCase()}`;
}

/**
 * `⌘` and `Ctrl` are the same intent on the two platforms the shell ships to,
 * so either satisfies `meta`. Shift is matched exactly: `⌘⇧D` must not fire on
 * `⌘D`, and `⌘2` must not fire on `⌘⇧2`.
 */
export function matchesShortcut(
  event: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "shiftKey">,
  shortcut: Shortcut,
): boolean {
  if (event.key.toLowerCase() !== shortcut.key.toLowerCase()) return false;
  const wantsModifier = shortcut.meta === true;
  if (wantsModifier !== (event.metaKey || event.ctrlKey)) return false;
  return (shortcut.shift === true) === event.shiftKey;
}

/** Substring match over `group + label`, exactly as §3.33 prescribes. */
export function filterCommands(
  commands: readonly Command[],
  query: string,
): Command[] {
  const needle = query.trim().toLowerCase();
  if (needle === "") return [...commands];
  return commands.filter((command) =>
    `${command.group} ${command.label}`.toLowerCase().includes(needle),
  );
}

/** Everything the catalogue needs from the app, so it stays a pure function. */
export interface CommandCatalogInput {
  /** The run a `Steer` row would target; `null` when nothing is running. */
  activeRun: { id: string; title: string } | null;
  /**
   * The tool awaiting confirmation, when the chat lane has one, and the
   * hidden characters its whole request holds (the card's count).
   */
  pendingConfirmation: {
    toolName: string;
    hidden: number;
    approve: () => void;
  } | null;
  goChat: () => void;
  goWork: () => void;
  goLibrary: () => void;
  goMemory: () => void;
  goSettingsSection: (sectionId: string) => void;
  steerRun: (runId: string) => void;
  toggleDense: () => void;
}

/**
 * The catalogue in the design's own order. `⌘1` is added: §3.33 notes it is
 * "implied but absent", and a Go group that can reach three views but not the
 * default one is a hole, not a design decision.
 */
export function buildCommands(input: CommandCatalogInput): Command[] {
  const commands: Command[] = [
    {
      id: "run.new",
      group: "Run",
      label: "New background job",
      shortcut: { key: "n", meta: true },
      run: input.goChat,
    },
  ];

  const run = input.activeRun;
  if (run !== null) {
    commands.push({
      id: "steer.active",
      group: "Steer",
      label: `Steer ${run.title}`,
      shortcut: { key: "s", meta: true, shift: true },
      run: () => input.steerRun(run.id),
    });
  }

  const pending = input.pendingConfirmation;
  if (pending !== null) {
    commands.push({
      id: "approve.pending",
      group: "Approve",
      // The row approves, so it names the tool as the card does, and it
      // shows the name alone: the whole request's count, and where it is
      // drawn, go beside it. A label is a string: the escapes are the
      // plain-text `<U+XXXX>` form.
      label: [
        `Approve pending ${escapeHiddenLine(pending.toolName).text}`,
        hiddenOnCard(pending.hidden),
      ]
        .filter((part) => part !== null)
        .join(" · "),
      // Enter is already the global approve binding (§4.5); drawing it here
      // documents the key without claiming a second owner for it.
      shortcutLabel: "↵",
      run: pending.approve,
    });
  }

  commands.push(
    {
      id: "go.chat",
      group: "Go",
      label: "Chat — conversation",
      shortcut: { key: "1", meta: true },
      run: input.goChat,
    },
    {
      id: "go.work",
      group: "Go",
      label: "Work — all runs",
      shortcut: { key: "2", meta: true },
      run: input.goWork,
    },
    {
      id: "go.library",
      group: "Go",
      label: "Library — artifacts",
      shortcut: { key: "3", meta: true },
      run: input.goLibrary,
    },
    // The fifth view (memory redesign G30) takes the next numeral: a Go group
    // that reaches four views of five has the same hole ⌘1 once filled.
    {
      id: "go.memory",
      group: "Go",
      label: "Memory — notes",
      shortcut: { key: "4", meta: true },
      run: input.goMemory,
    },
    {
      id: "view.density",
      group: "View",
      label: "Toggle compact density",
      shortcut: { key: "d", meta: true, shift: true },
      run: input.toggleDense,
    },
    {
      id: "go.settings.tools",
      group: "Go",
      label: "Settings — tools & extensions",
      shortcut: { key: ",", meta: true },
      run: () => input.goSettingsSection("tools"),
    },
  );

  return commands;
}
