import { describe, expect, it } from "vitest";

import { formatDiffStat, parseUnifiedDiff, sourceAsDiffLines } from "./diff";

const PATCH = `diff --git a/src/lib.rs b/src/lib.rs
index 1a2b3c4..5d6e7f8 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -12,6 +12,7 @@ impl Router {
     fn dispatch(&self) {
-        self.legacy();
+        self.lead_agent();
+        self.record();
     }
 }`;

describe("parseUnifiedDiff", () => {
  const parsed = parseUnifiedDiff(PATCH);

  it("counts added and removed lines from the patch itself", () => {
    expect(parsed.added).toBe(2);
    expect(parsed.removed).toBe(1);
  });

  it("classifies every line", () => {
    expect(parsed.lines.map((line) => line.kind)).toEqual([
      "meta",
      "meta",
      "meta",
      "meta",
      "hunk",
      "context",
      "removed",
      "added",
      "added",
      "context",
      "context",
    ]);
  });

  it("strips the marker but keeps the code", () => {
    const added = parsed.lines.filter((line) => line.kind === "added");
    expect(added.map((line) => line.text)).toEqual([
      "        self.lead_agent();",
      "        self.record();",
    ]);
  });

  it("numbers lines from the hunk header, skipping the other side", () => {
    const removed = parsed.lines.find((line) => line.kind === "removed");
    expect(removed?.oldNumber).toBe(13);
    expect(removed?.newNumber).toBeNull();

    const firstAdded = parsed.lines.find((line) => line.kind === "added");
    expect(firstAdded?.newNumber).toBe(13);
    expect(firstAdded?.oldNumber).toBeNull();
  });

  it("handles an empty patch without inventing a line", () => {
    expect(parseUnifiedDiff("")).toEqual({ lines: [], added: 0, removed: 0 });
    expect(parseUnifiedDiff("\n").lines).toEqual([]);
  });

  it("restarts numbering at every hunk", () => {
    const twoHunks = parseUnifiedDiff(
      ["@@ -1,1 +1,1 @@", " a", "@@ -40,1 +50,1 @@", " b"].join("\n"),
    );
    const contexts = twoHunks.lines.filter((line) => line.kind === "context");
    expect(contexts[0]?.oldNumber).toBe(1);
    expect(contexts[1]?.oldNumber).toBe(40);
    expect(contexts[1]?.newNumber).toBe(50);
  });
});

describe("formatDiffStat", () => {
  it("uses a real minus sign, as the design does", () => {
    expect(formatDiffStat(41, 6)).toEqual({ added: "+41", removed: "−6" });
  });
});

describe("sourceAsDiffLines", () => {
  it("numbers a plain file and marks every line context", () => {
    const lines = sourceAsDiffLines("one\ntwo\n");
    expect(lines).toHaveLength(2);
    expect(lines[1]).toEqual({
      kind: "context",
      text: "two",
      oldNumber: 2,
      newNumber: 2,
    });
  });

  it("returns nothing for empty bytes", () => {
    expect(sourceAsDiffLines("")).toEqual([]);
  });
});
