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

  it("covers the 20 gaps still open from API_MAP §3 (GAP-01/07/08a/08b/16 retired in Phase 0)", () => {
    expect(listGaps()).toHaveLength(20);
    expect(listGaps()[0]?.id).toBe("GAP-02");
    expect(listGaps().at(-1)?.id).toBe("GAP-23");
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
