import { describe, expect, it } from "vitest";

import { projectName } from "./format";

describe("projectName", () => {
  it.each([
    ["/Users/dev/app", "app"],
    ["/Users/dev/app/", "app"],
    ["C:\\Users\\dev\\app", "app"],
    ["/", null],
    ["", null],
  ] as const)("names %s by its last folder", (root, name) => {
    expect(projectName(root)).toBe(name);
  });

  it("names no project for a row with none", () => {
    expect(projectName(null)).toBeNull();
  });
});
