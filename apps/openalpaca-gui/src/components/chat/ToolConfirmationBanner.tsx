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
 * preview** (design 09 §4.4, `memory_confirm`'s card, whose arguments are
 * three opaque ids): it is shown above the arguments, labelled as
 * OpenAlpaca's own text — never the model's — and the arguments stay.
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
 */

import { EscapedText } from "@/components/ui";
import {
  escapeHidden,
  escapeHiddenLine,
  hiddenNotice,
} from "@/lib/hidden-chars";

import { formatToolArguments } from "./format";

export interface ToolConfirmationBannerProps {
  toolName: string;
  toolArguments: unknown;
  /** The daemon's own text for the card; `null`/absent for the ordinary one. */
  preview?: string | null;
  /** From the WS twin of this event; `null` when only the SSE frame arrived. */
  agentName?: string | null;
}

export function ToolConfirmationBanner({
  toolName,
  toolArguments,
  preview = null,
  agentName = null,
}: ToolConfirmationBannerProps) {
  const name = escapeHiddenLine(toolName);
  const command = escapeHidden(formatToolArguments(toolArguments));
  const agent = agentName === null ? null : escapeHiddenLine(agentName);
  // The whole request, whichever line it hides in: what the person approves
  // is all three.
  const notice = hiddenNotice(
    name.hidden + command.hidden + (agent?.hidden ?? 0),
  );

  return (
    <section
      role="alert"
      className="mb-[26px] rounded-3xl border border-amber-line bg-amber-surface px-[16px] py-[15px] wrap-anywhere"
    >
      <p className="mt-0 mb-[9px] font-mono text-2xs-plus tracking-eyebrow-w text-amber-ink uppercase">
        Confirmation required · <EscapedText escaped={name} />
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
        blocked on this. Answer in the composer to continue.
      </p>
    </section>
  );
}
