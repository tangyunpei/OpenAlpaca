import { beforeEach, describe, expect, it, vi } from "vitest";

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

  it("rejects a relative path, which the daemon never takes as a project", () => {
    expect(isAbsolutePath("openalpaca")).toBe(false);
    expect(isAbsolutePath("./openalpaca")).toBe(false);
    expect(isAbsolutePath("../openalpaca")).toBe(false);
    // The daemon does not expand `~`: to it `~/code/app` is a relative path.
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
    useProjectStore.setState({ path: null, id: null });
  });

  it("keeps the root and the id the daemon answered, so a re-point made elsewhere can be followed", () => {
    useProjectStore.getState().setPath("/Users/dev/app", "6f1c2a3b-4d5e");
    expect(useProjectStore.getState().path).toBe("/Users/dev/app");
    expect(useProjectStore.getState().id).toBe("6f1c2a3b-4d5e");
    expect(
      JSON.parse(localStorage.getItem(PROJECT_STORAGE_KEY) ?? "null"),
    ).toEqual({ root: "/Users/dev/app", id: "6f1c2a3b-4d5e" });
  });

  it("holds no id for a path set without one, and none once cleared", () => {
    useProjectStore.getState().setPath("/Users/dev/app", "6f1c2a3b-4d5e");
    useProjectStore.getState().setPath("/Users/dev/other");
    expect(useProjectStore.getState().id).toBeNull();
    useProjectStore.getState().setPath("/Users/dev/app", "6f1c2a3b-4d5e");
    useProjectStore.getState().setPath(null);
    expect(useProjectStore.getState().id).toBeNull();
  });

  it("clearing removes the stored value rather than storing an empty one", () => {
    useProjectStore.getState().setPath("/Users/dev/app");
    useProjectStore.getState().setPath(null);
    expect(useProjectStore.getState().path).toBeNull();
    expect(localStorage.getItem(PROJECT_STORAGE_KEY)).toBeNull();
  });

  it("reads back the root and the id it stored, after a restart", async () => {
    localStorage.setItem(
      PROJECT_STORAGE_KEY,
      JSON.stringify({ root: "/Users/dev/app", id: "6f1c2a3b-4d5e" }),
    );
    vi.resetModules();
    const restarted = (await import("./project")).useProjectStore.getState();
    expect(restarted.path).toBe("/Users/dev/app");
    expect(restarted.id).toBe("6f1c2a3b-4d5e");
  });

  it("reads a root an earlier build stored alone as that root, with no id", async () => {
    localStorage.setItem(PROJECT_STORAGE_KEY, "/Users/dev/app");
    vi.resetModules();
    const restarted = (await import("./project")).useProjectStore.getState();
    expect(restarted.path).toBe("/Users/dev/app");
    expect(restarted.id).toBeNull();
  });

  it("reads a stored value it cannot use as no project", async () => {
    for (const raw of [
      JSON.stringify({ id: "6f1c2a3b-4d5e" }),
      JSON.stringify({ root: "code/app", id: "6f1c2a3b-4d5e" }),
      "code/app",
    ]) {
      localStorage.setItem(PROJECT_STORAGE_KEY, raw);
      vi.resetModules();
      const restarted = (await import("./project")).useProjectStore.getState();
      expect(restarted.path).toBeNull();
      expect(restarted.id).toBeNull();
    }
  });

  it("refuses a path the daemon could not resolve", () => {
    useProjectStore.getState().setPath("code/app");
    expect(useProjectStore.getState().path).toBeNull();
  });
});
