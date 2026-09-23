/**
 * `StorageCard` — the Settings → Connection Storage card (T44 fix round 1).
 *
 * Tested directly rather than through the whole `ConnectionSection`: it takes
 * a plain `DaemonStatus | undefined` prop, so no daemon-connection or
 * usage/tasks hooks need mocking to exercise its rendering.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { DaemonStatus, SessionSweep } from "@/lib/api/types";
import { useUiStore } from "@/stores/ui";

import {
  ConnectionSection,
  DAEMON_LOG_LINES,
  StorageCard,
  homeStoreLabel,
} from "./ConnectionSection";

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

/** What `GET /v1/status` has answered, per test. */
const daemon = vi.hoisted(() => ({ data: undefined as unknown }));

/** The window's own view of the connection, mutable per test. */
const link = vi.hoisted(() => ({
  connected: true,
  lastError: null as string | null,
  stopIntent: null as "stopped_here" | "stopped_elsewhere" | null,
  start: vi.fn(),
  reconnect: vi.fn(),
  refetch: vi.fn(() => Promise.resolve()),
}));

vi.mock("@/hooks/useConnection", () => ({
  useConnectionStatus: () => ({
    info: null,
    health: undefined,
    socket: link.connected ? "connected" : "error",
    connected: link.connected,
    instanceChip: "7f3a",
    endpoint: "127.0.0.1:51823",
    lastError: link.lastError,
    stopIntent: link.stopIntent,
    stoppedAt: link.stopIntent === null ? null : Date.now(),
    reconnect: link.reconnect,
    start: link.start,
  }),
  useDaemonStatus: () => ({
    data: daemon.data,
    isPending: false,
    error: null,
    refetch: link.refetch,
  }),
}));

/** Connectors, read only by the stop dialog (T32's silence line). */
const connectorRows = vi.hoisted(() => ({ data: [] as unknown[] }));
vi.mock("@/hooks/useConnectors", () => ({
  useConnectors: () => ({
    data: connectorRows.data,
    isPending: false,
    error: null,
  }),
}));

/** The stop itself — the flow's own tests are `lib/daemon-control.test.ts`. */
const control = vi.hoisted(() => ({ stop: vi.fn() }));
vi.mock("@/lib/daemon-control", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/daemon-control")>()),
  stopDaemon: control.stop,
}));

/** The shell's `read_daemon_log_tail`, as the section reaches it. */
const shellLog = vi.hoisted(() => ({ read: vi.fn() }));

vi.mock("@/lib/connection", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/connection")>()),
  readDaemonLogTail: shellLog.read,
}));

afterEach(() => {
  link.connected = true;
  link.lastError = null;
  link.stopIntent = null;
  link.start.mockReset();
  link.reconnect.mockReset();
  link.refetch.mockReset();
  link.refetch.mockImplementation(() => Promise.resolve());
  control.stop.mockReset();
  connectorRows.data = [];
  shellLog.read.mockReset();
  daemon.data = undefined;
});

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

/**
 * G7 — the card printed the literal `~/.openalpaca` as where a project-less
 * run's files go. `OPENALPACA_HOME_STORE` moves that root, and the daemon
 * reports where it really is (`GET /v1/status`'s `home_root`), so printing the
 * default was a confident answer to a question this side cannot answer.
 */
describe("where a project-less run's files go (G7)", () => {
  it("names the daemon's own root, and stays generic until it has said", () => {
    expect(homeStoreLabel("/Volumes/work/store")).toBe(
      "the home store (/Volumes/work/store)",
    );
    expect(homeStoreLabel(null)).toBe("the home store");
    expect(homeStoreLabel("  ")).toBe("the home store");
  });

  it("prints the overridden root in the project card", () => {
    daemon.data = status({ home_root: "/Volumes/work/store" });
    render(<ConnectionSection />);

    expect(
      screen.getByText(/files go to the home store \(\/Volumes\/work\/store\)/),
    ).toBeInTheDocument();
    expect(screen.queryByText(/~\/\.openalpaca/)).toBeNull();
    daemon.data = undefined;
  });
});

/**
 * A daemon that would not start used to say nothing: the sidecar's output
 * went to `/dev/null`, and the one string that reached the window was "did
 * not become ready within timeout". The shell's rejection now ends with the
 * daemon's own log lines, and the panel shows them exactly as written.
 */
describe("why the daemon would not start", () => {
  const refusal =
    "The daemon did not start within 5 seconds. Its log (/s/state/logs/daemon.log) ends with:\n" +
    "\n" +
    "2026-09-22T10:00:00Z ERROR openalpacad: FATAL: an older OpenAlpaca install's data is still on this machine, and this build\n" +
    "does not move it for you.\n" +
    "\n" +
    "  Older install:  /old";

  it("renders the shell's reason verbatim, line breaks and all, while unreachable", () => {
    link.connected = false;
    link.lastError = refusal;
    render(<ConnectionSection />);

    const block = screen.getByLabelText("Why the daemon is unreachable");
    expect(block.tagName).toBe("PRE");
    expect(block.textContent).toBe(refusal);
  });

  it("shows no reason once the window is connected, even if one was recorded", () => {
    link.connected = true;
    link.lastError = refusal;
    render(<ConnectionSection />);

    expect(screen.queryByLabelText("Why the daemon is unreachable")).toBeNull();
  });

  it("shows nothing when unreachable with no reason recorded", () => {
    link.connected = false;
    link.lastError = null;
    render(<ConnectionSection />);

    expect(screen.queryByLabelText("Why the daemon is unreachable")).toBeNull();
  });
});

describe("Show daemon log", () => {
  it("reads the last 200 lines through the shell and shows them as written", async () => {
    const user = userEvent.setup();
    shellLog.read.mockResolvedValue("line one\n\n  indented line two");
    render(<ConnectionSection />);

    await user.click(screen.getByRole("button", { name: "Show daemon log" }));

    expect(shellLog.read).toHaveBeenCalledWith(DAEMON_LOG_LINES);
    expect(DAEMON_LOG_LINES).toBe(200);
    const block = await screen.findByLabelText("Daemon log");
    expect(block.textContent).toBe("line one\n\n  indented line two");
    expect(
      screen.getByRole("button", { name: "Hide daemon log" }),
    ).toHaveAttribute("aria-expanded", "true");
  });

  it("reads again on Refresh, and never before it is opened", async () => {
    const user = userEvent.setup();
    shellLog.read.mockResolvedValue("first");
    render(<ConnectionSection />);
    expect(shellLog.read).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Show daemon log" }));
    await screen.findByText("first");
    shellLog.read.mockResolvedValue("second");
    await user.click(screen.getByRole("button", { name: "Refresh" }));

    expect(await screen.findByText("second")).toBeInTheDocument();
    expect(shellLog.read).toHaveBeenCalledTimes(2);
  });

  it("says the log is empty rather than drawing an empty block", async () => {
    const user = userEvent.setup();
    shellLog.read.mockResolvedValue("");
    render(<ConnectionSection />);

    await user.click(screen.getByRole("button", { name: "Show daemon log" }));

    expect(
      await screen.findByText("The daemon log is empty."),
    ).toBeInTheDocument();
    expect(screen.queryByLabelText("Daemon log")).toBeNull();
  });

  it("says why when the shell cannot read it", async () => {
    const user = userEvent.setup();
    shellLog.read.mockRejectedValue(new Error("permission denied"));
    render(<ConnectionSection />);

    await user.click(screen.getByRole("button", { name: "Show daemon log" }));

    expect(
      await screen.findByText(
        "Could not read the daemon log: permission denied",
      ),
    ).toBeInTheDocument();
  });
});

/**
 * T30: both launchers write `daemon.log` now, so a `null` log path means a
 * daemon started by hand — the note used to blame "a daemon the app launched
 * itself", which is exactly the one that has a log now.
 */
describe("a daemon with no log of its own (T30)", () => {
  it("says it was started by hand, not that the app launched it", () => {
    daemon.data = status({ log_path: null });
    render(<ConnectionSection />);

    const note = screen.getByText(/no daemon\.log/i);
    expect(note).toHaveTextContent("started by hand");
    expect(note.textContent ?? "").not.toMatch(/launched itself/);
  });
});

describe("Stop daemon (§5.5)", () => {
  it("opens the confirmation and sends nothing until it is confirmed", async () => {
    const user = userEvent.setup();
    control.stop.mockResolvedValue({ kind: "stopped" });
    daemon.data = status({
      busy: {
        running_tasks: 1,
        pending_confirmations: 0,
        connected_clients: 2,
      },
    });
    render(<ConnectionSection />);

    await user.click(screen.getByRole("button", { name: "Stop daemon…" }));

    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent(
      "Right now: 1 workflow running · 2 windows connected.",
    );
    // The busy count is re-read on open, and nothing is stopped by opening.
    expect(link.refetch).toHaveBeenCalledTimes(1);
    expect(control.stop).not.toHaveBeenCalled();

    const stop = await screen.findByRole("button", { name: "Stop daemon" });
    await vi.waitFor(() => expect(stop).toBeEnabled());
    await user.click(stop);

    expect(control.stop).toHaveBeenCalledTimes(1);
    await vi.waitFor(() =>
      expect(useUiStore.getState().toast).toBe("Daemon stopped."),
    );
  });

  it("keeps the dialog's Stop disabled while the fresh read is in flight", async () => {
    const user = userEvent.setup();
    link.refetch.mockImplementation(() => new Promise(() => {}));
    daemon.data = status();
    render(<ConnectionSection />);

    await user.click(screen.getByRole("button", { name: "Stop daemon…" }));

    expect(screen.getByRole("button", { name: "Stop daemon" })).toBeDisabled();
  });

  it("warns of connector silence only when a connector is running", async () => {
    const user = userEvent.setup();
    connectorRows.data = [{ id: "telegram", status: "running" }];
    daemon.data = status();
    render(<ConnectionSection />);

    await user.click(screen.getByRole("button", { name: "Stop daemon…" }));

    expect(screen.getByRole("dialog")).toHaveTextContent(
      /Telegram, Discord or iMessage will get no reply and no error/,
    );
  });

  it("cannot be asked of a daemon this window cannot reach", () => {
    link.connected = false;
    render(<ConnectionSection />);

    expect(screen.getByRole("button", { name: "Stop daemon…" })).toBeDisabled();
  });
});

/**
 * §6.3: a stopped daemon is a state, not an incident. The card says who
 * stopped it in the neutral token, `Start daemon` replaces `Reconnect`, and
 * no stale failure is drawn as if it were the reason.
 */
describe("while the daemon is stopped", () => {
  it("says this window stopped it and offers Start, not Reconnect or Stop", async () => {
    const user = userEvent.setup();
    link.connected = false;
    link.stopIntent = "stopped_here";
    link.lastError = "WebSocket connection error";
    render(<ConnectionSection />);

    expect(screen.getByText("Daemon stopped")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "stopped" })).toBeInTheDocument();
    expect(screen.getByText(/^stopped /)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Reconnect" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Stop daemon…" })).toBeNull();
    expect(screen.queryByLabelText("Why the daemon is unreachable")).toBeNull();

    await user.click(screen.getByRole("button", { name: "Start daemon" }));
    expect(link.start).toHaveBeenCalledTimes(1);
    expect(link.reconnect).not.toHaveBeenCalled();
  });

  it("says when it was stopped from elsewhere", () => {
    link.connected = false;
    link.stopIntent = "stopped_elsewhere";
    render(<ConnectionSection />);

    expect(
      screen.getByText("Daemon stopped from elsewhere"),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Start daemon" }),
    ).toBeInTheDocument();
  });
});
