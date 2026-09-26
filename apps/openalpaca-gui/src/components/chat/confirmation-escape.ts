/**
 * A pending confirmation as the approval surfaces show it (`lib/hidden-chars`).
 *
 * The card draws all three fields — the tool's name and the agent's on one
 * line each, the arguments as `formatToolArguments` lays them out — and says
 * how many hidden characters the request holds. The composer's bar, the
 * palette's Approve row and the run card show the name alone, and say the
 * same number beside it (`hiddenOnCard`). One function computes it for all of
 * them, so the card and the controls that answer it cannot disagree.
 */

import {
  escapeHidden,
  escapeHiddenLine,
  type Escaped,
} from "@/lib/hidden-chars";

import { formatToolArguments } from "./format";

export interface ConfirmationFields {
  toolName: string;
  toolArguments: unknown;
  /** The agent's name as the card shows it; `null` when none is known. */
  agentName?: string | null;
}

export interface EscapedConfirmation {
  name: Escaped;
  command: Escaped;
  agent: Escaped | null;
  /** The whole request's count: name, arguments and agent. */
  hidden: number;
}

export function escapeConfirmation({
  toolName,
  toolArguments,
  agentName = null,
}: ConfirmationFields): EscapedConfirmation {
  const name = escapeHiddenLine(toolName);
  const command = escapeHidden(formatToolArguments(toolArguments));
  const agent = agentName === null ? null : escapeHiddenLine(agentName);
  return {
    name,
    command,
    agent,
    hidden: name.hidden + command.hidden + (agent?.hidden ?? 0),
  };
}
