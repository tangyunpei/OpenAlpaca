/**
 * Test-only builder for one `GET /v1/extensions` row (ADR-030 §8).
 *
 * The row has 23 fields and every state under test differs in two or three of
 * them, so the tests say what a case *is* rather than restating the shape.
 * Defaults are the empty/`null` values the daemon actually serves — `tools` is
 * empty when nothing is running, never a remembered list.
 */

import type { ExtensionRow } from "@/lib/api/types";

export function extensionRow(patch: Partial<ExtensionRow> = {}): ExtensionRow {
  return {
    kind: "plugin",
    id: "example",
    version: null,
    transport: null,
    enabled: true,
    consent: null,
    state: "enabled",
    reason: null,
    actionable: false,
    detail: null,
    hint: null,
    missing_config_keys: [],
    added_capabilities: [],
    tools: [],
    skipped_tools: [],
    withdrawn_by_server: [],
    tools_changed_at: null,
    declared: null,
    skills: [],
    agents: [],
    connector: null,
    provider: null,
    since: "2026-09-01T10:04:00Z",
    ...patch,
  };
}
