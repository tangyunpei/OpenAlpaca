/**
 * The shell's typed refusals, pinned from this side (Q13): every row of
 * `shell-errors.json` — the wire form the shell's own test serializes each
 * variant of its `ShellError` against — reads as its kind, its sentence and
 * its log tail, and the kinds this side knows are exactly the rows' kinds.
 */

import { describe, expect, it } from "vitest";

import fixture from "./shell-errors.json";
import { SHELL_ERROR_KINDS, shellErrorOf } from "./shell-error";

describe("shellErrorOf", () => {
  it("reads every refusal the shell answers with", () => {
    for (const row of fixture.refusals) {
      const error = shellErrorOf(row);
      expect(error, JSON.stringify(row)).not.toBeNull();
      expect(error?.kind).toBe(row.kind);
      expect(error?.message).toBe(row.message);
      expect(error?.logTail).toBe(
        "log_tail" in row ? (row.log_tail ?? null) : null,
      );
    }
  });

  it("knows exactly the kinds the shell answers with", () => {
    const rows = new Set(fixture.refusals.map((row) => row.kind));
    expect([...SHELL_ERROR_KINDS].sort()).toEqual([...rows].sort());
  });

  it("reads nothing else as the shell's", () => {
    for (const cause of [
      "command reveal_path missing required key path",
      null,
      undefined,
      42,
      new Error("Discovery token has expired"),
      { message: "Discovery token has expired" },
      { kind: "expired" },
      { kind: "expired", message: 7 },
      { kind: "unheard_of", message: "x" },
      { kind: "spawn_failed", message: "x", log_tail: 7 },
    ]) {
      expect(shellErrorOf(cause), String(cause)).toBeNull();
    }
  });
});
