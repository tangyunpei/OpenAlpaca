/**
 * The Memory view's Diff tab compares two versions of one note on this side:
 * the daemon keeps the last five revisions a note was written at
 * (`.history`, design §2) and serves each one's text, but no patch. The patch
 * is built here and fed to the Library's own `DiffView`, so it has to be one
 * that view's parser reads back line for line.
 */

import { describe, expect, it } from "vitest";

import { parseUnifiedDiff } from "@/components/work";

import { unifiedPatch } from "./line-diff";

describe("unifiedPatch", () => {
  it("is empty when nothing changed", () => {
    expect(unifiedPatch("a\nb\n", "a\nb\n")).toBe("");
  });

  it("marks one changed line against its neighbours", () => {
    const patch = unifiedPatch(
      "# Editor\n\nUses vim.\n",
      "# Editor\n\nUses helix.\n",
    );
    const parsed = parseUnifiedDiff(patch);
    expect(parsed.added).toBe(1);
    expect(parsed.removed).toBe(1);
    expect(
      parsed.lines
        .filter((line) => line.kind === "added" || line.kind === "removed")
        .map((line) => `${line.kind}:${line.text}`),
    ).toEqual(["removed:Uses vim.", "added:Uses helix."]);
    // The hunk header numbers the lines the way a real patch does.
    expect(patch.startsWith("@@ -1,3 +1,3 @@\n")).toBe(true);
  });

  it("keeps three lines of context and splits distant changes into hunks", () => {
    const before = Array.from({ length: 20 }, (_, i) => `line ${i + 1}`);
    const after = [...before];
    after[1] = "line two";
    after[17] = "line eighteen";
    const patch = unifiedPatch(before.join("\n"), after.join("\n"));
    const hunks = patch.split("\n").filter((line) => line.startsWith("@@"));
    expect(hunks).toEqual(["@@ -1,5 +1,5 @@", "@@ -15,6 +15,6 @@"]);
    const parsed = parseUnifiedDiff(patch);
    expect(parsed.added).toBe(2);
    expect(parsed.removed).toBe(2);
    // Old and new numbers survive the round trip.
    const removed = parsed.lines.find((line) => line.kind === "removed");
    expect(removed?.oldNumber).toBe(2);
  });

  it("reads a pure addition and a pure removal", () => {
    expect(parseUnifiedDiff(unifiedPatch("", "one\ntwo\n")).added).toBe(2);
    expect(parseUnifiedDiff(unifiedPatch("one\ntwo\n", "")).removed).toBe(2);
  });

  it("keeps CJK and emoji lines whole", () => {
    const parsed = parseUnifiedDiff(unifiedPatch("编译 ✅\n", "测试 🚀\n"));
    expect(
      parsed.lines
        .filter((line) => line.kind !== "hunk")
        .map((line) => line.text),
    ).toEqual(["编译 ✅", "测试 🚀"]);
  });
});
