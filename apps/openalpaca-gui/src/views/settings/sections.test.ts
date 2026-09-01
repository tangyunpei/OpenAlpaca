import { describe, expect, it } from "vitest";

import { SETTINGS_SECTIONS, sectionMeta, toSectionId } from "./sections";

describe("Settings sections (§5.4)", () => {
  it("ships the design's eight sections in order", () => {
    expect(SETTINGS_SECTIONS.map((section) => section.label)).toEqual([
      "Connection",
      "Models & keys",
      "Connectors",
      "Skills",
      "Plugins",
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
    expect(sectionMeta("skills").blurb).toBe(
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
    const blurb = sectionMeta("plugins").blurb;
    expect(blurb).not.toMatch(/wasm/i);
    expect(blurb).toMatch(/JSON-RPC/);
  });

  it("only Models, Connectors and Plugins carry an add action", () => {
    expect(
      SETTINGS_SECTIONS.filter((section) => section.add !== undefined).map(
        (section) => section.add,
      ),
    ).toEqual(["Add provider", "Connect service", "Install plugin"]);
  });

  it("falls back to Connection for an unknown persisted section id", () => {
    expect(toSectionId("plugins")).toBe("plugins");
    expect(toSectionId("nonsense")).toBe("connection");
  });
});
