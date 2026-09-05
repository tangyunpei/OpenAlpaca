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
  // C7 deleted — the family that replaced them carries `ts`/`instance_id`.
  it("covers the 19 gaps still open from API_MAP §3", () => {
    expect(listGaps()).toHaveLength(19);
    expect(listGaps()[0]?.id).toBe("GAP-02");
    expect(listGaps().at(-1)?.id).toBe("GAP-24");
    expect(listGaps().map((gap) => gap.id)).not.toContain("GAP-19");
    expect(listGaps().map((gap) => gap.id)).not.toContain("GAP-22");
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
    const result = unavailable("GAP-04");

    expect(isAvailable(result)).toBe(false);
    expect(result.reason).toBe("Artifact API not yet available");
    expect(result.missingApi).toContain("/v1/artifacts");
    expect(result.gap.id).toBe("GAP-04");
  });

  it("uses the override phrasing where the generic sentence would read wrong", () => {
    expect(gapNote(GAPS["GAP-12"])).toBe(
      "Pins are stored on this machine only",
    );
  });

  it("accepts a caller-supplied reason", () => {
    expect(unavailable("GAP-09", "No timeline for this run yet").reason).toBe(
      "No timeline for this run yet",
    );
  });

  it("unwraps to the fallback rather than to fabricated data", () => {
    expect(unwrapOr(unavailable("GAP-04"), [])).toEqual([]);
  });
});
