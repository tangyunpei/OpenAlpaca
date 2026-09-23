/**
 * A stored key's row. `KeyInfo.source` is a union the daemon does not
 * enforce: the CLI writes display labels, so an unknown string must come
 * through as itself.
 */

import { describe, expect, it } from "vitest";

import type { KeyInfo } from "@/lib/api/types";

import { formatKeyHealth, formatKeySource, keyRowSummary } from "./key-rows";

const key = (over: Partial<KeyInfo> = {}): KeyInfo => ({
  id: "anthropic_1700000000",
  masked_secret: "sk-ant-…7f3a",
  tier: null,
  priority: "primary",
  source: "api_console",
  notes: null,
  status: "healthy",
  monthly_usage_usd: null,
  ...over,
});

describe("a key's source", () => {
  it("labels every known value", () => {
    expect(formatKeySource("api_console")).toBe("API Console");
    expect(formatKeySource("claude_code")).toBe("Claude Code");
    expect(formatKeySource("claude_max_pro")).toBe("Claude Max/Pro");
    expect(formatKeySource("codex")).toBe("Codex");
    expect(formatKeySource("environment")).toBe("Environment");
    expect(formatKeySource("other")).toBe("Other");
  });

  it("passes the CLI's own label, or any unknown string, through verbatim", () => {
    expect(formatKeySource("API Console")).toBe("API Console");
    expect(formatKeySource("Claude Code")).toBe("Claude Code");
    expect(formatKeySource("a vault somewhere")).toBe("a vault somewhere");
    // Not a prototype key read off the label table.
    expect(formatKeySource("toString")).toBe("toString");
  });

  it("says when there is none rather than drawing a blank", () => {
    expect(formatKeySource(null)).toBe("no source recorded");
    expect(formatKeySource(undefined)).toBe("no source recorded");
    expect(formatKeySource("")).toBe("no source recorded");
  });
});

describe("a key's row", () => {
  it("reads masked secret, priority, source and health", () => {
    expect(keyRowSummary(key())).toBe(
      "sk-ant-…7f3a · primary · API Console · healthy",
    );
    expect(
      keyRowSummary(
        key({
          priority: "fallback",
          source: "Claude Code" as KeyInfo["source"],
          status: "rate_limited",
        }),
      ),
    ).toBe("sk-ant-…7f3a · fallback · Claude Code · rate limited");
  });

  it("does not invent a health for a key nobody has used", () => {
    expect(keyRowSummary(key({ status: "unknown" }))).toBe(
      "sk-ant-…7f3a · primary · API Console · health unknown",
    );
    expect(formatKeyHealth("unknown")).not.toContain("healthy");
    expect(formatKeyHealth("something new")).toBe("something new");
  });
});
