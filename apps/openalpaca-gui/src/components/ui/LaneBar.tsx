/**
 * `LaneBar` (DESIGN_SPEC §3.21) — one horizontal Gantt lane, two sizes.
 *
 * The red→green coupling in §4.4 is the important part: a `block` lane is red
 * *and* carries a hatched amber "pending" overlay only while the app is
 * actually blocked on a confirmation. Resolving the confirmation turns the same
 * lane green and removes the hatch — one state change, three visual
 * consequences — so `blocked` is a prop of the lane row, never baked into the
 * lane data.
 *
 * `start`/`end` are percentages of the run's wall clock. They are clamped here
 * because they come from a timeline the daemon does not serve yet (GAP-09) and
 * an out-of-range value must not paint outside the track.
 */

import { cn } from "@/lib/cn";

export type LaneState = "done" | "run" | "block";

export interface Lane {
  label: string;
  /** Percent of the run's wall clock. */
  start: number;
  end: number;
  state: LaneState;
  /** Wide size only — e.g. "awaiting you". */
  detail?: string;
}

export type LaneSize = "compact" | "wide";

const HATCH =
  "repeating-linear-gradient(90deg,#E3C9B8 0 3px, transparent 3px 6px)";

/** Bar fill: a blocked lane is red only while the app is still blocked. */
export function laneColor(state: LaneState, blocked: boolean): string {
  if (state === "done") return "var(--color-green)";
  if (state === "run") return "var(--color-line-hover)";
  return blocked ? "var(--color-red)" : "var(--color-green)";
}

/** The amber hatch marks work that has not run yet because it is waiting on you. */
export function showsPending(state: LaneState, blocked: boolean): boolean {
  return state === "block" && blocked;
}

const clampPct = (value: number): number =>
  Number.isFinite(value) ? Math.min(100, Math.max(0, value)) : 0;

/** `left`/`width` for the bar, already clamped into the track. */
export function laneGeometry(lane: Lane): { left: number; width: number } {
  const left = clampPct(lane.start);
  const end = Math.max(left, clampPct(lane.end));
  return { left, width: end - left };
}

export interface LaneBarProps {
  lane: Lane;
  size?: LaneSize;
  /** True while a tool confirmation is pending on this run. */
  blocked?: boolean;
  className?: string;
}

export function LaneBar({
  lane,
  size = "compact",
  blocked = false,
  className,
}: LaneBarProps) {
  const wide = size === "wide";
  const { left, width } = laneGeometry(lane);
  const pending = showsPending(lane.state, blocked);
  const isBlocked = lane.state === "block";

  return (
    <div className={cn("flex items-center", wide ? "gap-[11px]" : "gap-[8px]")}>
      <span
        className={cn(
          "shrink-0 text-right font-mono",
          wide ? "w-[96px] text-xs-plus" : "w-[70px] text-2xs-plus",
          isBlocked
            ? "text-amber-ink"
            : wide
              ? "text-secondary"
              : "text-tertiary",
        )}
      >
        {lane.label}
      </span>

      <div
        className={cn(
          "relative flex-1 overflow-hidden rounded-sm",
          wide ? "h-[16px] bg-muted-2" : "h-[8px] bg-muted",
          className,
        )}
      >
        <div
          className="absolute top-0 bottom-0 rounded-sm"
          style={{
            left: `${left}%`,
            width: `${width}%`,
            background: laneColor(lane.state, blocked),
          }}
        />
        {pending && (
          <div
            aria-hidden
            className="absolute top-0 bottom-0"
            style={{
              left: `${left + width}%`,
              width: `${100 - (left + width)}%`,
              background: HATCH,
            }}
          />
        )}
      </div>

      {wide && (
        <span className="w-[88px] shrink-0 font-mono text-2xs-plus text-muted-fg">
          {lane.detail ?? ""}
        </span>
      )}
    </div>
  );
}

export interface LaneAxisProps {
  /** Three labels: start · mid · now. */
  labels: readonly [string, string, string];
}

/** The wide timeline's axis row — a 96px spacer, the labels, an 88px spacer. */
export function LaneAxis({ labels }: LaneAxisProps) {
  return (
    <div className="mb-[7px] flex items-center gap-[11px]">
      <span className="w-[96px] shrink-0" />
      <div className="flex flex-1 justify-between font-mono text-2xs-plus text-faint">
        {labels.map((label) => (
          <span key={label}>{label}</span>
        ))}
      </div>
      <span className="w-[88px] shrink-0" />
    </div>
  );
}
