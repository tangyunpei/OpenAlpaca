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
  | "GAP-02"
  | "GAP-03"
  | "GAP-04"
  | "GAP-05"
  | "GAP-06"
  | "GAP-08c"
  | "GAP-09"
  | "GAP-10"
  | "GAP-11"
  | "GAP-12"
  | "GAP-13"
  | "GAP-14"
  | "GAP-15"
  | "GAP-17"
  | "GAP-18"
  | "GAP-20"
  | "GAP-21"
  | "GAP-23"
  | "GAP-24";

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
  "GAP-08c": {
    id: "GAP-08c",
    label: "Usage summary",
    missingApi:
      "no GET /v1/usage/summary; the per-provider token figure sums the lifetime all_provider_usage(), not today; no cost cap is served",
    proposedEndpoint:
      "GET /v1/usage/summary?window=today → { date, total_cost_usd, by_provider[], caps: { workflow_max_cost_usd, agent_max_cost_usd } }",
    blocks: "Today's per-provider token figure; a served spend-cap line",
    fixSize: "S",
    noteOverride:
      "Spend is not capped daily by design — caps are per workflow, so the progress bar has no denominator to draw against",
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
  "GAP-17": {
    id: "GAP-17",
    label: "Connector detail",
    missingApi: "GET /v1/connectors returns id/name/status/configured only",
    proposedEndpoint: "GET /v1/connectors with source, registered, calls_7d",
    blocks: "Call counts, the `unwired` badge, Connect service",
    fixSize: "M",
  },
  // The tool half closed with `GET /v1/tools` (ADR-030 §8): the Settings →
  // Tools rows are real, and `enabled` is struck from the claim entirely —
  // that field is derived from the extension row and does not exist per tool.
  // The skill half is still open, which is why the health rows read as ids.
  "GAP-18": {
    id: "GAP-18",
    label: "Skill catalog",
    missingApi:
      "GET /v1/skills/health is the only skill route — no listing carries a skill's name, description or triggers",
    proposedEndpoint: "GET /v1/skills",
    blocks: "Naming a skill in Settings → Tools; the health rows show ids",
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
  // GAP-22 (the six `plugin_*` variants carrying no `ts`/`instance_id`) is
  // closed: C7 deleted those variants with `/v1/plugins*`, and the extension
  // family that replaced them carries both on every frame (ADR-030 §7.3).
  "GAP-23": {
    id: "GAP-23",
    label: "Message → run links",
    missingApi: "ConversationMessage has no task_id and no artifact refs",
    proposedEndpoint: "add task_id and artifact_ids to ConversationMessage",
    blocks: "Rebuilding run-report and artifact cards after a reload",
    fixSize: "M",
  },
  // Was GAP-19 ("plugin install"), widened to both extension kinds: the same
  // mechanism is missing for an MCP server, which had no gap id at all
  // (ADR-030 §9.1). `DELETE /v1/extensions/plugin/{id}` removes an orphan's
  // permissions entry — it is not an uninstall and never touches a directory.
  "GAP-24": {
    id: "GAP-24",
    label: "Extension install / uninstall",
    missingApi:
      "no install or uninstall route for either kind; a plugin is a directory copied into the plugins root, an MCP server a hand-written [servers.<name>] block",
    proposedEndpoint:
      "POST /v1/extensions/{kind} { source } and DELETE /v1/extensions/{kind}/{id}?uninstall=true",
    blocks: "Add extension; removing an installed plugin or MCP server",
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
