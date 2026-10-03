import { beforeEach, describe, expect, it } from "vitest";

import {
  clearConfirmationsOver,
  isConfirmationOver,
  noteConfirmationOver,
} from "./confirmations-over";

const id = (n: number) => `req-${n}`;

beforeEach(() => {
  clearConfirmationsOver();
});

describe("confirmations-over", () => {
  it("holds what the daemon said is over, and nothing else", () => {
    noteConfirmationOver("req-answered");
    expect(isConfirmationOver("req-answered")).toBe(true);
    expect(isConfirmationOver("req-pending")).toBe(false);
  });

  it("keeps the newest 1,000 ids, dropping the oldest first", () => {
    for (let n = 0; n <= 1_000; n++) noteConfirmationOver(id(n));

    expect(isConfirmationOver(id(0))).toBe(false);
    expect(isConfirmationOver(id(1))).toBe(true);
    expect(isConfirmationOver(id(1_000))).toBe(true);
  });

  it("noting an id it already holds evicts nothing", () => {
    for (let n = 0; n < 1_000; n++) noteConfirmationOver(id(n));

    noteConfirmationOver(id(500));

    expect(isConfirmationOver(id(0))).toBe(true);
    expect(isConfirmationOver(id(500))).toBe(true);
    expect(isConfirmationOver(id(999))).toBe(true);
  });
});
