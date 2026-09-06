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
  // server-side pins (GAP-12). Phase 4 closed the subagent timeline (GAP-09):
  // `subagent_span` + `GET /v1/tasks/{id}/timeline` serve the swimlanes.
  it("covers the 14 gaps still open from API_MAP §3", () => {
    expect(listGaps()).toHaveLength(14);
    expect(listGaps()[0]?.id).toBe("GAP-02");
    expect(listGaps().at(-1)?.id).toBe("GAP-24");
    for (const closed of [
      "GAP-04",
      "GAP-05",
      "GAP-09",
      "GAP-11",
      "GAP-12",
      "GAP-19",
      "GAP-22",
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
});

describe("Unavailable results", () => {
  it("carries a note that names the missing API", () => {
    const result = unavailable("GAP-03");

    expect(isAvailable(result)).toBe(false);
    expect(result.reason).toBe("Follow-up API not yet available");
    expect(result.missingApi).toContain("/followups");
    expect(result.gap.id).toBe("GAP-03");
  });

  it("uses the override phrasing where the generic sentence would read wrong", () => {
    expect(gapNote(GAPS["GAP-02"])).toBe(
      "Steering has no direct endpoint — sent through chat as `/steer …`",
    );
  });

  it("accepts a caller-supplied reason", () => {
    expect(unavailable("GAP-10", "No events for this run yet").reason).toBe(
      "No events for this run yet",
    );
  });

  it("unwraps to the fallback rather than to fabricated data", () => {
    expect(unwrapOr(unavailable("GAP-03"), [])).toEqual([]);
  });
});
