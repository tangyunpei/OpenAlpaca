/**
 * R81: `x-workspace-path` is percent-encoded UTF-8 on the wire.
 *
 * The bug this closes is not cosmetic on either side: `fetch` throws on a header
 * value holding a non-Latin-1 character, so a window pointed at `~/项目/repo`
 * failed every request that carried the project, and the daemon's own read drops
 * anything above `\x7f` silently.
 */
import { describe, expect, it } from "vitest";

import { encodeWorkspacePath, workspaceHeader } from "./workspace-header";

describe("encodeWorkspacePath", () => {
  it("leaves a plain ASCII path exactly as it is", () => {
    expect(encodeWorkspacePath("/Users/dev/openalpaca")).toBe(
      "/Users/dev/openalpaca",
    );
    expect(encodeWorkspacePath("/Users/dev/my repo (2)")).toBe(
      "/Users/dev/my repo (2)",
    );
  });

  it("percent-encodes the UTF-8 bytes of a non-ASCII path", () => {
    expect(encodeWorkspacePath("/Users/jun/项目/openalpaca")).toBe(
      "/Users/jun/%E9%A1%B9%E7%9B%AE/openalpaca",
    );
    // Outside the BMP: one code point, four bytes, four escapes.
    expect(encodeWorkspacePath("/tmp/🦙")).toBe("/tmp/%F0%9F%A6%99");
  });

  it("escapes a literal percent so the daemon decodes the path it was given", () => {
    expect(encodeWorkspacePath("/tmp/50%20off")).toBe("/tmp/50%2520off");
  });

  it("is a header pair, or nothing when the window has no project", () => {
    expect(workspaceHeader("/Users/jun/项目")).toEqual({
      "x-workspace-path": "/Users/jun/%E9%A1%B9%E7%9B%AE",
    });
    expect(workspaceHeader(null)).toBeUndefined();
    expect(workspaceHeader(undefined)).toBeUndefined();
    // An empty path is the same signal, not an empty header.
    expect(workspaceHeader("")).toBeUndefined();
  });

  it("produces a value fetch can actually send", () => {
    // The whole point: `new Headers()` throws on the raw path.
    expect(
      () => new Headers(workspaceHeader("/Users/jun/项目/openalpaca")),
    ).not.toThrow();
    expect(
      () => new Headers({ "x-workspace-path": "/Users/jun/项目/openalpaca" }),
    ).toThrow();
  });
});
