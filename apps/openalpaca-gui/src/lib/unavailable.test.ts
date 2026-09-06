import { describe, expect, it } from "vitest";

import {
  GAPS,
  gapNote,
  isAvailable,
  listGaps,
  unavailable,
  unwrapOr,
} from "./unavailable";

describe("gap registry", () => {
  it("keys every entry by its own id, so the report cannot drift", () => {
    for (const [key, gap] of Object.entries(GAPS)) {
      expect(gap.id).toBe(key);
      expect(gap.label.length).toBeGreaterThan(0);
      expect(gap.missingApi.length).toBeGreaterThan(0);
      expect(gap.proposedEndpoint.length).toBeGreaterThan(0);
    }
  });

  // GAP-01/07/08a/08b/16 retired in Phase 0; GAP-19 became GAP-24 (widened to
  // both extension kinds) and GAP-22 closed with the six `plugin_*` variants
  // C7 deleted. Phase 3 closed four more: the artifact resource (GAP-04), its
  // versions and diff (GAP-05), browser-loadable content (GAP-11) and
  // server-side pins (GAP-12). Phase 4 closed two: the subagent timeline
  // (GAP-09), served by `subagent_span` + `GET /v1/tasks/{id}/timeline`, and
  // the per-run event log (GAP-10), served by `event_log.task_id` +
  // `GET /v1/events/history?task_id=`. Phase 5 closed two: steering (GAP-02),
  // where `POST /v1/tasks/{id}/steer` addresses a run rather than a lane, and
  // the follow-up queue (GAP-03), where `/v1/lanes/{lane_key}/followups` reads,
  // writes and cancels it — and re-run/start (GAP-06), where
  // `POST /v1/tasks/{id}/rerun` dispatches a new run from a finished one's goal
  // and `{ action: "start" }` runs a queued row under its own id. Phase 6
  // closed the message → run link (GAP-23): migration 038's
  // `conversation_messages.task_id` plus the report's `role='artifact'` links,
  // both served by the two history routes. Phase 7a closed conversation
  // rename/delete (GAP-21): migration 039's `PATCH`/`DELETE /v1/sessions/{id}`
  // and the chat view's conversation sidebar, which calls them. Phase 8 closed
  // the daemon status detail (GAP-14): `GET /v1/status` carries `started_at`,
  // `uptime_secs`, `schema_version` and `log_path`, so uptime, `Schema vNN` and
  // `Copy log path` are all served. GAP-20 is *narrowed*, not closed: its run
  // counts are served now, its toggle is not — so the count is 7.
  it("covers the 7 gaps still open from API_MAP §3", () => {
    expect(listGaps()).toHaveLength(7);
    expect(listGaps()[0]?.id).toBe("GAP-08c");
    expect(listGaps().at(-1)?.id).toBe("GAP-24");
    for (const closed of [
      "GAP-02",
      "GAP-03",
      "GAP-04",
      "GAP-06",
      "GAP-05",
      "GAP-09",
      "GAP-10",
      "GAP-11",
      "GAP-12",
      "GAP-14",
      "GAP-19",
      "GAP-21",
      "GAP-22",
      "GAP-23",
    ]) {
      expect(listGaps().map((gap) => gap.id)).not.toContain(closed);
    }
  });

  // §9.1: the tool half is served by `GET /v1/tools`; the `enabled` half of
  // the claim is struck, because that field is derived from the extension row
  // and does not exist per tool.
  it("keeps only the skill half of GAP-18, with no claim about `enabled`", () => {
    expect(GAPS["GAP-18"].proposedEndpoint).toBe("GET /v1/skills");
    expect(GAPS["GAP-18"].missingApi).not.toMatch(/tool registry/);
    expect(GAPS["GAP-18"].blocks).not.toMatch(/enabled/);
  });

  // P8's counterpart: run counts are served (`run_count`/`last_run_at` off
  // `subagent_span`), so GAP-20 keeps only the toggle. It stays in the
  // registry because the disabled switch in Settings → Agents still needs a
  // note that names why it cannot be operated.
  it("keeps only the enable/disable half of GAP-20, with no claim about counts", () => {
    expect(GAPS["GAP-20"].missingApi).not.toMatch(/run count/i);
    expect(GAPS["GAP-20"].blocks).not.toMatch(/runs/);
    expect(GAPS["GAP-20"].proposedEndpoint).not.toMatch(/window/);
    expect(gapNote(GAPS["GAP-20"])).toMatch(/enabled flag/);
  });
});

describe("Unavailable results", () => {
  it("carries a note that names the missing API", () => {
    const result = unavailable("GAP-24");

    expect(isAvailable(result)).toBe(false);
    expect(result.reason).toBe(
      "Extension install / uninstall not yet available",
    );
    expect(result.missingApi).toContain("no install or uninstall route");
    expect(result.gap.id).toBe("GAP-24");
  });

  it("uses the override phrasing where the generic sentence would read wrong", () => {
    expect(gapNote(GAPS["GAP-13"])).toBe(
      "Changing the model here changes the daemon default for every client",
    );
  });

  it("accepts a caller-supplied reason", () => {
    expect(
      unavailable("GAP-17", "No call counts for this connector").reason,
    ).toBe("No call counts for this connector");
  });

  it("unwraps to the fallback rather than to fabricated data", () => {
    expect(unwrapOr(unavailable("GAP-18"), [])).toEqual([]);
  });
});
