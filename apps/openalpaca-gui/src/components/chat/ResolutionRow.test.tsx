import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import {
  ResolutionRow,
  resolutionNote,
  settlingRun,
  type ToolRun,
} from "./ResolutionRow";

const ran = (patch: Partial<ToolRun> = {}): ToolRun => ({
  toolName: "artifact_write",
  success: true,
  duration: "1.4s",
  atMs: 2_000,
  ...patch,
});

describe("ResolutionRow (§3.15)", () => {
  it("labels the outcome and prints the note", () => {
    render(
      <ResolutionRow
        resolution="approved"
        note="artifact_write approved · returned in 1.4s, the agent resumed."
        time="14:23"
      />,
    );
    expect(screen.getByText("Approved")).toBeInTheDocument();
    expect(screen.getByText(/returned in 1\.4s/)).toBeInTheDocument();
    expect(screen.getByText("14:23")).toBeInTheDocument();
  });
});

/**
 * G6 — the card kept saying "approved · waiting for the tool to run…" long
 * after the tool had run and the turn had finished.
 *
 * Nothing was lost on the wire: the daemon releases the tool the instant the
 * answer is posted, so `tool_executed` beat the POST's own `onSuccess`, and
 * the upgrade pass ran over a list that did not yet contain the card.
 */
describe("settling an answered confirmation (G6)", () => {
  it("uses a run that already reported when the card is made", () => {
    expect(
      resolutionNote(
        "approved",
        "artifact_write",
        settlingRun([ran()], "artifact_write", 1_000),
      ),
    ).toBe("artifact_write approved · returned in 1.4s, the agent resumed.");
  });

  it("still says `waiting` when nothing has reported yet", () => {
    expect(resolutionNote("approved", "artifact_write", null)).toBe(
      "artifact_write approved · waiting for the tool to run…",
    );
  });

  it("will not borrow an earlier call of the same tool", () => {
    // The run happened *before* this answer went out, so it is somebody
    // else's — the card must keep waiting for its own.
    expect(
      settlingRun([ran({ atMs: 900 })], "artifact_write", 1_000),
    ).toBeNull();
  });

  it("matches on the tool, newest first", () => {
    const runs = [
      ran({ atMs: 1_100, duration: "0.2s" }),
      ran({ toolName: "file_write", atMs: 1_200 }),
      ran({ atMs: 1_300, success: false, duration: "9.0s" }),
    ];
    expect(settlingRun(runs, "artifact_write", 1_000)?.duration).toBe("9.0s");
    expect(settlingRun(runs, "shell_execute", 1_000)).toBeNull();
  });

  /** A denial is settled the moment it is made: nothing is going to run. */
  it("leaves a denial alone", () => {
    expect(resolutionNote("denied", "artifact_write", ran())).toBe(
      "artifact_write denied · the agent was told to skip it.",
    );
  });
});
