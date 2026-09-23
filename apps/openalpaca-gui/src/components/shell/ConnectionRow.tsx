/**
 * `ConnectionRow` (DESIGN_SPEC §3.6) — the rail's daemon status line.
 *
 * The design draws one state (green · "connected" · `7f3a`). §3.6 extrapolates
 * the other two colours from the status palette: red for an error, gold while
 * connecting. The label words come from the socket's own state machine
 * (`lib/events.ts`), so nothing here is invented — and the instance id is the
 * real `instanceId`, cut to four characters exactly as the design shows.
 *
 * A **stopped** daemon is a fourth state, and deliberately not an error: the
 * dot takes the neutral muted token and the word says `stopped` (this window
 * stopped it) or `stopped elsewhere` (the daemon said it was going away and
 * this window had not asked). `disconnected` and the red dot stay for a
 * daemon that went away without saying so.
 *
 * `ConnectionRowView` is the presentational half; `ConnectionRow` binds it to
 * the live socket + `/v1/health`.
 */

import { useConnectionStatus } from "@/hooks/useConnection";
import type { StopIntent, StopPhase } from "@/lib/connection";
import type { EventsStatus } from "@/lib/events";
import { cn } from "@/lib/cn";

export type ConnectionTone = "up" | "pending" | "down" | "stopped";

/** Socket state → the dot's colour and the word beside it. */
export function connectionTone(
  status: EventsStatus,
  healthy: boolean,
  intent: StopIntent = null,
): ConnectionTone {
  if (intent !== null) return "stopped";
  if (status === "connected") return healthy ? "up" : "pending";
  if (status === "connecting" || status === "idle") return "pending";
  return "down";
}

export function connectionLabel(
  status: EventsStatus,
  intent: StopIntent = null,
  phase: StopPhase = null,
): string {
  if (intent === "stopped_here") {
    // Only what the shell's wait concluded may say "stopped".
    switch (phase) {
      case "stopping":
        return "stopping";
      case "still_alive":
        return "still running";
      case "lock_still_held":
        return "lock still held";
      case "unconfirmed":
        return "stop unconfirmed";
      default:
        return "stopped";
    }
  }
  if (intent === "stopped_elsewhere") return "stopped elsewhere";
  switch (status) {
    case "connected":
      return "connected";
    case "connecting":
      return "connecting";
    case "idle":
      return "starting";
    case "disconnected":
      return "disconnected";
    case "error":
      return "connection error";
  }
}

const TONE_DOT: Record<ConnectionTone, string> = {
  up: "bg-green",
  pending: "bg-gold",
  down: "bg-red",
  stopped: "bg-muted-fg",
};

export interface ConnectionRowViewProps {
  tone: ConnectionTone;
  label: string;
  /** First four characters of `instanceId`; `null` before the daemon answers. */
  instance: string | null;
}

export function ConnectionRowView({
  tone,
  label,
  instance,
}: ConnectionRowViewProps) {
  return (
    <div className="flex items-center gap-[8px] px-[8px]">
      <span
        role="img"
        aria-label={`Daemon ${label}`}
        className={cn("block h-[7px] w-[7px] rounded-full", TONE_DOT[tone])}
      />
      <span className="font-mono text-xs text-tertiary">{label}</span>
      {instance !== null && (
        <span className="ml-auto font-mono text-xs text-faint">{instance}</span>
      )}
    </div>
  );
}

export function ConnectionRow() {
  const { socket, connected, instanceChip, stopIntent, stopPhase } =
    useConnectionStatus();
  return (
    <ConnectionRowView
      tone={connectionTone(socket, connected, stopIntent)}
      label={connectionLabel(socket, stopIntent, stopPhase)}
      instance={instanceChip}
    />
  );
}
