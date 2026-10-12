import { describe, expect, it } from "vitest";

import { baseName, projectName } from "./format";

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

describe("baseName", () => {
  it.each([
    ["src/main.rs", "main.rs"],
    ["docs\\notes.md", "notes.md"],
    ["01-connector-audit-findings.md", "01-connector-audit-findings.md"],
  ] as const)("saves %s as %s", (name, base) => {
    expect(baseName(name)).toBe(base);
  });
});
