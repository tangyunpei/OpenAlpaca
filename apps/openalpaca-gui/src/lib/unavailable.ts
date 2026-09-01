/**
 * Honest degradation.
 *
 * The design shows surfaces the daemon cannot serve (API_MAP §3). Rather than
 * inventing placeholder rows that look like real data, every such surface goes
 * through this module: the adapter returns a typed `Unavailable`, and the view
 * renders the design's own empty-state copy plus a muted note naming the
 * missing API.
 *
 * The registry below is the single source of truth for those notes **and** for
 * the gap report — the hand-off document is generated from this table, so it
 * cannot drift from what the UI actually says.
 *
 * Rule: never fabricate. If it is not in the registry and not on the wire, it
 * does not render.
 */

export type GapId =
  | "GAP-01"
  | "GAP-02"
  | "GAP-03"
  | "GAP-04"
  | "GAP-05"
  | "GAP-06"
  | "GAP-07"
  | "GAP-08"
  | "GAP-09"
  | "GAP-10"
  | "GAP-11"
  | "GAP-12"
  | "GAP-13"
  | "GAP-14"
  | "GAP-15"
  | "GAP-16"
  | "GAP-17"
  | "GAP-18"
  | "GAP-19"
  | "GAP-20"
  | "GAP-21"
  | "GAP-22"
  | "GAP-23";

export type GapFixSize = "XS" | "S" | "S–M" | "M" | "L";

export interface GapDescriptor {
  id: GapId;
  /** Reads as "{label} not yet available" in the UI note. */
  label: string;
  /** The route or field that does not exist. */
  missingApi: string;
  /** What API_MAP §3 proposes adding to the daemon. */
  proposedEndpoint: string;
  /** Which design surfaces this blocks. */
  blocks: string;
  fixSize: GapFixSize;
  /** Set when "{label} not yet available" would read wrong. */
  noteOverride?: string;
}

export const GAPS: Record<GapId, GapDescriptor> = {
  "GAP-01": {
    id: "GAP-01",
    label: "Allowlist scope",
    missingApi: "POST /v1/chat/confirmations/{id} ignores `approval_scope`",
    proposedEndpoint:
      'POST /v1/chat/confirmations/{id} { approved, approval_scope: "entire_tool" }',
    blocks: 'The confirmation card\'s "Always allow" button',
    fixSize: "XS",
    noteOverride:
      "Always-allow is not yet honoured by the daemon — it approves this call only",
  },
  "GAP-02": {
    id: "GAP-02",
    label: "Steering API",
    missingApi: "no POST /v1/tasks/{id}/steer — steering is chat-text-only",
    proposedEndpoint: "POST /v1/tasks/{id}/steer { message }",
    blocks: "Steer button on a run; composer steer mode",
    fixSize: "S–M",
    noteOverride:
      "Steering has no direct endpoint — sent through chat as `/steer …`",
  },
  "GAP-03": {
    id: "GAP-03",
    label: "Follow-up API",
    missingApi:
      "no /v1/lanes/{lane}/followups routes (storage exists, routes do not)",
    proposedEndpoint: "POST|GET|DELETE /v1/lanes/{lane_key}/followups",
    blocks: "Queue follow-up button; pending follow-ups list",
    fixSize: "M",
  },
  "GAP-04": {
    id: "GAP-04",
    label: "Artifact API",
    missingApi:
      "no /v1/artifacts list route; FileAsset has no task_id/agent_id/kind",
    proposedEndpoint: "GET /v1/artifacts?task_id=&kind=&limit=&offset=",
    blocks:
      "The entire Library view; per-run Files sections; inline artifact cards",
    fixSize: "L",
  },
  "GAP-05": {
    id: "GAP-05",
    label: "Artifact version history",
    missingApi: "nothing versioned exists in storage",
    proposedEndpoint: "GET /v1/artifacts/{id}/versions and /diff?from=&to=",
    blocks: "The History and Diff tabs; `v2 of 2` stamps",
    fixSize: "L",
  },
  "GAP-06": {
    id: "GAP-06",
    label: "Re-run and Start actions",
    missingApi: "POST /v1/tasks/{id}/action accepts only cancel|pause|resume",
    proposedEndpoint:
      'POST /v1/tasks/{id}/action { action: "rerun" | "start" }',
    blocks: "Re-run on a terminal run; Start now on a queued run",
    fixSize: "S",
  },
  "GAP-07": {
    id: "GAP-07",
    label: "Task titles on status events",
    missingApi:
      'event_bridge sends `title: ""` on every task_status but TaskCreated',
    proposedEndpoint:
      "carry the title through the SystemEvent → ServerEvent bridge",
    blocks: "Live run titles without an N+1 refetch",
    fixSize: "XS",
    noteOverride:
      "Live status events omit the run title — titles are refetched",
  },
  "GAP-08": {
    id: "GAP-08",
    label: "Cost reporting",
    missingApi:
      "daily_cost_usd is hardcoded 0.0; no per-task cost; no cost cap served",
    proposedEndpoint:
      "GET /v1/usage/summary?window=today; GET /v1/llm/usage?task_id=",
    blocks: "Per-run $ figures, today's spend, the spend cap",
    fixSize: "S",
  },
  "GAP-09": {
    id: "GAP-09",
    label: "Subagent timeline",
    missingApi: "agent_task_history has no started_at, label, or detail",
    proposedEndpoint: "GET /v1/tasks/{id}/timeline + a SubagentSpan event",
    blocks: "The Parallel work swimlanes",
    fixSize: "L",
  },
  "GAP-10": {
    id: "GAP-10",
    label: "Per-run event log",
    missingApi:
      "event_log has no task_id column; /v1/events/history filters by agent_id only",
    proposedEndpoint: "GET /v1/events/history?task_id=&before=&event_type=",
    blocks: "The run detail's Event log",
    fixSize: "M",
  },
  "GAP-11": {
    id: "GAP-11",
    label: "Direct artifact content URLs",
    missingApi:
      "GET /v1/files/{id}/content is Bearer-only, so <img>/<iframe> cannot load it",
    proposedEndpoint: "GET /v1/files/{id}/content?token=",
    blocks: "Inline image and HTML previews",
    fixSize: "S",
    noteOverride:
      "Previews are fetched into memory because the content route is header-authenticated",
  },
  "GAP-12": {
    id: "GAP-12",
    label: "Server-side pins",
    missingApi: "no pinned column anywhere in storage",
    proposedEndpoint: "PUT /v1/artifacts/{id}/pin — or keep pins client-side",
    blocks: "Nothing: pins are per-machine and live in localStorage",
    fixSize: "XS",
    noteOverride: "Pins are stored on this machine only",
  },
  "GAP-13": {
    id: "GAP-13",
    label: "Per-chat model override",
    missingApi:
      "POST /v1/chat takes no model; the only writable setting is global",
    proposedEndpoint:
      "POST /v1/chat { model } or PUT /v1/lanes/{lane}/preferences",
    blocks: "The composer's model picker being conversation-scoped",
    fixSize: "M",
    noteOverride:
      "Changing the model here changes the daemon default for every client",
  },
  "GAP-14": {
    id: "GAP-14",
    label: "Daemon status detail",
    missingApi: "/v1/health returns status/version/pid/instance_id only",
    proposedEndpoint:
      "GET /v1/status with started_at, uptime_secs, schema_version, log_path",
    blocks: "uptime, Schema vNN, Copy log path",
    fixSize: "S",
  },
  "GAP-15": {
    id: "GAP-15",
    label: "Provider enable/disable",
    missingApi:
      "no provider-enable route; removing every key is the only off switch",
    proposedEndpoint: "PUT /v1/settings/llm/providers/{provider}/enabled",
    blocks: "The per-provider toggle in Models & keys",
    fixSize: "S",
  },
  "GAP-16": {
    id: "GAP-16",
    label: "Identity endpoint",
    missingApi: "no route returns the default lane key or local_user_id",
    proposedEndpoint: "GET /v1/me",
    blocks: "Loading a transcript before the first send",
    fixSize: "S",
    noteOverride: "The default lane is learned from the first reply",
  },
  "GAP-17": {
    id: "GAP-17",
    label: "Connector detail",
    missingApi: "GET /v1/connectors returns id/name/status/configured only",
    proposedEndpoint: "GET /v1/connectors with source, registered, calls_7d",
    blocks: "Call counts, the `unwired` badge, Connect service",
    fixSize: "M",
  },
  "GAP-18": {
    id: "GAP-18",
    label: "Tool and skill catalog",
    missingApi:
      "neither the tool registry nor the skill catalog has an HTTP listing",
    proposedEndpoint: "GET /v1/tools and GET /v1/skills",
    blocks: "The Settings → Skills rows (name, description, asks, enabled)",
    fixSize: "M",
  },
  "GAP-19": {
    id: "GAP-19",
    label: "Plugin install",
    missingApi:
      "no install route; plugins are dropped into the plugins dir by hand",
    proposedEndpoint: "POST /v1/plugins/install { source, path }",
    blocks: "Install plugin",
    fixSize: "M",
  },
  "GAP-20": {
    id: "GAP-20",
    label: "Agent template metrics",
    missingApi: "TemplateResponse has no run counts and no enabled flag",
    proposedEndpoint:
      "GET /v1/agent-templates?window=7d; PUT /v1/agent-templates/{id}/enabled",
    blocks: "`12 runs 7d` per template; the per-template toggle",
    fixSize: "M",
  },
  "GAP-21": {
    id: "GAP-21",
    label: "Conversation rename and delete",
    missingApi: "only two conversation routes exist, both GET",
    proposedEndpoint:
      "PATCH /v1/conversations/{id}; DELETE /v1/conversations/{id}",
    blocks: "Renaming or removing a stored lane",
    fixSize: "S",
  },
  "GAP-22": {
    id: "GAP-22",
    label: "Timestamps on plugin events",
    missingApi: "six plugin_* ServerEvent variants carry no ts or instance_id",
    proposedEndpoint: "add ts and instance_id to the six plugin variants",
    blocks: "Ordering plugin rows in the event log",
    fixSize: "XS",
    noteOverride: "Plugin events arrive without a timestamp",
  },
  "GAP-23": {
    id: "GAP-23",
    label: "Message → run links",
    missingApi: "ConversationMessage has no task_id and no artifact refs",
    proposedEndpoint: "add task_id and artifact_ids to ConversationMessage",
    blocks: "Rebuilding run-report and artifact cards after a reload",
    fixSize: "M",
  },
};

// ── Result type ─────────────────────────────────────────────────────────────

export interface Available<T> {
  available: true;
  data: T;
}

export interface Unavailable {
  available: false;
  /** The gap this surface is waiting on. */
  gap: GapDescriptor;
  /** Human sentence for the muted note under the empty state. */
  reason: string;
  /** The specific route/field that is missing. */
  missingApi: string;
}

export type Availability<T> = Available<T> | Unavailable;

export function available<T>(data: T): Available<T> {
  return { available: true, data };
}

/** The note a view shows beside the design's own empty-state copy. */
export function gapNote(gap: GapDescriptor): string {
  return gap.noteOverride ?? `${gap.label} not yet available`;
}

/**
 * The fuller note an empty state shows: what is missing, and what the daemon
 * would have to grow for the surface to work. Every view uses this one string
 * so the same gap never reads two different ways.
 */
export function gapDetail(result: Unavailable): string {
  return `${result.reason} — proposed ${result.gap.proposedEndpoint}`;
}

export function unavailable(id: GapId, reason?: string): Unavailable {
  const gap = GAPS[id];
  return {
    available: false,
    gap,
    reason: reason ?? gapNote(gap),
    missingApi: gap.missingApi,
  };
}

export function isAvailable<T>(
  result: Availability<T>,
): result is Available<T> {
  return result.available;
}

export function unwrapOr<T>(result: Availability<T>, fallback: T): T {
  return result.available ? result.data : fallback;
}

/** Every gap, ordered by id — the source for the generated hand-off report. */
export function listGaps(): GapDescriptor[] {
  return Object.values(GAPS).sort((a, b) => a.id.localeCompare(b.id));
}

/** Gaps that leave a design surface with no data at all. */
export function listBlockingGaps(): GapDescriptor[] {
  return listGaps().filter((gap) => gap.noteOverride === undefined);
}
