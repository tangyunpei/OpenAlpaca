import { describe, expect, it } from "vitest";

import { SETTINGS_SECTIONS, sectionMeta, toSectionId } from "./sections";

describe("Settings sections (§5.4)", () => {
  it("ships the design's eight sections in order", () => {
    expect(SETTINGS_SECTIONS.map((section) => section.label)).toEqual([
      "Connection",
      "Models & keys",
      "Connectors",
      "Tools",
      "Extensions",
      "Agents",
      "Conversations",
      "Event log",
    ]);
  });

  it("keeps the design's copy verbatim", () => {
    expect(sectionMeta("connection").blurb).toBe(
      "Daemon status, endpoint and today's spend against the cap.",
    );
    expect(sectionMeta("models").blurb).toBe(
      "Providers the router can reach, in priority order. Pick a model to make it the chat default.",
    );
    expect(sectionMeta("connectors").blurb).toBe(
      "External services the agents may read and write.",
    );
    expect(sectionMeta("tools").blurb).toBe(
      "Capabilities the agents can invoke, and whether each asks first.",
    );
    expect(sectionMeta("agents").blurb).toBe(
      "Templates the orchestrator spawns from.",
    );
    expect(sectionMeta("conversations").blurb).toBe(
      "Stored lanes. Memory compaction runs weekly.",
    );
    expect(sectionMeta("events").blurb).toBe(
      "Everything the daemon emitted, newest first.",
    );
  });

  it("corrects the one factually wrong blurb — plugins are not WASM", () => {
    const blurb = sectionMeta("extensions").blurb;
    expect(blurb).not.toMatch(/wasm/i);
    expect(blurb).toMatch(/JSON-RPC/);
  });

  // ADR-030 §9.2: MCP servers and plugins are one list under one ENABLE axis.
  it("says the Extensions section covers MCP servers too", () => {
    expect(sectionMeta("extensions").blurb).toMatch(/MCP/);
  });

  it("only Models, Connectors and Extensions carry an add action", () => {
    expect(
      SETTINGS_SECTIONS.filter((section) => section.add !== undefined).map(
        (section) => section.add,
      ),
    ).toEqual(["Add provider", "Connect service", "Add extension"]);
  });

  it("falls back to Connection for an unknown persisted section id", () => {
    expect(toSectionId("extensions")).toBe("extensions");
    // The two ids C7 renamed are unknown now, and degrade rather than throw.
    expect(toSectionId("plugins")).toBe("connection");
    expect(toSectionId("nonsense")).toBe("connection");
  });
});
