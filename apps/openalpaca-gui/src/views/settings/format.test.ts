import { describe, expect, it } from "vitest";

import { formatUptime } from "./format";

describe("formatUptime", () => {
  // The design's own string (§5.4): days and a zero-padded hour.
  it("reads as the design's `4d 02h` once a daemon has run for days", () => {
    expect(formatUptime(4 * 86_400 + 2 * 3_600 + 61)).toBe("4d 02h");
    expect(formatUptime(4 * 86_400)).toBe("4d 00h");
  });

  it("drops to the largest unit that is actually non-zero", () => {
    expect(formatUptime(3 * 3_600 + 7 * 60)).toBe("3h 07m");
    expect(formatUptime(12 * 60 + 30)).toBe("12m");
    expect(formatUptime(42)).toBe("42s");
    expect(formatUptime(0)).toBe("0s");
  });

  // A daemon that has not answered yet has no uptime; the em dash is the
  // design's own placeholder, and inventing `0s` would read as a restart.
  it("has a placeholder for the answer it does not have", () => {
    expect(formatUptime(undefined)).toBe("—");
    expect(formatUptime(-5)).toBe("—");
  });
});
