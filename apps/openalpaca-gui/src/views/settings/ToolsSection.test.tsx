import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useUiStore } from "@/stores/ui";

import { ToolsSection } from "./ToolsSection";

const state = vi.hoisted(() => ({
  tools: [] as unknown[],
  skills: [] as unknown[],
}));

vi.mock("@/hooks/useSkills", () => ({
  useTools: () => ({ data: state.tools, isPending: false, error: null }),
  useSkillHealth: () => ({ data: state.skills, isPending: false, error: null }),
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

beforeEach(() => {
  state.tools = [builtin, fromMcp];
  state.skills = [];
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

  it("keeps skill health in its own subsection and names the listing still missing", () => {
    state.skills = [
      {
        skill_id: "connector_audit",
        total_invocations: 9,
        clean_success_rate: 0.9,
        repair_rate: 0.1,
        avg_duration_ms: 1400,
      },
    ];
    render(<ToolsSection />);

    expect(screen.getByText("Skill health")).toBeInTheDocument();
    expect(screen.getByText("connector_audit")).toBeInTheDocument();
    expect(
      screen.getByText(/Skill catalog not yet available/),
    ).toBeInTheDocument();
  });
});
