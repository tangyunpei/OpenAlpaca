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
  // `Copy log path` are all served — and the catalog (GAP-18), whose tool half
  // was `GET /v1/tools` and whose skill half is now `GET /v1/skills`: the
  // health rows are named from it — and the provider toggle (GAP-15), whose
  // route writes `llm.toml` and then unloads or reloads the provider live.
  // GAP-20 and GAP-17 are *narrowed*, not closed: run counts and connector
  // detail are served now, the template toggle and the `Connect service` add
  // flow are not. Phase 8 item 7 (T50) closed GAP-08c outright —
  // `GET /v1/usage/summary?window=today` serves today's total, its
  // per-provider breakdown and the two caps that actually bound spend — so the
  // count is 4.
  it("covers the 4 gaps still open from API_MAP §3", () => {
    expect(listGaps()).toHaveLength(4);
    expect(listGaps()[0]?.id).toBe("GAP-13");
    expect(listGaps().at(-1)?.id).toBe("GAP-24");
    for (const closed of [
      "GAP-02",
      "GAP-08c",
      "GAP-03",
      "GAP-04",
      "GAP-06",
      "GAP-05",
      "GAP-09",
      "GAP-10",
      "GAP-11",
      "GAP-12",
      "GAP-14",
      "GAP-15",
      "GAP-18",
      "GAP-19",
      "GAP-21",
      "GAP-22",
      "GAP-23",
    ]) {
      expect(listGaps().map((gap) => gap.id)).not.toContain(closed);
    }
  });

  // Phase 8 item 6 (T49) closed the detail half of GAP-17: `source`,
  // `registered` and `messages_7d` are on the wire, and the display name comes
  // off the connector's own factory rather than a two-id `match`. The `unwired`
  // badge was never a daemon gap — it is the client-side join of the extension
  // rows against this list. What is left is `Connect service`.
  it("keeps only the add-flow half of GAP-17, with no claim about counts", () => {
    expect(GAPS["GAP-17"].missingApi).not.toMatch(/id\/name\/status/);
    expect(GAPS["GAP-17"].blocks).not.toMatch(/count/i);
    expect(GAPS["GAP-17"].blocks).not.toMatch(/unwired/i);
    expect(GAPS["GAP-17"].proposedEndpoint).not.toMatch(/calls_7d/);
    expect(gapNote(GAPS["GAP-17"])).toMatch(/no route that adds one/);
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
      unavailable("GAP-17", "No way to add a connector from here").reason,
    ).toBe("No way to add a connector from here");
  });

  it("unwraps to the fallback rather than to fabricated data", () => {
    expect(unwrapOr(unavailable("GAP-17"), [])).toEqual([]);
  });
});
