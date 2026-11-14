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
    window: "7d",
    ...overrides,
  };
}

beforeEach(() => {
  state.templates = [];
  state.instances = [];
});

describe("AgentsSection run counts (GAP-20, counts half — T48)", () => {
  /**
   * The count and its window are both the daemon's — `subagent_span`,
   * completed runs only, and the window it was asked about (T48) — giving the
   * design's `12 runs 7d` chip exactly.
   */
  it("renders the run count and window the server sent", () => {
    state.templates = [
      template({
        run_count: 12,
        last_run_at: "2026-09-04T09:15:00.000Z",
        window: "7d",
      }),
    ];

    render(<AgentsSection />);

    expect(screen.getByText("12 runs · 7d")).toBeInTheDocument();
  });

  /** A different window on the row relabels the card, not just the count. */
  it("relabels the card when the server reports a different window", () => {
    state.templates = [template({ run_count: 4, window: "30d" })];

    render(<AgentsSection />);

    expect(screen.getByText("4 runs · 30d")).toBeInTheDocument();
  });

  /**
   * A daemon that predates T48 sends `run_count` but no `window` field at
   * all. The card must not fabricate a window it was not told about — it
   * drops the label rather than guessing `7d` for a count that might be
   * lifetime-and-in-flight (the old interim shape).
   */
  it("drops the window label when the server did not send one", () => {
    state.templates = [template({ run_count: 5 })];
    delete (state.templates[0] as Record<string, unknown>).window;

    render(<AgentsSection />);

    expect(screen.getByText("5 runs")).toBeInTheDocument();
    expect(screen.queryByText(/undefined/)).toBeNull();
  });

  /**
   * A windowed zero says only what the data says — none in this window — and
   * never claims the template has never run.
   */
  it("says no runs in the window rather than never, for a windowed zero", () => {
    state.templates = [template({ run_count: 0, window: "7d" })];

    render(<AgentsSection />);

    expect(screen.getByText(/No runs · 7d/)).toBeInTheDocument();
    expect(screen.queryByText(/No runs yet/)).toBeNull();
    expect(screen.queryByText(/0 runs/)).toBeNull();
  });

  /** An all-time zero is the one case that can honestly say "never". */
  it("says a template has never run for an all-time zero", () => {
    state.templates = [template({ run_count: 0, window: "all" })];

    render(<AgentsSection />);

    expect(screen.getByText(/No runs yet/)).toBeInTheDocument();
    expect(screen.queryByText(/0 runs/)).toBeNull();
  });

  /**
   * A daemon older than the field serves a row without it, and `apiFetch`
   * casts rather than validates — so the missing count has to read the same as
   * a zero one. It must never reach the card as the string `undefined`.
   */
  it("reads a row with no run_count as never having run", () => {
    state.templates = [template()];
    delete (state.templates[0] as Record<string, unknown>).run_count;

    render(<AgentsSection />);

    expect(screen.getByText(/No runs/)).toBeInTheDocument();
    expect(screen.queryByText(/undefined/)).toBeNull();
  });

  /** One run is `1 run`, not `1 runs`. */
  it("says 1 run in the singular", () => {
    state.templates = [template({ run_count: 1 })];

    render(<AgentsSection />);

    expect(screen.getByText("1 run · 7d")).toBeInTheDocument();
    expect(screen.queryByText(/1 runs/)).toBeNull();
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
   * The other half of GAP-20 narrowed with C3a: a file saying `disabled:
   * true` switches a template off and the spawn path refuses it, but no
   * route writes that file, so the switch stays disabled and names the file
   * that does the job.
   */
  it("leaves the per-template toggle disabled and attributed", () => {
    state.templates = [template({ run_count: 3 })];

    render(<AgentsSection />);

    const toggle = screen.getByRole("switch");
    expect(toggle).toBeDisabled();
    expect(screen.getByText(/disabled: true/)).toBeInTheDocument();
  });
});

describe("AgentsSection switched-off templates (§7 O-1 (b), C3a)", () => {
  /**
   * A file in `config/agents` saying `disabled: true` switches a template
   * off (subagent design §2.4 L-4a): the daemon lists its row with
   * `state: "disabled"` and the file's `path`. The row says so in words and
   * names the file, and its toggle reads off — still not a control.
   */
  it("shows a switched-off template's chip and file", () => {
    const path = "/Users/x/.openalpaca/config/agents/research_agent.md";
    state.templates = [
      template({
        id: "research_agent",
        name: "research_agent",
        description: "",
        state: "disabled",
        path,
        source: "file",
      }),
    ];

    render(<AgentsSection />);

    expect(screen.getByText("Switched off")).toBeInTheDocument();
    expect(screen.getByText(`Switched off by ${path}`)).toBeInTheDocument();
    const toggle = screen.getByRole("switch");
    expect(toggle).toHaveAttribute("aria-checked", "false");
    expect(toggle).toBeDisabled();
  });

  /** A ready template keeps today's row: no chip, the toggle on. */
  it("leaves a ready template's row as it was", () => {
    state.templates = [template({ state: "ready", source: "bundled" })];

    render(<AgentsSection />);

    expect(screen.queryByText(/Switched off/)).toBeNull();
    expect(screen.getByRole("switch")).toHaveAttribute("aria-checked", "true");
  });
});

describe("AgentsSection sources and copies not used (§2.7 R-4, C5)", () => {
  /**
   * A template built into the daemon reads "Built in", and every definition
   * of its id that does not run — here an earlier build's unedited copy in
   * `config/agents`, set aside so the built-in one runs — gets a line of its
   * own under the row.
   */
  it("shows the built-in chip and each not-used copy", () => {
    const copy = "/Users/x/.openalpaca/config/agents/system_agent.md";
    state.templates = [
      template({
        id: "system_agent",
        name: "System",
        source: "bundled",
        plugin: null,
        path: null,
        overrides_bundled: false,
        state: "ready",
        not_used: [
          {
            source: "file",
            path: copy,
            plugin: null,
            reason: "unedited_copy",
            version: "earlier",
            edit: null,
          },
        ],
        warnings: [],
      }),
    ];

    render(<AgentsSection />);

    expect(screen.getByText("Built in")).toBeInTheDocument();
    expect(
      screen.getByText(
        `An earlier build's unedited copy, ${copy}, is not used`,
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/^Yours/)).toBeNull();
  });

  /** A file of the person's: "Yours", or over a built-in one, says so. */
  it("tells the person's own template from one that replaces a built-in", () => {
    state.templates = [
      template({
        id: "my_agent",
        name: "Mine",
        source: "file",
        path: "/Users/x/.openalpaca/config/agents/my_agent.md",
        overrides_bundled: false,
        state: "ready",
        not_used: [],
        warnings: [],
      }),
      template({
        id: "review_agent",
        name: "Review",
        source: "file",
        path: "/Users/x/.openalpaca/config/agents/review_agent.md",
        overrides_bundled: true,
        state: "ready",
        not_used: [
          {
            source: "bundled",
            path: null,
            plugin: null,
            reason: "overridden",
            version: null,
            edit: null,
          },
        ],
        warnings: [],
      }),
    ];

    render(<AgentsSection />);

    expect(screen.getByText("Yours")).toBeInTheDocument();
    expect(screen.getByText("Yours · replaces built-in")).toBeInTheDocument();
    expect(
      screen.getByText("The built-in template is not used"),
    ).toBeInTheDocument();
  });

  /** A plugin's template names its plugin, and a refused one is listed. */
  it("shows a plugin template's chip with its id", () => {
    state.templates = [
      template({
        id: "notion_writer",
        name: "Notion writer",
        source: "plugin",
        plugin: "notion",
        path: null,
        overrides_bundled: false,
        state: "ready",
        not_used: [
          {
            source: "plugin",
            path: null,
            plugin: "other-notes",
            reason: "plugin_refused",
            version: null,
            edit: null,
          },
        ],
        warnings: [],
      }),
    ];

    render(<AgentsSection />);

    expect(screen.getByText("Plugin · notion")).toBeInTheDocument();
    expect(
      screen.getByText(
        "The template the plugin other-notes offers for this id is not used",
      ),
    ).toBeInTheDocument();
  });

  /**
   * A file that holds the id and does not parse withholds it (L-4): the row
   * reads "Withheld", names the file, and shows the daemon's error.
   */
  it("shows a withheld template's file and error", () => {
    const path = "/Users/x/.openalpaca/config/agents/writing_agent.md";
    const error = `${path} does not parse (missing field \`name\`)`;
    state.templates = [
      template({
        id: "writing_agent",
        name: "writing_agent",
        description: "",
        source: "file",
        path,
        overrides_bundled: false,
        state: "withheld",
        not_used: [],
        warnings: [error],
      }),
    ];

    render(<AgentsSection />);

    expect(screen.getByText("Withheld")).toBeInTheDocument();
    expect(screen.getByText(`Withheld by ${path}`)).toBeInTheDocument();
    expect(screen.getByText(error)).toBeInTheDocument();
  });

  /** A daemon older than the fields: no chip, no line, the row as before. */
  it("draws no source chip for a row without a source", () => {
    state.templates = [template()];

    render(<AgentsSection />);

    for (const chip of ["Built in", "Yours", "Withheld"]) {
      expect(screen.queryByText(chip)).toBeNull();
    }
    expect(screen.queryByText(/not used/)).toBeNull();
  });
});
