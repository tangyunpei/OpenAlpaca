/**
 * `Resizer` (DESIGN_SPEC §2.7, §4.6) — the 7px grab strip between two panes.
 *
 * The negative 3px margins let the hit area overlap both neighbours without
 * moving them, and the hover background paints a 2px hairline down the middle
 * of that 7px zone rather than filling it.
 *
 * Drag maths is §4.6 verbatim: capture the pointer's x and the pane's width,
 * clamp `startW + dir * dx` into the pane's bounds, and persist all three
 * widths on release. Double-click resets this pane and persists.
 *
 * Two additions, both accessibility (§8.8 lists none of this in the design):
 * the strip is a focusable `separator`, and arrow keys nudge it. A pointer is
 * otherwise the only way to change a pane's width.
 */

import { useCallback, useEffect, useRef } from "react";

import { cn } from "@/lib/cn";
import { PANE_BOUNDS, type PaneKey } from "@/stores/pane-widths";
import { useUiStore } from "@/stores/ui";

/** One arrow press. */
const KEY_STEP = 16;

export interface ResizerProps {
  paneKey: PaneKey;
  /** `-1` when dragging left should *grow* the pane (the chat aside). */
  direction: 1 | -1;
  /** Names the pane for assistive tech, e.g. "chat side pane". */
  label: string;
}

export function Resizer({ paneKey, direction, label }: ResizerProps) {
  const width = useUiStore((s) => s.paneWidths[paneKey]);
  const setPaneWidth = useUiStore((s) => s.setPaneWidth);
  const persist = useUiStore((s) => s.persistPaneWidths);
  const reset = useUiStore((s) => s.resetPaneWidth);
  const bounds = PANE_BOUNDS[paneKey];

  /** Non-null only mid-drag; window listeners read it without re-subscribing. */
  const drag = useRef<{ startX: number; startW: number } | null>(null);
  const cleanup = useRef<(() => void) | null>(null);

  // A drag that outlives the component would keep writing to the store.
  useEffect(() => () => cleanup.current?.(), []);

  const onMouseDown = useCallback(
    (event: React.MouseEvent<HTMLDivElement>) => {
      event.preventDefault();
      drag.current = {
        startX: event.clientX,
        startW: useUiStore.getState().paneWidths[paneKey],
      };
      document.body.style.userSelect = "none";
      document.body.style.cursor = "col-resize";

      const onMove = (moveEvent: MouseEvent) => {
        const start = drag.current;
        if (start === null) return;
        // The store clamps; this only has to compute the intent.
        setPaneWidth(
          paneKey,
          start.startW + direction * (moveEvent.clientX - start.startX),
        );
      };
      const onUp = () => {
        cleanup.current?.();
        persist();
      };

      cleanup.current = () => {
        window.removeEventListener("mousemove", onMove);
        window.removeEventListener("mouseup", onUp);
        cleanup.current = null;
        drag.current = null;
        document.body.style.userSelect = "";
        document.body.style.cursor = "";
      };

      window.addEventListener("mousemove", onMove);
      window.addEventListener("mouseup", onUp);
    },
    [direction, paneKey, persist, setPaneWidth],
  );

  const onKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      const delta =
        event.key === "ArrowLeft"
          ? -KEY_STEP
          : event.key === "ArrowRight"
            ? KEY_STEP
            : 0;
      if (delta !== 0) {
        event.preventDefault();
        setPaneWidth(paneKey, width + direction * delta);
        persist();
        return;
      }
      if (event.key === "Home") {
        event.preventDefault();
        reset(paneKey);
      }
    },
    [direction, paneKey, persist, reset, setPaneWidth, width],
  );

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label={`Resize ${label}`}
      aria-valuenow={width}
      aria-valuemin={bounds.min}
      aria-valuemax={bounds.max}
      tabIndex={0}
      title="Drag to resize · double-click to reset"
      onMouseDown={onMouseDown}
      onDoubleClick={() => reset(paneKey)}
      onKeyDown={onKeyDown}
      className={cn(
        "relative z-10 mx-[-3px] w-[7px] shrink-0 cursor-col-resize self-stretch",
        "hover:bg-[linear-gradient(90deg,transparent_2px,#B9B3A6_2px,#B9B3A6_4px,transparent_4px)]",
        "focus-visible:bg-[linear-gradient(90deg,transparent_2px,#3A5FCC_2px,#3A5FCC_4px,transparent_4px)] focus-visible:outline-none",
      )}
    />
  );
}
