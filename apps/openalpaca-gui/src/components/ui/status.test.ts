import { describe, expect, it } from "vitest";

import { statusLabelClasses } from "./StatusLabel";
import {
  isActive,
  isLive,
  STATUS_TEXT,
  statusAria,
  statusPulses,
  toUiStatus,
  type UiStatus,
} from "./status";

describe("run status mapping", () => {
  it("translates the daemon's wire spelling", () => {
    expect(toUiStatus("completed")).toBe("done");
    expect(toUiStatus("running")).toBe("running");
    expect(toUiStatus("cancelled")).toBe("cancelled");
  });

  it("keeps `failed` distinct instead of reporting it as done", () => {
    expect(toUiStatus("failed")).toBe("failed");
    expect(STATUS_TEXT.failed).toBe("FAILED");
  });

  it("pulses only while running (§1.7 — the one keyframe)", () => {
    const statuses: UiStatus[] = [
      "running",
      "queued",
      "paused",
      "done",
      "cancelled",
      "failed",
    ];
    expect(statuses.filter(statusPulses)).toEqual(["running"]);
  });

  it("derives railRuns (isLive) and activeCount (isActive) per §4.2", () => {
    expect(
      ["running", "queued", "paused"].every(
        (s) => isLive(s as UiStatus) && isActive(s as UiStatus),
      ),
    ).toBe(true);
    expect(isLive("done")).toBe(false);
    expect(isLive("cancelled")).toBe(false);
    expect(isLive("failed")).toBe(false);
    expect(isActive("done")).toBe(false);
  });

  /**
   * Daemon plan §5.6b — the boot sweep's new status crosses the wire, so the
   * vocabulary must name it. Terminal (out of the rail and the active count),
   * but styled from the warning tokens: a daemon restart is not an error the
   * run made.
   */
  it("carries interrupted as a terminal, non-error status", () => {
    expect(STATUS_TEXT.interrupted).toBe("INTERRUPTED");
    expect(statusAria("interrupted")).toBe("interrupted");
    expect(statusPulses("interrupted")).toBe(false);
    expect(isLive("interrupted")).toBe(false);
    expect(isActive("interrupted")).toBe(false);
    expect(toUiStatus("interrupted")).toBe("interrupted");
    const classes = statusLabelClasses("interrupted", "card");
    expect(classes).toContain("text-gold");
    expect(classes).not.toContain("text-red");
  });

  it("keeps the label's colour and its size in the same class list", () => {
    const classes = statusLabelClasses("running", "row");
    expect(classes).toContain("text-green");
    expect(classes).toContain("text-2xs-plus");
    expect(classes).toContain("font-mono");
  });
});
