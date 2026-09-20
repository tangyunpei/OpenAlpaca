/**
 * `Scrim` — the two dismiss layers in the design (DESIGN_SPEC §1.6, §3.33).
 *
 *   `veil`    — the visible `rgba(30,29,27,.28)` behind the command palette,
 *               `position:absolute` inside the app frame, z-50.
 *   `catcher` — an invisible `position:fixed; inset:0` click-catcher under the
 *               model picker (z-39) and the artifact picker (z-30).
 *
 * The palette's overlay closes **only** when the click lands on the overlay
 * itself (§4.4) — clicks that bubble up from the dialog must not dismiss it —
 * which is what `closeOnSelfOnly` encodes.
 */

import { cn } from "@/lib/cn";

export type ScrimVariant = "veil" | "catcher";

export interface ScrimProps {
  variant?: ScrimVariant;
  /** z-30 artifact picker · z-39 model picker · z-50 palette. */
  zIndex: number;
  onClose: () => void;
  /** `true` for the palette veil: ignore clicks that bubble from children. */
  closeOnSelfOnly?: boolean;
  children?: React.ReactNode;
  className?: string;
}

export function Scrim({
  variant = "catcher",
  zIndex,
  onClose,
  closeOnSelfOnly = false,
  children,
  className,
}: ScrimProps) {
  return (
    <div
      // Not a button: it is a backdrop. Keyboard users dismiss with Escape,
      // which the global key handler owns.
      role="presentation"
      onClick={(event) => {
        if (closeOnSelfOnly && event.target !== event.currentTarget) return;
        onClose();
      }}
      style={{ zIndex }}
      className={cn(
        "inset-0",
        variant === "veil"
          ? "absolute flex items-start justify-center bg-[rgba(30,29,27,.28)] pt-[120px]"
          : "fixed",
        className,
      )}
    >
      {children}
    </div>
  );
}
