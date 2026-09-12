/**
 * `Toast` (DESIGN_SPEC §3.34) — one slot, never a stack.
 *
 * Presentational only: the single-slot behaviour and the 2600ms timer live in
 * the UI store (`showToast`), which already cancels a pending timer when a new
 * toast replaces the old one. `role="status"` is an accessibility addition
 * (§8.8) — the design announces nothing.
 */

import { cn } from "@/lib/cn";

export interface ToastProps {
  message: string;
  className?: string;
}

export function Toast({ message, className }: ToastProps) {
  return (
    <div
      role="status"
      aria-live="polite"
      className={cn(
        "absolute right-[20px] bottom-[20px] z-[60] flex max-w-[380px] items-center gap-[10px] rounded-2xl bg-ink px-[14px] py-[10px] text-on-dark shadow-toast",
        className,
      )}
    >
      <span
        aria-hidden
        className="block h-[6px] w-[6px] shrink-0 rounded-full bg-green-dot"
      />
      <span className="text-base-plus leading-[1.45]">{message}</span>
    </div>
  );
}
