/**
 * `StorageCard` — the Settings → Connection Storage card (T44 fix round 1).
 *
 * Tested directly rather than through the whole `ConnectionSection`: it takes
 * a plain `DaemonStatus | undefined` prop, so no daemon-connection or
 * usage/tasks hooks need mocking to exercise its rendering.
 */
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type { DaemonStatus, SessionSweep } from "@/lib/api/types";

import { StorageCard } from "./ConnectionSection";

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
