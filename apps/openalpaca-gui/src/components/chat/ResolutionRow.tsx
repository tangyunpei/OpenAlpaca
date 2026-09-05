/**
 * `ResolutionRow` (DESIGN_SPEC §3.15) — the echo of an answered confirmation.
 *
 * The design's copy quotes a specific outcome ("cargo tree returned in 1.4s").
 * That timing is real and available: `tool_executed {tool_name, success,
 * duration_ms}` arrives on the WS once the approved tool runs, so the note is
 * upgraded in place when it lands and stays honest ("waiting for the tool to
 * run…") until then.
 */

export type Resolution = "approved" | "denied";

export interface ResolutionRowProps {
  resolution: Resolution;
  /** The sentence after the label. */
  note: string;
  /** `14:23`, when known. */
  time?: string | null;
}

export function ResolutionRow({
  resolution,
  note,
  time = null,
}: ResolutionRowProps) {
  return (
    <div className="mb-[26px] flex items-center gap-[9px] rounded-xl border border-line-subtle bg-muted px-[13px] py-[11px]">
      <span className="shrink-0 font-mono text-2xs-plus tracking-eyebrow text-tertiary uppercase">
        {resolution === "approved" ? "Approved" : "Denied"}
      </span>
      <span className="flex-1 text-base-plus text-secondary">{note}</span>
      {time !== null && (
        <span className="shrink-0 font-mono text-xs text-faint">{time}</span>
      )}
    </div>
  );
}

/** The note a fresh resolution shows before any `tool_executed` arrives. */
export function pendingResolutionNote(
  resolution: Resolution,
  toolName: string,
): string {
  return resolution === "approved"
    ? `${toolName} approved · waiting for the tool to run…`
    : `${toolName} denied · the agent was told to skip it.`;
}

/** The note once `tool_executed` reports how the approved call went. */
export function executedResolutionNote(
  toolName: string,
  success: boolean,
  duration: string,
): string {
  return success
    ? `${toolName} approved · returned in ${duration}, the agent resumed.`
    : `${toolName} approved · failed after ${duration}, the agent continued without it.`;
}
