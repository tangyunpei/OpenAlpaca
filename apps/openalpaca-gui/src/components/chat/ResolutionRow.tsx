/**
 * `ResolutionRow` (DESIGN_SPEC §3.15) — the echo of an answered confirmation.
 *
 * The design's copy quotes a specific outcome ("cargo tree returned in 1.4s").
 * That timing is real and available: `tool_executed {tool_name, success,
 * duration_ms}` arrives on the WS once the approved tool runs, so the note is
 * upgraded in place when it lands and stays honest ("waiting for the tool to
 * run…") until then.
 *
 * A prompt nobody answered ends the same way (T1): the daemon's wait runs out,
 * the tool does not run, and the card that was on screen settles into this row
 * rather than staying up for ever. It is a resolution — it just is not an
 * answer.
 */

export type Resolution = "approved" | "denied" | "timed_out";

/** The eyebrow label for each outcome. */
function resolutionLabel(resolution: Resolution): string {
  if (resolution === "approved") return "Approved";
  if (resolution === "denied") return "Denied";
  return "Timed out";
}

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
        {resolutionLabel(resolution)}
      </span>
      <span className="flex-1 text-base-plus text-secondary">{note}</span>
      {time !== null && (
        <span className="shrink-0 font-mono text-xs text-faint">{time}</span>
      )}
    </div>
  );
}

/**
 * What a prompt that ran out its clock says (T1).
 *
 * The daemon waited the policy's confirmation timeout, nobody answered, and it
 * refused the call — so this is a *denial with a different reason*, and the
 * sentence says both halves: it was not answered, and the tool did not run.
 */
export function timedOutResolutionNote(toolName: string): string {
  return `${toolName} timed out — not run. Nobody answered in time, so the agent continued without it.`;
}

/** The note a fresh resolution shows before any `tool_executed` arrives. */
export function pendingResolutionNote(
  resolution: Resolution,
  toolName: string,
): string {
  if (resolution === "timed_out") return timedOutResolutionNote(toolName);
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

/** One `tool_executed` frame, remembered long enough to settle a card. */
export interface ToolRun {
  toolName: string;
  success: boolean;
  /** Already formatted — `1.4s`. */
  duration: string;
  /** When this client saw the frame, in epoch ms. */
  atMs: number;
}

/**
 * The run that settles a resolution card, or `null` while there is none (G6).
 *
 * The two events race, and the wrong order was the common one: the broker
 * releases the tool the instant the answer is posted, so `tool_executed`
 * usually arrives *before* the POST's own response has resolved and put the
 * card on screen. The card therefore never found its upgrade and sat on
 * "waiting for the tool to run…" for the rest of the session.
 *
 * `sinceMs` is when this client sent the answer: only a run of that tool that
 * started after it can be the one it approved, so an earlier call of the same
 * tool cannot be borrowed. The match is by name because that is all the two
 * frames share — a confirmation carries a `request_id`, a `tool_executed`
 * carries none.
 */
export function settlingRun(
  runs: readonly ToolRun[],
  toolName: string,
  sinceMs: number,
): ToolRun | null {
  for (let index = runs.length - 1; index >= 0; index -= 1) {
    const run = runs[index];
    if (run === undefined) continue;
    if (run.toolName === toolName && run.atMs >= sinceMs) return run;
  }
  return null;
}

/**
 * What a freshly answered confirmation says: the outcome when this client has
 * already seen it, the honest "waiting" otherwise.
 */
export function resolutionNote(
  resolution: Resolution,
  toolName: string,
  settled: ToolRun | null,
): string {
  if (resolution === "approved" && settled !== null) {
    return executedResolutionNote(toolName, settled.success, settled.duration);
  }
  return pendingResolutionNote(resolution, toolName);
}
