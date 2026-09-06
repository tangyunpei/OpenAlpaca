import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useUiStore } from "@/stores/ui";

import { ToolsSection } from "./ToolsSection";

const state = vi.hoisted(() => ({
  tools: [] as unknown[],
  skills: [] as unknown[],
  catalog: [] as unknown[],
}));

vi.mock("@/hooks/useSkills", () => ({
  useTools: () => ({ data: state.tools, isPending: false, error: null }),
  useSkillHealth: () => ({ data: state.skills, isPending: false, error: null }),
  useSkillCatalog: () => ({
    data: state.catalog,
    isPending: false,
    error: null,
  }),
}));

const builtin = {
  name: "shell_execute",
  description: "Runs a shell command.",
  source: "builtin",
  origin: null,
  provides_capabilities: ["shell_execute"],
  requires_confirmation: true,
  invocations_today: 4,
  version: "1.0.0",
  author: "builtin",
};

const fromMcp = {
  name: "github__create_issue",
  description: "Opens an issue.",
  source: "mcp",
  origin: { kind: "mcp", id: "github", enabled: true, state: "enabled" },
  provides_capabilities: ["github__create_issue"],
  requires_confirmation: false,
  invocations_today: 12,
  version: "1.4.0",
  author: "mcp:github",
};

const health = {
  skill_id: "connector_audit",
  total_invocations: 9,
  clean_success_rate: 0.9,
  repair_rate: 0.1,
  avg_duration_ms: 1400,
};

const catalogued = {
  id: "connector_audit",
  name: "Connector Audit",
  description: "Check every connector's credentials",
  source: "file",
  origin: null,
  requires_capabilities: [],
  triggers: { slash: "audit", keywords: [] },
  schedule: null,
  invocations_today: 2,
  version: "0.2.0",
  author: "file:project",
};

beforeEach(() => {
  state.tools = [builtin, fromMcp];
  state.skills = [];
  state.catalog = [];
  useUiStore.setState({ settingsSectionId: "tools", toast: null });
});

describe("ToolsSection (ADR-030 §9.3)", () => {
  /**
   * S1: ENABLE is one toggle per MCP server and per plugin; ALLOW is per-agent
   * capability. A per-tool switch — even a disabled one — would assert a
   * mechanism that does not exist.
   */
  it("draws no control on any row, builtin or extension", () => {
    render(<ToolsSection />);

    expect(screen.queryByRole("switch")).toBeNull();
    expect(screen.getByText("shell_execute")).toBeInTheDocument();
    expect(screen.getByText("github__create_issue")).toBeInTheDocument();
    expect(
      screen.getByText(/Tools have no individual on\/off switch/),
    ).toBeInTheDocument();
  });

  it("shows provenance for an extension tool and none for a builtin", async () => {
    render(<ToolsSection />);

    const chip = screen.getByRole("button", {
      name: "via MCP github — enabled",
    });
    expect(screen.queryByText(/via .* builtin/)).toBeNull();

    // The chip leads to the row that *does* carry the switch.
    await userEvent.setup().click(chip);
    expect(useUiStore.getState().settingsSectionId).toBe("extensions");
  });

  it("carries the asks badge and today's count from the catalog", () => {
    render(<ToolsSection />);

    expect(screen.getByText("asks")).toBeInTheDocument();
    expect(screen.getByText("4 today")).toBeInTheDocument();
    expect(screen.getByText("12 today")).toBeInTheDocument();
  });

  it("names a health row from the catalog and keeps the catalog id beside it", () => {
    state.skills = [health];
    state.catalog = [catalogued];
    render(<ToolsSection />);

    expect(screen.getByText("Skill health")).toBeInTheDocument();
    expect(screen.getByText("Connector Audit")).toBeInTheDocument();
    // The catalog id is what `/slash` resolves, so it stays on the row rather
    // than being replaced by the name.
    expect(screen.getByText(/connector_audit ·/)).toBeInTheDocument();
    // GAP-18 is closed — the note that named the missing listing is gone.
    expect(screen.queryByText(/Skill catalog not yet available/)).toBeNull();
  });

  /**
   * `skill_execution_log.skill_id` is **not** the catalog id: every invocation
   * path resolves the entry and logs `frontmatter.name` (`/slash` and the
   * router through `Intent::SkillInvocation`, and the model's `invoke_skill`).
   * The join resolves a health row the way `SkillCatalog::get` does — id first,
   * then name, case-insensitively — or it would miss on every file skill
   * anyone actually ran.
   */
  it("resolves a health row logged under the frontmatter name", () => {
    state.skills = [{ ...health, skill_id: "Connector Audit" }];
    state.catalog = [catalogued];
    render(<ToolsSection />);

    expect(screen.getByText("Connector Audit")).toBeInTheDocument();
    // Named from the catalog, and the canonical id is what the second line
    // shows — not the spelling the log happened to hold.
    expect(screen.getByText(/^connector_audit ·/)).toBeInTheDocument();
  });

  /**
   * `skill_execution_log` outlives the catalog: a skill directory can be
   * deleted, and a plugin's skill is withdrawn from the catalog the moment the
   * plugin is disabled. The row then shows the id it has — never a placeholder
   * name, and never nothing.
   */
  it("falls back to the id for a health row the catalog no longer holds", () => {
    state.skills = [health];
    state.catalog = [];
    render(<ToolsSection />);

    expect(screen.getByText("connector_audit")).toBeInTheDocument();
    expect(screen.queryByText("Connector Audit")).toBeNull();
  });
});
