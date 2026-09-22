/**
 * The resolution rows and what may rewrite them (G6, F5).
 *
 * `noteToolRun` is the upgrade pass: a `tool_executed` frame turns an approved
 * row's honest "waiting for the tool to run…" into the outcome. The socket is
 * the daemon-wide firehose, so the pass has to be able to say which row the
 * frame belongs to — matching on the tool's name alone let any agent's call
 * anywhere in the daemon rewrite this window's row.
 */

import { beforeEach, describe, expect, it } from "vitest";

import { useResolutions } from "./resolution-store";
import type { ResolutionEntry } from "./transcript-model";

const MAIN_LOOP = { agentId: "orchestrator", taskId: null };

function waiting(patch: Partial<ResolutionEntry> = {}): ResolutionEntry {
  return {
    requestId: "req-1",
    resolution: "approved",
    note: "shell_execute approved · waiting for the tool to run…",
    at: "2026-09-20T10:00:00Z",
    agentId: "orchestrator",
    taskId: null,
    ...patch,
  };
}

const note = (row: ResolutionEntry | undefined) => row?.note;

describe("noteToolRun", () => {
  beforeEach(() => {
    useResolutions.getState().clear();
  });

  it("upgrades the row the execution belongs to", () => {
    useResolutions.getState().add(waiting());
    useResolutions
      .getState()
      .noteToolRun(
        "shell_execute",
        MAIN_LOOP,
        "shell_execute approved · returned in 1.4s, the agent resumed.",
      );
    expect(note(useResolutions.getState().rows[0])).toBe(
      "shell_execute approved · returned in 1.4s, the agent resumed.",
    );
  });

  /**
   * The case this exists for: chat runs alongside workflows, so a subagent
   * finishing its own `shell_execute` 400 ms after the user clicked Approve
   * used to write "failed after 200ms" onto the user's row — and the user's
   * own call, reporting 40 s later, then found no row still waiting.
   */
  it("leaves a row whose agent and run differ untouched", () => {
    useResolutions.getState().add(waiting());
    useResolutions
      .getState()
      .noteToolRun(
        "shell_execute",
        { agentId: "researcher", taskId: "a1b2" },
        "shell_execute approved · failed after 200ms, the agent continued without it.",
      );
    expect(note(useResolutions.getState().rows[0])).toMatch(/waiting/);
  });

  it("leaves a row of the same agent in another run untouched", () => {
    useResolutions
      .getState()
      .add(waiting({ agentId: "lead_agent", taskId: "c3d4" }));
    useResolutions
      .getState()
      .noteToolRun(
        "shell_execute",
        { agentId: "lead_agent", taskId: "e5f6" },
        "shell_execute approved · returned in 0.2s, the agent resumed.",
      );
    expect(note(useResolutions.getState().rows[0])).toMatch(/waiting/);
  });

  /** Nothing to correlate on: the honest "waiting" beats a borrowed outcome. */
  it("never upgrades a row that names no agent", () => {
    useResolutions.getState().add(waiting({ agentId: null }));
    useResolutions
      .getState()
      .noteToolRun(
        "shell_execute",
        MAIN_LOOP,
        "shell_execute approved · returned in 1.4s, the agent resumed.",
      );
    expect(note(useResolutions.getState().rows[0])).toMatch(/waiting/);
  });

  it("upgrades only the matching row when several are waiting", () => {
    useResolutions.getState().add(waiting());
    useResolutions
      .getState()
      .add(
        waiting({ requestId: "req-2", agentId: "researcher", taskId: "a1" }),
      );
    useResolutions
      .getState()
      .noteToolRun(
        "shell_execute",
        { agentId: "researcher", taskId: "a1" },
        "shell_execute approved · returned in 0.4s, the agent resumed.",
      );
    const rows = useResolutions.getState().rows;
    expect(note(rows[0])).toMatch(/waiting/);
    expect(note(rows[1])).toMatch(/returned in 0\.4s/);
  });

  it("leaves a denial alone — nothing is going to run", () => {
    useResolutions
      .getState()
      .add(
        waiting({ resolution: "denied", note: "shell_execute denied · skip" }),
      );
    useResolutions
      .getState()
      .noteToolRun("shell_execute", MAIN_LOOP, "rewritten");
    expect(note(useResolutions.getState().rows[0])).toBe(
      "shell_execute denied · skip",
    );
  });
});
