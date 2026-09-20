import { beforeEach, describe, expect, it } from "vitest";

import {
  isAbsolutePath,
  normalizeProjectPath,
  PROJECT_STORAGE_KEY,
  useProjectStore,
  workspaceOption,
} from "./project";

describe("isAbsolutePath", () => {
  it("accepts what the daemon can resolve on either platform", () => {
    expect(isAbsolutePath("/Users/dev/openalpaca")).toBe(true);
    expect(isAbsolutePath("C:\\code\\app")).toBe(true);
    expect(isAbsolutePath("C:/code/app")).toBe(true);
    expect(isAbsolutePath("\\\\server\\share")).toBe(true);
  });

  it("rejects a path the daemon would read against its own directory", () => {
    expect(isAbsolutePath("openalpaca")).toBe(false);
    expect(isAbsolutePath("./openalpaca")).toBe(false);
    expect(isAbsolutePath("../openalpaca")).toBe(false);
    // The daemon does not expand `~`; sending it would name a literal dir.
    expect(isAbsolutePath("~/code/app")).toBe(false);
  });
});

describe("normalizeProjectPath", () => {
  it("treats blank and unusable values as no project", () => {
    expect(normalizeProjectPath(null)).toBeNull();
    expect(normalizeProjectPath("")).toBeNull();
    expect(normalizeProjectPath("   ")).toBeNull();
    expect(normalizeProjectPath("relative/path")).toBeNull();
  });

  it("trims surrounding space and a trailing separator", () => {
    expect(normalizeProjectPath("  /Users/dev/app  ")).toBe("/Users/dev/app");
    expect(normalizeProjectPath("/Users/dev/app/")).toBe("/Users/dev/app");
    // A bare root survives — it is a path, not a separator to strip.
    expect(normalizeProjectPath("/")).toBe("/");
  });
});

describe("workspaceOption", () => {
  it("omits the field entirely when no project is chosen", () => {
    expect(workspaceOption(null)).toEqual({});
    expect("workspacePath" in workspaceOption(null)).toBe(false);
  });

  it("carries the path when one is", () => {
    expect(workspaceOption("/Users/dev/app")).toEqual({
      workspacePath: "/Users/dev/app",
    });
  });
});

describe("useProjectStore", () => {
  beforeEach(() => {
    localStorage.clear();
    useProjectStore.setState({ path: null });
  });

  it("persists the choice so it survives a restart", () => {
    useProjectStore.getState().setPath("/Users/dev/app");
    expect(useProjectStore.getState().path).toBe("/Users/dev/app");
    expect(localStorage.getItem(PROJECT_STORAGE_KEY)).toBe("/Users/dev/app");
  });

  it("clearing removes the stored value rather than storing an empty one", () => {
    useProjectStore.getState().setPath("/Users/dev/app");
    useProjectStore.getState().setPath(null);
    expect(useProjectStore.getState().path).toBeNull();
    expect(localStorage.getItem(PROJECT_STORAGE_KEY)).toBeNull();
  });

  it("refuses a path the daemon could not resolve", () => {
    useProjectStore.getState().setPath("code/app");
    expect(useProjectStore.getState().path).toBeNull();
  });
});
