/**
 * `ConnectionRow` (DESIGN_SPEC §3.6) — the rail's daemon status line.
 *
 * The design draws one state (green · "connected" · `7f3a`). §3.6 extrapolates
 * the other two colours from the status palette: red for an error, gold while
 * connecting. The label words come from the socket's own state machine
 * (`lib/events.ts`), so nothing here is invented — and the instance id is the
 * real `instanceId`, cut to four characters exactly as the design shows.
 *
 * `ConnectionRowView` is the presentational half; `ConnectionRow` binds it to
 * the live socket + `/v1/health`.
 */

import { useConnectionStatus } from "@/hooks/useConnection";
import type { EventsStatus } from "@/lib/events";
import { cn } from "@/lib/cn";

export type ConnectionTone = "up" | "pending" | "down";

/** Socket state → the dot's colour and the word beside it. */
export function connectionTone(
  status: EventsStatus,
  healthy: boolean,
): ConnectionTone {
  if (status === "connected") return healthy ? "up" : "pending";
  if (status === "connecting" || status === "idle") return "pending";
  return "down";
}

export function connectionLabel(status: EventsStatus): string {
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
  const { socket, connected, instanceChip } = useConnectionStatus();
  return (
    <ConnectionRowView
      tone={connectionTone(socket, connected)}
      label={connectionLabel(socket)}
      instance={instanceChip}
    />
  );
}
