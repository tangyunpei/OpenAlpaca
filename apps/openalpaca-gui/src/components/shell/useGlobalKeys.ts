/**
 * Global keyboard bindings (DESIGN_SPEC §4.5).
 *
 * One `keydown` listener on `window`, exactly as the design does it:
 *
 *   ⌘K / Ctrl+K  always            preventDefault, **toggle** the palette
 *   Esc          palette open      close the palette
 *                else picker open  close the artifact picker
 *                else panel open   `panelArt = null; workOpen = true`
 *                else blocked      `resolve("denied")`
 *   Enter        blocked, in chat,
 *                palette closed    preventDefault, `resolve("approved")`
 *
 * The Escape ladder is strictly ordered — one if/else chain, not four
 * independent handlers — and lives in the UI store as `handleEscape()` so the
 * ordering is testable without a DOM. This hook only adds the last rung, which
 * needs a confirmation the store knows nothing about.
 *
 * `blocked`, `onApprove` and `onDeny` come from the chat view, which owns the
 * pending confirmation; until that lands the last rung is simply inert.
 */

import { useEffect } from "react";

import { useUiStore } from "@/stores/ui";

export interface GlobalKeyOptions {
  /** True while a tool confirmation is pending in the chat lane. */
  blocked?: boolean;
  /** `resolve("approved")`. */
  onApprove?: () => void;
  /** `resolve("denied")`. */
  onDeny?: () => void;
}

/** A field the user is typing into must keep Enter and Escape for itself. */
function isTextEntry(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  return (
    tag === "INPUT" ||
    tag === "TEXTAREA" ||
    tag === "SELECT" ||
    target.isContentEditable
  );
}

export function useGlobalKeys({
  blocked = false,
  onApprove,
  onDeny,
}: GlobalKeyOptions = {}): void {
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        useUiStore.getState().togglePalette();
        return;
      }

      const state = useUiStore.getState();

      if (event.key === "Escape") {
        const outcome = state.handleEscape();
        if (outcome !== "none") {
          event.preventDefault();
          return;
        }
        if (blocked && onDeny !== undefined) {
          event.preventDefault();
          onDeny();
        }
        return;
      }

      if (
        event.key === "Enter" &&
        !event.shiftKey &&
        blocked &&
        !state.paletteOpen &&
        state.view === "chat" &&
        onApprove !== undefined &&
        // The composer is unmounted while blocked (§4.5), but the palette input
        // and any other field must still get their own Enter.
        !isTextEntry(event.target)
      ) {
        event.preventDefault();
        onApprove();
      }
    }

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [blocked, onApprove, onDeny]);
}
