import { describe, expect, it, vi } from "vitest";

import {
  clampPaneWidth,
  loadPaneWidths,
  PANE_DEFAULTS,
  PANE_WIDTHS_STORAGE_KEY,
  parsePaneWidths,
  savePaneWidths,
  type WidthStorage,
} from "./pane-widths";

function memoryStorage(initial: Record<string, string> = {}): WidthStorage & {
  values: Record<string, string>;
} {
  const values = { ...initial };
  return {
    values,
    getItem: (key) => values[key] ?? null,
    setItem: (key, value) => {
      values[key] = value;
    },
  };
}

describe("clampPaneWidth", () => {
  it("holds each pane inside the design's drag range", () => {
    expect(clampPaneWidth("workW", 120)).toBe(300);
    expect(clampPaneWidth("workW", 9000)).toBe(600);
    expect(clampPaneWidth("workListW", 100)).toBe(260);
    expect(clampPaneWidth("workListW", 999)).toBe(480);
    expect(clampPaneWidth("libListW", 100)).toBe(260);
    expect(clampPaneWidth("libListW", 999)).toBe(480);
  });

  it("passes an in-range value through, rounded to whole pixels", () => {
    expect(clampPaneWidth("workW", 412.6)).toBe(413);
  });

  it("falls back to the default for a non-finite width", () => {
    expect(clampPaneWidth("workW", Number.NaN)).toBe(PANE_DEFAULTS.workW);
    expect(clampPaneWidth("libListW", Number.POSITIVE_INFINITY)).toBe(480);
  });
});

describe("parsePaneWidths", () => {
  it("returns the defaults for missing or corrupt payloads", () => {
    expect(parsePaneWidths(null)).toEqual(PANE_DEFAULTS);
    expect(parsePaneWidths("{not json")).toEqual(PANE_DEFAULTS);
    expect(parsePaneWidths("[]")).toEqual(PANE_DEFAULTS);
  });

  it("falls back per key, so one bad width does not reset the layout", () => {
    const parsed = parsePaneWidths(
      JSON.stringify({ workW: 500, workListW: "wide" }),
    );
    expect(parsed).toEqual({
      workW: 500,
      workListW: PANE_DEFAULTS.workListW,
      libListW: PANE_DEFAULTS.libListW,
    });
  });

  it("clamps stored values that are out of range", () => {
    expect(parsePaneWidths(JSON.stringify({ workW: 5000 })).workW).toBe(600);
  });
});

describe("persistence", () => {
  it("round-trips through the legacy `oa-pane-widths` key", () => {
    const storage = memoryStorage();
    savePaneWidths({ workW: 500, workListW: 300, libListW: 400 }, storage);

    expect(storage.values[PANE_WIDTHS_STORAGE_KEY]).toBe(
      '{"workW":500,"workListW":300,"libListW":400}',
    );
    expect(loadPaneWidths(storage)).toEqual({
      workW: 500,
      workListW: 300,
      libListW: 400,
    });
  });

  it("survives a storage that throws (private mode, blocked site data)", () => {
    const throwing: WidthStorage = {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("blocked");
      },
    };

    expect(loadPaneWidths(throwing)).toEqual(PANE_DEFAULTS);
    expect(() => savePaneWidths(PANE_DEFAULTS, throwing)).not.toThrow();
  });

  it("is a no-op when there is no storage at all", () => {
    const setItem = vi.fn();
    expect(loadPaneWidths(null)).toEqual(PANE_DEFAULTS);
    expect(setItem).not.toHaveBeenCalled();
  });
});
