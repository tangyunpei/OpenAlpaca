import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { AgentsSection } from "./AgentsSection";

const state = vi.hoisted(() => ({
  templates: [] as unknown[],
  instances: [] as unknown[],
}));

vi.mock("@/hooks/useAgents", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/useAgents")>()),
  useAgentTemplates: () => ({
    data: state.templates,
    isPending: false,
    error: null,
  }),
  useAgentInstances: () => ({
    data: state.instances,
    isPending: false,
    error: null,
  }),
}));

function template(overrides: Record<string, unknown> = {}) {
  return {
    id: "review_agent",
    name: "Review",
    description: "Reviews things.",
    singleton: false,
    capabilities: [],
    denied_capabilities: [],
    temperature: 0.2,
    verbosity: "normal",
    fallback_models: [],
    require_confirmation_for: [],
    persona: "You review.",
    body: "",
    run_count: 0,
    ...overrides,
  };
}

beforeEach(() => {
  state.templates = [];
  state.instances = [];
});

describe("AgentsSection run counts (GAP-20, counts half)", () => {
  /**
   * The count is the daemon's, from `subagent_span` — the number the design's
   * `12 runs` chip promised and the registry called unavailable until now.
   */
  it("renders the run count the server sent", () => {
    state.templates = [
      template({ run_count: 12, last_run_at: "2026-09-04T09:15:00.000Z" }),
    ];

    render(<AgentsSection />);

    expect(screen.getByText(/12 runs/)).toBeInTheDocument();
  });

  /** A template nothing has spawned says so; it does not borrow a number. */
  it("says a template has never run rather than showing a bare 0", () => {
    state.templates = [template({ run_count: 0 })];

    render(<AgentsSection />);

    expect(screen.getByText(/No runs yet/)).toBeInTheDocument();
    expect(screen.queryByText(/0 runs/)).toBeNull();
  });

  /** Running instances are a different fact, and both fit on the meta line. */
  it("keeps the running-instance count beside the run count", () => {
    state.templates = [template({ run_count: 3 })];
    state.instances = [
      { id: "review_agent::a", template_id: "review_agent", name: "Review" },
    ];

    render(<AgentsSection />);

    expect(screen.getByText(/1 running/)).toBeInTheDocument();
    expect(screen.getByText(/3 runs/)).toBeInTheDocument();
  });

  /**
   * The other half of GAP-20 is untouched: enabling a template needs
   * enforcement in the spawn path (plan P-31), so the switch stays disabled
   * and still names why.
   */
  it("leaves the per-template toggle disabled and attributed", () => {
    state.templates = [template({ run_count: 3 })];

    render(<AgentsSection />);

    const toggle = screen.getByRole("switch");
    expect(toggle).toBeDisabled();
    expect(screen.getByText(/no enabled flag/i)).toBeInTheDocument();
  });
});
