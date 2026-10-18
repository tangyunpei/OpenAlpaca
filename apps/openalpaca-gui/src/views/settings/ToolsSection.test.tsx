import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { SkillCatalogEntry, SkillDiagnostic } from "@/lib/api/types";
import { useUiStore } from "@/stores/ui";

import {
  ToolsSection,
  skillProblemsLine,
  skillsByLogKey,
} from "./ToolsSection";

const state = vi.hoisted(() => ({
  tools: [] as unknown[],
  skills: [] as unknown[],
  catalog: [] as unknown[],
  diagnostics: undefined as unknown[] | undefined,
}));

vi.mock("@/hooks/useSkills", () => ({
  useTools: () => ({ data: state.tools, isPending: false, error: null }),
  useSkillHealth: () => ({ data: state.skills, isPending: false, error: null }),
  useSkillCatalog: () => ({
    data: state.catalog,
    isPending: false,
    error: null,
  }),
  useSkillDiagnostics: () => ({
    data: state.diagnostics,
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

/**
 * What the daemon serves for a skills directory holding a malformed
 * SKILL.md, a FIFO named SKILL.md and two skills claiming one slash command.
 */
const diagnostics: SkillDiagnostic[] = [
  {
    code: "invalid_yaml",
    severity: "error",
    path: "/cfg/skills/broken/SKILL.md",
    skill_id: "broken",
    message: "Invalid YAML: did not find expected node content",
  },
  {
    code: "read_failed",
    severity: "error",
    path: "/cfg/skills/piped/SKILL.md",
    skill_id: "piped",
    message: "Could not read SKILL.md: not a regular file",
  },
  {
    code: "slash_conflict",
    severity: "warning",
    path: "/cfg/skills/second-review/SKILL.md",
    skill_id: "second-review",
    message:
      "Slash command '/review' is also claimed by skill 'code-review'; this skill now answers it",
  },
];

beforeEach(() => {
  state.tools = [builtin, fromMcp];
  state.skills = [];
  state.catalog = [];
  state.diagnostics = [];
  useUiStore.setState({ settingsSectionId: "tools", toast: null });
});

describe("the skill scan's problems (N-SKILL-2)", () => {
  it("counts what the last scan found and points at the CLI that names it", () => {
    state.diagnostics = diagnostics;
    render(<ToolsSection />);

    const status = screen.getByRole("status");
    expect(status).toHaveTextContent(
      "3 problems in the skill files — 2 not loaded, 1 warning. `openalpaca skills list` names each one.",
    );
    expect(status).toHaveTextContent(/^3/);
  });

  it("draws no count when the scan found nothing, or the read has not answered", () => {
    render(<ToolsSection />);
    expect(screen.queryByRole("status")).toBeNull();

    state.diagnostics = undefined;
    render(<ToolsSection />);
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("words one problem in the singular", () => {
    expect(skillProblemsLine([])).toBeNull();
    expect(skillProblemsLine(diagnostics.slice(2))).toBe(
      "1 problem in the skill files — 0 not loaded, 1 warning. `openalpaca skills list` names each one.",
    );
    expect(skillProblemsLine(diagnostics.slice(0, 2))).toBe(
      "2 problems in the skill files — 2 not loaded, 0 warnings. `openalpaca skills list` names each one.",
    );
  });
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

  /**
   * The daemon's `resolve_skill_key` breaks a shared frontmatter name on the
   * **lowest** id, so two reads of an unchanged catalog agree. The client
   * mirrors that rule, or the same log key would name one skill here and count
   * towards another in `GET /v1/skills`.
   */
  it("breaks a shared frontmatter name on the lowest id, in either order", () => {
    const a = {
      ...catalogued,
      id: "a_audit",
      name: "Audit",
    } as SkillCatalogEntry;
    const z = {
      ...catalogued,
      id: "z_audit",
      name: "Audit",
    } as SkillCatalogEntry;

    expect(skillsByLogKey([a, z]).get("audit")?.id).toBe("a_audit");
    expect(skillsByLogKey([z, a]).get("audit")?.id).toBe("a_audit");
    // An id still wins outright over another entry's name.
    expect(skillsByLogKey([a, z]).get("z_audit")?.id).toBe("z_audit");
  });
});
