/**
 * `ToolConfirmationBanner` (DESIGN_SPEC §3.14).
 *
 * Maps to SSE `confirmation_requested {request_id, tool_name, tool_arguments}`
 * — the same information also arrives on the WS with an `agent_id`, which is
 * the only way to name the agent in the note; without it the copy stays true
 * by saying "the agent".
 *
 * The command box shows the literal argument the user is being asked to
 * approve. It is never summarised.
 */

import { formatToolArguments } from "./format";

export interface ToolConfirmationBannerProps {
  toolName: string;
  toolArguments: unknown;
  /** From the WS twin of this event; `null` when only the SSE frame arrived. */
  agentName?: string | null;
}

export function ToolConfirmationBanner({
  toolName,
  toolArguments,
  agentName = null,
}: ToolConfirmationBannerProps) {
  const command = formatToolArguments(toolArguments);
  const who = agentName ?? "The agent";

  return (
    <section
      role="alert"
      className="mb-[26px] rounded-3xl border border-amber-line bg-amber-surface px-[16px] py-[15px]"
    >
      <p className="mt-0 mb-[9px] font-mono text-2xs-plus tracking-eyebrow-w text-amber-ink uppercase">
        Confirmation required · {toolName}
      </p>
      {command !== "" && (
        <pre className="m-0 overflow-x-auto rounded-md border border-amber-line-2 bg-raised px-[11px] py-[9px] font-mono text-base whitespace-pre-wrap text-ink">
          {command}
        </pre>
      )}
      <p className="mt-[9px] mb-0 text-base-plus leading-[1.5] text-tertiary">
        {who} is blocked on this. Answer in the composer to continue.
      </p>
    </section>
  );
}
