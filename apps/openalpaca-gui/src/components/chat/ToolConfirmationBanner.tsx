/**
 * `ToolConfirmationBanner` (DESIGN_SPEC §3.14).
 *
 * Maps to SSE `confirmation_requested {request_id, tool_name, tool_arguments,
 * preview, per_call}` — the same information also arrives on the WS with an
 * `agent_id`, which is the only way to name the agent in the note; without it
 * the copy stays true by saying "the agent".
 *
 * The command box shows the literal argument the user is being asked to
 * approve. It is never summarised. The one exception is a **daemon-authored
 * preview**, shown above the arguments and labelled as OpenAlpaca's own text,
 * with the arguments staying: `memory_confirm`'s (design 09 §4.4, whose
 * arguments are three opaque ids), composed from the notes; and, since
 * storage design Stage 2 (R34, R49), `shell_execute`'s, `file_write`'s,
 * `file_edit`'s and `send`'s, naming the folder a command or a write acts in.
 * `file_edit`'s quotes `old_string` and `new_string` from the call — already
 * escaped (`<U+XXXX>`) and cut to a budget by the daemon, with its own count
 * and cut lines — so it is still drawn as written.
 *
 * **Hidden characters** (`lib/hidden-chars`). The tool name, the arguments
 * and the agent's name come from the model, an MCP server or a plugin, and
 * `JSON.stringify` passes a bidi override or a zero-width space through
 * raw: the card would read differently from what runs. Each such character
 * is shown as a styled `<U+XXXX>` — the name and the agent on one line, so
 * a line break in them is shown too — and one line directly under the
 * arguments says how many the request holds, in the words every other
 * approval surface uses. The daemon's `preview` is its own prose and is
 * shown as it wrote it. The arguments are laid out left to right and
 * bidi-isolated, so right-to-left letters in a value cannot reorder the
 * card around them. The card wraps a run with no break opportunity in it (a
 * URL's query string, base64, a padded command) anywhere: under `pre-wrap`
 * alone such a run ran past the box's edge, behind a scrollbar macOS does
 * not draw, and its tail was off screen. Display only: what is answered is
 * the request id.
 *
 * **Several pending** (`lib/confirmation-queue`). The composer answers the
 * oldest prompt while the transcript scrolls to the newest card, so with more
 * than one pending the card it answers is outlined in red, like the
 * composer's answer box, and says so; every other card says it waits.
 */

import { EscapedText } from "@/components/ui";
import { cn } from "@/lib/cn";
import { hiddenNotice } from "@/lib/hidden-chars";

import { escapeConfirmation } from "./confirmation-escape";

export interface ToolConfirmationBannerProps {
  toolName: string;
  toolArguments: unknown;
  /** The daemon's own text for the card; `null`/absent for the ordinary one. */
  preview?: string | null;
  /** From the WS twin of this event; `null` when only the SSE frame arrived. */
  agentName?: string | null;
  /**
   * Its place in the order the composer answers prompts (1 for the one
   * Approve answers) and how many are pending. With more than one, the card
   * says which the composer answers (`lib/confirmation-queue`).
   */
  queue?: { position: number; total: number } | null;
}

export function ToolConfirmationBanner({
  toolName,
  toolArguments,
  preview = null,
  agentName = null,
  queue = null,
}: ToolConfirmationBannerProps) {
  const { name, command, agent, hidden } = escapeConfirmation({
    toolName,
    toolArguments,
    agentName,
  });
  // The whole request, whichever line it hides in: what the person approves
  // is all three.
  const notice = hiddenNotice(hidden);
  const queued = queue !== null && queue.total > 1;
  const answeredNext = queued && queue.position === 1;

  return (
    <section
      role="alert"
      data-answered-next={answeredNext ? "" : undefined}
      className={cn(
        "mb-[26px] rounded-3xl border bg-amber-surface px-[16px] py-[15px] wrap-anywhere",
        // The composer's answer box is outlined in red: so is the card it
        // answers, when there is more than one to tell apart.
        answeredNext ? "border-red" : "border-amber-line",
      )}
    >
      <p className="mt-0 mb-[9px] font-mono text-2xs-plus tracking-eyebrow-w text-amber-ink uppercase">
        Confirmation required ·{" "}
        {/* As written: capitals fold names that differ (a dotless i, an i). */}
        <span className="normal-case">
          <EscapedText escaped={name} />
        </span>
      </p>
      {preview !== null && preview !== "" && (
        <div className="mb-[9px]">
          <p className="mt-0 mb-[5px] font-mono text-2xs-plus tracking-eyebrow-w text-tertiary uppercase">
            From OpenAlpaca
          </p>
          <p className="m-0 text-base-plus leading-[1.5] whitespace-pre-wrap text-ink">
            {preview}
          </p>
        </div>
      )}
      {command.text !== "" && (
        <pre
          dir="ltr"
          className="m-0 overflow-x-auto rounded-md border border-amber-line-2 bg-raised px-[11px] py-[9px] font-mono text-base whitespace-pre-wrap text-ink [unicode-bidi:isolate]"
        >
          <EscapedText escaped={command} />
        </pre>
      )}
      {notice !== null && (
        <p className="mt-[7px] mb-0 text-base leading-[1.5] text-red-ink">
          {notice}
        </p>
      )}
      <p className="mt-[9px] mb-0 text-base-plus leading-[1.5] text-tertiary">
        {agent === null ? "The agent" : <EscapedText escaped={agent} />} is
        blocked on this.{" "}
        {!queued
          ? "Answer in the composer to continue."
          : answeredNext
            ? `Approve and Deny in the composer answer this card first — 1 of ${queue.total} pending, oldest first.`
            : `${queue.position} of ${queue.total} pending — the composer answers the card outlined in red first.`}
      </p>
    </section>
  );
}
