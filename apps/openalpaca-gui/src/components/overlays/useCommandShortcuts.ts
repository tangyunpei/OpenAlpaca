/**
 * Binds the palette's shortcuts globally (DESIGN_SPEC §5.6).
 *
 * The export leaves `⌘N`, `⌘⇧S`, `⌘1–3`, `⌘⇧D` and `⌘,` as decoration; the spec
 * says to make them real. They are bound here rather than in `useGlobalKeys`
 * because the catalogue is what defines them — a command with no matcher (the
 * `Approve` row, whose `↵` `useGlobalKeys` already owns) is skipped, so the two
 * listeners can never both claim a key.
 *
 * Mount this **once**, at the app root. The listener sits on `window`, so it
 * fires with the palette's own input focused too — the palette does not (and
 * must not) mount a second copy: `stopPropagation` does not stop other
 * listeners on the same target, so two copies would run every command twice.
 * `stopImmediatePropagation` is used here so a future second listener on
 * `window` still cannot double-fire one key.
 */

import { useEffect } from "react";

import { matchesShortcut, type Command } from "./commands";

export function useCommandShortcuts(commands: readonly Command[]): void {
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      for (const command of commands) {
        if (command.shortcut === undefined) continue;
        if (!matchesShortcut(event, command.shortcut)) continue;
        event.preventDefault();
        event.stopImmediatePropagation();
        command.run();
        return;
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [commands]);
}
