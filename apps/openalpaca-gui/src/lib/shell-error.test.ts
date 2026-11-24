/**
 * The shell's typed refusals, pinned from this side (Q13): every row of
 * `shell-errors.json` — the wire form the shell's own test serializes each
 * variant of its `ShellError` against — reads as its kind, its sentence and
 * its log tail — and a mismatch its two protocols and its remedy (Q04) —
 * and the kinds this side knows are exactly the rows' kinds.
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
      expect(error?.mismatch).toEqual(
        row.kind === "mismatch"
          ? {
              daemonVersion:
                "daemon_version" in row ? row.daemon_version : null,
              daemonProtocol:
                "daemon_protocol" in row ? row.daemon_protocol : null,
              shellProtocol:
                "shell_protocol" in row ? row.shell_protocol : null,
              remedy: "remedy" in row ? row.remedy : null,
            }
          : null,
      );
    }
  });

  it("reads a mismatch whose daemon's version and protocol are unknown", () => {
    expect(
      shellErrorOf({
        kind: "mismatch",
        message: "m",
        daemon_version: null,
        daemon_protocol: null,
        shell_protocol: 1,
        remedy: "r",
      }),
    ).toEqual({
      kind: "mismatch",
      message: "m",
      logTail: null,
      mismatch: {
        daemonVersion: null,
        daemonProtocol: null,
        shellProtocol: 1,
        remedy: "r",
      },
    });
  });

  it("knows exactly the kinds the shell answers with", () => {
    const rows = new Set(fixture.refusals.map((row) => row.kind));
    expect(SHELL_ERROR_KINDS.toSorted()).toEqual([...rows].toSorted());
  });

  it("reads nothing else as the shell's", () => {
    for (const cause of [
      "command read_daemon_log_tail missing required key lines",
      null,
      undefined,
      42,
      new Error("Discovery token has expired"),
      { message: "Discovery token has expired" },
      { kind: "expired" },
      { kind: "expired", message: 7 },
      { kind: "unheard_of", message: "x" },
      { kind: "spawn_failed", message: "x", log_tail: 7 },
      { kind: "still_starting", message: "x", log_tail: 7 },
      { kind: "mismatch", message: "x", shell_protocol: 1 },
      { kind: "mismatch", message: "x", shell_protocol: "1", remedy: "r" },
      {
        kind: "mismatch",
        message: "x",
        daemon_protocol: "2",
        shell_protocol: 1,
        remedy: "r",
      },
    ]) {
      expect(shellErrorOf(cause), String(cause)).toBeNull();
    }
  });
});
