import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";

import { parseTimestamp, timestampMs } from "./time";

/**
 * G3 — the bug was only visible off UTC, so these run in Los Angeles.
 *
 * Node applies a `TZ` written at runtime; `vi.unstubAllEnvs` puts the runner's
 * own zone back so no other suite inherits this one.
 */
beforeAll(() => {
  vi.stubEnv("TZ", "America/Los_Angeles");
});
afterAll(() => {
  vi.unstubAllEnvs();
});

describe("parseTimestamp", () => {
  it("reads a zone-less daemon stamp as UTC, not as local time", () => {
    // What `GET /v1/chat/history` returns for a persisted row.
    const parsed = parseTimestamp("2026-09-18 17:05:12");
    expect(parsed?.toISOString()).toBe("2026-09-18T17:05:12.000Z");
    // …which is 10:05 for the person reading it, not 17:05.
    expect(parsed?.getHours()).toBe(10);

    // The bug this replaces: `new Date` takes it as local.
    expect(new Date("2026-09-18 17:05:12").getHours()).toBe(17);
  });

  it("leaves a stamp that carries its own zone alone", () => {
    expect(parseTimestamp("2026-09-18T17:05:12Z")?.toISOString()).toBe(
      "2026-09-18T17:05:12.000Z",
    );
    expect(parseTimestamp("2026-09-18T17:05:12.123Z")?.toISOString()).toBe(
      "2026-09-18T17:05:12.123Z",
    );
    expect(parseTimestamp("2026-09-18T10:05:12-07:00")?.toISOString()).toBe(
      "2026-09-18T17:05:12.000Z",
    );
    expect(parseTimestamp("2026-09-18T10:05:12-0700")?.toISOString()).toBe(
      "2026-09-18T17:05:12.000Z",
    );
  });

  it("answers null rather than guessing", () => {
    expect(parseTimestamp(null)).toBeNull();
    expect(parseTimestamp(undefined)).toBeNull();
    expect(parseTimestamp("")).toBeNull();
    expect(parseTimestamp("   ")).toBeNull();
    expect(parseTimestamp("not a date")).toBeNull();
  });

  it("gives the two spellings of the same instant the same epoch", () => {
    expect(timestampMs("2026-09-18 17:05:12")).toBe(
      timestampMs("2026-09-18T17:05:12Z"),
    );
    expect(timestampMs("nonsense")).toBeNull();
  });
});
