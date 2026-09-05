/**
 * One extension row, rendered from ledger state (ADR-030 §9.2).
 *
 * This is the whole of the design's row table as a pure function, so every
 * state can be asserted without mounting React — and so the section holds no
 * second opinion about what a state means.
 *
 * Two rules the table turns on:
 *
 *  * **The toggle binds to `record.enabled`, never to the state word.** A
 *    plugin that is enabled but crashed, loading or needs-config renders the
 *    switch **ON** with a failure tag. Anything else would lie about what the
 *    owner asked for and make Retry nonsensical. (The old panel computed
 *    `checked={word === "running"}`, so clicking it fired `enable` on
 *    something already enabled.)
 *  * **Tone carries the actionable / not-actionable split** — `asks` means
 *    *you* can fix it, `warn` means *it* is broken — driven by the API's
 *    `actionable` boolean, not by matching reason strings here.
 *
 * The location-bearing states name where the bit lives (X-10): that is what
 * teaches the declaration/disposition model without documentation.
 */

import type { TagTone } from "@/components/ui";
import type { ExtensionRow } from "@/lib/api/types";

import { whenStamp } from "./format";

/** Primary affordances. `reload` is deliberately not one — see `menu`. */
export type ExtensionAction =
  "approve" | "deny" | "retry" | "remove" | "configure";

export interface ExtensionRowView {
  /** The state word itself — §9.2's tag text, kept as the row's own word. */
  tag: string;
  tone: TagTone;
  /** `none` = consent pre-empts the switch; a switch would misrepresent it. */
  control: "toggle" | "none";
  /** Always `record.enabled`. */
  toggleChecked: boolean;
  toggleDisabled: boolean;
  /** Surfaced as the control's `title` when it cannot be driven. */
  disabledReason?: string;
  description: string;
  /** The mono second line: where the bit lives, or what the row asks for. */
  secondary: string | null;
  actions: ExtensionAction[];
  /** Overflow-menu items; a reload is not a primary control. */
  menu: "reload"[];
  /** Degraded first (G-4): 0 failed/unapproved/orphaned, 1 live, 2 disabled. */
  rank: 0 | 1 | 2;
  /** `hint` on a `needs_authorization` row — the URL the owner must visit. */
  authorizeUrl: string | null;
}

const plural = (n: number, one: string, many = `${one}s`) =>
  `${n} ${n === 1 ? one : many}`;

/** Where the **disposition bit** lives, per kind (§5, §9.2). */
export function dispositionLocation(row: ExtensionRow): string {
  return row.kind === "mcp"
    ? `config/mcp.toml → [servers.${row.id}] enabled = false`
    : "plugins/.permissions.toml";
}

/** Where the **declaration** lives — what an orphan is missing. */
export function declarationLocation(row: ExtensionRow): string {
  return row.kind === "mcp"
    ? `config/mcp.toml → [servers.${row.id}]`
    : `plugins/${row.id}/plugin.toml`;
}

/** Where a plugin's own configuration lives — what `config_invalid` names. */
function configLocation(row: ExtensionRow): string {
  return row.kind === "mcp"
    ? `config/mcp.toml → [servers.${row.id}]`
    : `plugins/.config/${row.id}.toml`;
}

/**
 * The `unapproved` suffix. The bit is real even while no switch is drawn (§4),
 * so the row says which way it is pointing rather than hiding it — and says
 * nothing at all when nobody can read it.
 */
function approvalSuffix(row: ExtensionRow): string {
  if (row.enabled === null) return "";
  return row.enabled ? " — starts on approval" : " — stays off after approval";
}

/** What an `enabled` row is actually serving. */
function contributions(row: ExtensionRow): string {
  const parts = [plural(row.tools.length, "tool")];
  if (row.kind === "mcp" && row.transport !== null) parts.push(row.transport);
  if (row.skills.length > 0) parts.push(plural(row.skills.length, "skill"));
  if (row.agents.length > 0) parts.push(plural(row.agents.length, "agent"));
  if (row.connector !== null) parts.push(`connector ${row.connector}`);
  if (row.provider !== null) parts.push(`provider ${row.provider}`);
  return parts.join(" · ");
}

/** The unreadable-bit rows of §4: `enabled` is `null` and every verb is 409. */
const UNREADABLE =
  "the daemon cannot read this extension's on/off setting, so it will not change it";

export function extensionRowView(row: ExtensionRow): ExtensionRowView {
  const view = describe(row);
  // §4/§8: a `null` bit is unknown, not `false`. Drawing a live switch over it
  // would repeat exactly the lie this commit fixes, so the control is inert
  // and says why.
  if (row.enabled === null && view.control === "toggle") {
    return { ...view, toggleDisabled: true, disabledReason: UNREADABLE };
  }
  return view;
}

function describe(row: ExtensionRow): ExtensionRowView {
  const on = row.enabled ?? false;
  const base = {
    control: "toggle" as const,
    toggleChecked: on,
    toggleDisabled: false,
    secondary: null,
    actions: [] as ExtensionAction[],
    menu: [] as "reload"[],
    authorizeUrl: null,
  };

  switch (row.state) {
    case "enabled":
      return {
        ...base,
        tag: "active",
        tone: "live",
        description: contributions(row),
        rank: 1,
        // Apply an edited declaration or a rotated credential. Not a primary
        // control: nothing is wrong, so nothing should ask to be clicked.
        menu: ["reload"],
      };

    case "disabled":
      return {
        ...base,
        tag: "disabled",
        tone: "neutral",
        description: "Turned off",
        secondary: dispositionLocation(row),
        rank: 2,
      };

    case "enabling":
    case "disabling":
      return {
        ...base,
        tag: "loading",
        tone: "neutral",
        toggleDisabled: true,
        disabledReason:
          row.state === "enabling" ? "connecting…" : "shutting down…",
        description:
          row.state === "enabling" ? "Connecting…" : "Shutting down…",
        rank: 1,
      };

    case "unapproved":
      return unapproved(row, base);

    case "failed":
      return failed(row, base);

    case "orphaned":
      return {
        ...base,
        tag: "orphaned",
        tone: "warn",
        toggleDisabled: true,
        disabledReason: "the declaration is gone — nothing to turn on",
        description: `declaration not found at ${declarationLocation(row)}`,
        // Removing the entry is a plugin-store operation; an MCP row's
        // disposition lives in the file that is missing.
        actions: row.kind === "plugin" ? ["remove"] : [],
        rank: 0,
      };
  }
}

type Base = Omit<
  ExtensionRowView,
  "tag" | "tone" | "description" | "rank" | "disabledReason"
>;

/** Consent pre-empts the switch: a switch would misrepresent the gate. */
function unapproved(row: ExtensionRow, base: Base): ExtensionRowView {
  const shared = {
    ...base,
    control: "none" as const,
    secondary: dispositionLocation(row),
    rank: 0 as const,
  };

  if (row.reason === "denied") {
    return {
      ...shared,
      tag: "denied",
      tone: "neutral",
      description: `You denied this plugin${approvalSuffix(row)}`,
      actions: ["approve"],
    };
  }

  if (row.reason === "capabilities_grew") {
    // The delta, not the whole list — that is the only thing that changed.
    const added = row.added_capabilities.join(", ");
    return {
      ...shared,
      tag: "waiting-approval",
      tone: "asks",
      description: `Now also asks for: ${added}${approvalSuffix(row)}`,
      actions: ["approve", "deny"],
    };
  }

  // `never_seen`. Listed from the row's **static** `declared` object, never
  // from runtime `tools`, which is empty here (X-19).
  const declared = row.declared?.capabilities ?? [];
  const asks =
    declared.length > 0
      ? `Asks for: ${declared.join(", ")}`
      : "Declares no capabilities";
  return {
    ...shared,
    tag: "waiting-approval",
    tone: "asks",
    description: `${asks}${approvalSuffix(row)}`,
    actions: ["approve", "deny"],
  };
}

/**
 * `enabled` + `warn` answers the "enabled but not working" question directly:
 * the switch stays ON.
 */
function failed(row: ExtensionRow, base: Base): ExtensionRowView {
  // The API's boolean, not a reason match here (§9.2).
  const tone: TagTone = row.actionable ? "asks" : "warn";
  const shared = { ...base, tone, rank: 0 as const };

  switch (row.reason) {
    case "needs_authorization":
      return {
        ...shared,
        tag: "needs-auth",
        description: row.detail ?? "Authorization required",
        authorizeUrl: row.hint,
      };

    case "needs_config":
      return {
        ...shared,
        tag: "needs-config",
        description:
          row.missing_config_keys.length > 0
            ? `Missing: ${row.missing_config_keys.join(", ")}`
            : (row.detail ?? "Configuration required"),
        secondary: configLocation(row),
        // The config route is plugins-only; `kind=mcp` is
        // `409 unsupported_for_kind`, so the row names the file instead.
        actions: row.kind === "plugin" ? ["configure"] : [],
      };

    case "config_invalid":
      return {
        ...shared,
        tag: "config-invalid",
        // The parse error, quoted and never interpreted.
        description: quoted(row.detail) ?? "The declaration could not be read",
        secondary: configLocation(row),
      };

    default:
      // `unreachable` | `crashed`, and anything a newer daemon adds: not
      // actionable, so `warn` and a Retry.
      return {
        ...shared,
        tag: "crashed",
        description: `${quoted(row.detail) ?? "Stopped running"} · since ${whenStamp(row.since)}`,
        // Retry is `reload`, which from `Failed` is `enable` in effect —
        // one button, one verb (§3.4.1).
        actions: ["retry"],
      };
  }
}

function quoted(detail: string | null): string | null {
  return detail === null || detail.length === 0 ? null : `"${detail}"`;
}

/**
 * Degraded rows first, disabled ones last (G-4). Within a rank the daemon's
 * own `(kind, id)` order is preserved, so the list does not reshuffle on a
 * refetch.
 */
export function orderExtensions(
  rows: readonly ExtensionRow[],
): Array<{ row: ExtensionRow; view: ExtensionRowView }> {
  return rows
    .map((row) => ({ row, view: extensionRowView(row) }))
    .sort((a, b) => a.view.rank - b.view.rank);
}

/**
 * The flat `{"error": "<word>"}` envelope (§8, R20) as row copy.
 *
 * `parseErrorPayload` hands the word through as the message, so this is the
 * one place that knows what each word means to a person.
 */
export function extensionErrorCopy(message: string): string {
  switch (message.trim()) {
    case "not_loaded":
      return "Nothing is loaded to reload — turn it on instead.";
    case "store_unreadable":
      return "The daemon cannot read the file that holds this setting, so it changed nothing.";
    case "orphaned":
      return "The declaration is gone, so only Remove is available.";
    case "not_orphaned":
      return "This extension is still declared, so it cannot be removed.";
    case "unsupported_for_kind":
      return "That action does not apply to this kind of extension.";
    default:
      return message;
  }
}
