/**
 * The eight Settings sections (DESIGN_SPEC §5.4).
 *
 * The blurbs are the design's own copy, verbatim, with **three** corrections
 * the system itself calls for. A blurb is the first sentence a user reads about
 * a section, so one describing a mechanism the daemon does not have teaches
 * them something false about their own machine:
 *
 *   1. the design writes "Loaded WASM plugins and what each contributes", but
 *      OpenAlpaca's plugins are out-of-process child programs speaking
 *      JSON-RPC 2.0 over stdio (CLAUDE.md, `crates/openalpaca_plugins`) —
 *      there is no WASM anywhere in the system;
 *   2. Connection said "today's spend against the cap". There is no daily
 *      budget and none is to be added (N4): the caps are per workflow and per
 *      agent turn, which is what the panel's own note names, so the blurb says
 *      that instead of pointing at a ceiling today's figure is not measured
 *      against;
 *   3. Conversations said "Stored lanes. Memory compaction runs weekly." The
 *      rows are conversations, not lanes — one lane holds many — and nothing
 *      in the daemon runs a weekly compaction: `GET /v1/sessions` is an
 *      inventory across every lane and that is all this section shows.
 *
 * Two ids changed with ADR-030 §9.1 and still count eight: `skills` → `tools`
 * (the design's "Skills" rows are tools) and `plugins` → `extensions` (MCP
 * servers and plugins are one list under one ENABLE axis).
 */

export const SETTINGS_SECTION_IDS = [
  "connection",
  "models",
  "connectors",
  "tools",
  "extensions",
  "agents",
  "conversations",
  "events",
] as const;

export type SettingsSectionId = (typeof SETTINGS_SECTION_IDS)[number];

export interface SettingsSectionMeta {
  id: SettingsSectionId;
  label: string;
  blurb: string;
  /** The add-bar button label, where the design has one. */
  add?: string;
}

export const SETTINGS_SECTIONS: readonly SettingsSectionMeta[] = [
  {
    id: "connection",
    label: "Connection",
    blurb:
      "Daemon status, endpoint and today's spend; caps are per workflow and per agent turn.",
  },
  {
    id: "models",
    label: "Models & keys",
    blurb:
      "Providers the router can reach, in priority order. Pick a model to make it the chat default.",
    add: "Add provider",
  },
  {
    id: "connectors",
    label: "Connectors",
    blurb: "External services the agents may read and write.",
    add: "Connect service",
  },
  {
    id: "tools",
    label: "Tools",
    // The design calls these rows "Skills"; they are tools (ADR-030 §9.1,
    // API_MAP §2.4) and `GET /v1/tools` is what serves them. Skill *health*
    // keeps a subsection inside this one.
    blurb: "Capabilities the agents can invoke, and whether each asks first.",
  },
  {
    id: "extensions",
    label: "Extensions",
    // Corrected from the design's "Loaded WASM plugins …" — see the file note —
    // and widened from plugins alone to the whole ENABLE axis: MCP servers and
    // plugins are one list, because they carry the same one-bit toggle
    // (ADR-030 §1, §9.2).
    blurb:
      "MCP servers and out-of-process plugins the daemon speaks JSON-RPC to, and what each contributes.",
    add: "Add extension",
  },
  {
    id: "agents",
    label: "Agents",
    blurb: "Templates the orchestrator spawns from.",
  },
  {
    id: "conversations",
    label: "Conversations",
    blurb: "Stored conversations across every lane.",
  },
  {
    id: "events",
    label: "Event log",
    blurb: "Everything the daemon emitted, newest first.",
  },
];

const IDS: readonly string[] = SETTINGS_SECTION_IDS;

/** The store holds a bare string; anything unknown falls back to Connection. */
export function toSectionId(value: string): SettingsSectionId {
  return IDS.includes(value) ? (value as SettingsSectionId) : "connection";
}

export function sectionMeta(id: SettingsSectionId): SettingsSectionMeta {
  const found = SETTINGS_SECTIONS.find((section) => section.id === id);
  // The union guarantees a hit; the assertion keeps the caller free of a null.
  return found as SettingsSectionMeta;
}
