import { describe, expect, it } from "vitest";

import { statusLabelClasses } from "./StatusLabel";
import {
  isActive,
  isLive,
  STATUS_TEXT,
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

  it("keeps the label's colour and its size in the same class list", () => {
    const classes = statusLabelClasses("running", "row");
    expect(classes).toContain("text-green");
    expect(classes).toContain("text-2xs-plus");
    expect(classes).toContain("font-mono");
  });
});
