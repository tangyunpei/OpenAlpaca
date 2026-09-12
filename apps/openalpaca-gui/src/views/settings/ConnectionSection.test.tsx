/**
 * `StorageCard` — the Settings → Connection Storage card (T44 fix round 1).
 *
 * Tested directly rather than through the whole `ConnectionSection`: it takes
 * a plain `DaemonStatus | undefined` prop, so no daemon-connection or
 * usage/tasks hooks need mocking to exercise its rendering.
 */
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { DaemonStatus, SessionSweep } from "@/lib/api/types";

import { ConnectionSection, StorageCard } from "./ConnectionSection";

/**
 * The Today card reads `GET /v1/usage/summary` (GAP-08c, T50). Every hook it
 * needs is mocked so the card renders against a known payload; `StorageCard`
 * below takes a plain prop and is unaffected by any of this.
 */
const summary = vi.hoisted(() => ({
  data: {
    date: "2026-09-08",
    total_usd: 0.0184,
    by_provider: [
      { provider: "anthropic", usd: 0.0184, calls: 12, tokens: 41_000 },
    ],
    caps: { workflow_max_cost_usd: 5.0, agent_max_cost_usd: 1.0 },
  } as unknown,
}));

vi.mock("@/hooks/useUsage", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/useUsage")>()),
  useUsageSummary: () => ({
    data: summary.data,
    isPending: false,
    error: null,
  }),
}));

// The moved-project offer (§4.8, P-12) reads two queries of its own; this card
// is not what those tests are about, so it is mocked to "nothing has moved".
vi.mock("@/hooks/useWorkspaces", () => ({
  useMovedProject: () => ({ moved: null, pending: false, error: null }),
  useRebaseWorkspace: () => ({
    mutate: vi.fn(),
    isPending: false,
    error: null,
  }),
}));

vi.mock("@/hooks/useConnection", () => ({
  useConnectionStatus: () => ({
    info: null,
    health: undefined,
    socket: "connected",
    connected: true,
    instanceChip: "7f3a",
    endpoint: "127.0.0.1:51823",
    reconnect: vi.fn(),
  }),
  useDaemonStatus: () => ({ data: undefined, isPending: false, error: null }),
}));

vi.mock("@/hooks/useTasks", () => ({
  useTasks: () => ({
    data: [
      { id: "a", created_at: "2026-09-08T09:00:00Z" },
      { id: "b", created_at: "2026-09-08T23:59:00Z" },
      { id: "c", created_at: "2026-09-07T22:00:00Z" },
    ],
    isPending: false,
    error: null,
  }),
}));

function sweep(overrides: Partial<SessionSweep>): SessionSweep {
  return {
    sessions_visited: 12,
    sessions_evicted: 3,
    files_removed: 7,
    bytes_freed: 4_096,
    bytes_before: 9_000,
    bytes_after: 4_904,
    over_cap_after: false,
    index_rows_cleared: 2,
    ...overrides,
  };
}

function status(overrides: Partial<DaemonStatus> = {}): DaemonStatus {
  return {
    home_root: "/home/.openalpaca",
    state_dir: "/home/.openalpaca/state",
    db_path: "/home/.openalpaca/state/openalpaca.db",
    project_root: null,
    started_at: "2026-09-04T00:00:00Z",
    uptime_secs: 10,
    schema_version: 39,
    log_path: null,
    upload_bytes: 0,
    produced_bytes: 0,
    retention: {
      log_max_session_bytes: 256 * 1024 * 1024,
      log_max_total_bytes: 2 * 1024 * 1024 * 1024,
      log_retention_days: 0,
    },
    sessions: { last_sweep: null, dropped_records: 0 },
    routing: { resume_enabled: false },
    ...overrides,
  };
}

describe("StorageCard", () => {
  // Important #1 (T44 fix round 1): the "raise it" advice must name the cap
  // it means — the `retention` block's own `log_max_total_bytes` — not send
  // the owner off with a bare config key and no number to act on.
  it("names the total cap beside the still-over-cap note when nothing was evicted", () => {
    render(
      <StorageCard
        status={status({
          sessions: {
            last_sweep: sweep({ files_removed: 0, over_cap_after: true }),
            dropped_records: 0,
          },
        })}
      />,
    );

    expect(screen.getByText(/2048\.0 MB cap/)).toBeInTheDocument();
    // Minor #3, fixed in the same line: no literal backticks reach the screen.
    expect(screen.queryByText(/`log_max_total_bytes`/)).toBeNull();
  });

  it("names the total cap in the sweep line when eviction still left it over cap", () => {
    render(
      <StorageCard
        status={status({
          sessions: {
            last_sweep: sweep({ files_removed: 7, over_cap_after: true }),
            dropped_records: 0,
          },
        })}
      />,
    );

    expect(
      screen.getByText(/still over the 2048\.0 MB cap/),
    ).toBeInTheDocument();
  });

  it("says nothing about the cap once the sweep brought it back under", () => {
    render(
      <StorageCard
        status={status({
          sessions: {
            last_sweep: sweep({ files_removed: 7, over_cap_after: false }),
            dropped_records: 0,
          },
        })}
      />,
    );

    expect(screen.queryByText(/cap/)).toBeNull();
  });

  it("renders nothing about the sweep before the daemon has answered", () => {
    render(<StorageCard status={undefined} />);

    expect(screen.queryByText(/cap/)).toBeNull();
    expect(screen.getAllByText("—")).toHaveLength(2);
  });
});

describe("the Today card (GAP-08c, T50)", () => {
  /**
   * Spend, tokens and runs are all the daemon's own UTC day — the one `date`
   * names. The client's local date is not consulted anywhere, which is the
   * point: the two disagree for up to twelve hours.
   */
  it("reports spend, tokens and runs for the UTC day the daemon named", () => {
    render(<ConnectionSection />);

    expect(screen.getByText("$0.0184")).toBeInTheDocument();
    expect(screen.getByText("41k")).toBeInTheDocument();
    // Two of the three runs fall on 2026-09-08 UTC.
    expect(screen.getByText("2")).toBeInTheDocument();
  });

  /**
   * N4 on screen: the panel names the two caps that exist instead of implying
   * a daily budget that does not. The design's progress bar has no
   * denominator to draw against, and the copy says why.
   */
  it("names the per-workflow and per-turn caps, never a daily one", () => {
    render(<ConnectionSection />);

    const note = screen.getByText(/not capped daily/);
    expect(note).toHaveTextContent("$5.00 per workflow");
    expect(note).toHaveTextContent("$1.00 per agent turn");
    expect(note.textContent ?? "").not.toMatch(/budget/i);
  });

  /** With no summary yet, the card shows dashes rather than zeroes. */
  it("shows no figures at all until the summary arrives", () => {
    summary.data = undefined;
    try {
      render(<ConnectionSection />);
      expect(screen.getAllByText("—").length).toBeGreaterThanOrEqual(3);
    } finally {
      summary.data = {
        date: "2026-09-08",
        total_usd: 0.0184,
        by_provider: [
          { provider: "anthropic", usd: 0.0184, calls: 12, tokens: 41_000 },
        ],
        caps: { workflow_max_cost_usd: 5.0, agent_max_cost_usd: 1.0 },
      };
    }
  });
});
