/**
 * `confirmationBelongsHere` — the one rule both ways a prompt reaches this
 * view are filtered by (the live `tool_confirmation_requested` frame and S9's
 * `GET /v1/chat/confirmations` snapshot).
 *
 * Pinned here as a table because accepting a prompt blocks this composer and
 * arms Enter-approve: the rows it *rejects* are as much the contract as the
 * ones it takes, and V9 asked specifically what happens to the `lane_key:
 * null` rows a lead-agent run raises.
 */

import { describe, expect, it } from "vitest";

import { confirmationBelongsHere } from "./useChatSession";

const HERE = "user:gui";

/** This window: lane `user:gui`, stream `stream-1`, and it started `run-1`. */
function here(
  overrides: Partial<Parameters<typeof confirmationBelongsHere>[1]> = {},
) {
  return {
    streamId: "stream-1",
    laneKey: HERE,
    startedHere: (taskId: string) => taskId === "run-1",
    ...overrides,
  };
}

describe("confirmationBelongsHere", () => {
  it("takes this stream's own prompt", () => {
    expect(
      confirmationBelongsHere(
        { stream_id: "stream-1", task_id: null, lane_key: "someone:telegram" },
        here(),
      ),
    ).toBe(true);
  });

  /**
   * The case V9 names: `SandboxPolicy` carries a lane only on the main-loop
   * policy, so every confirmation raised inside a workflow — which is every
   * lead-agent run — is served with `lane_key: null`. The run this window
   * started is what identifies it.
   */
  it("takes a lane-less prompt from a run this window started", () => {
    expect(
      confirmationBelongsHere({ task_id: "run-1", lane_key: null }, here()),
    ).toBe(true);
  });

  /**
   * `task_id` is what rescues a row the lane test would throw away: a prompt
   * *is* answerable from the window that launched the run, whatever lane the
   * daemon recorded on it.
   */
  it("takes a foreign-lane prompt from a run this window started", () => {
    expect(
      confirmationBelongsHere(
        { task_id: "run-1", lane_key: "someone:telegram" },
        here(),
      ),
    ).toBe(true);
  });

  /**
   * And a lane-less prompt from a run this window knows nothing about is
   * still kept — deliberately (S9): dropping those would leave the GUI's own
   * background runs unanswerable, and the CLI and GUI share one lane, so
   * "not started in this window" does not mean "not this owner's".
   */
  it("keeps a lane-less prompt it cannot place at all", () => {
    expect(
      confirmationBelongsHere(
        { task_id: "run-elsewhere", lane_key: null },
        here(),
      ),
    ).toBe(true);
    expect(
      confirmationBelongsHere({ task_id: null, lane_key: null }, here()),
    ).toBe(true);
  });

  it("takes a prompt raised on this very lane", () => {
    expect(
      confirmationBelongsHere({ task_id: null, lane_key: HERE }, here()),
    ).toBe(true);
  });

  it("refuses another lane's prompt for a run it did not start", () => {
    expect(
      confirmationBelongsHere(
        { task_id: "run-foreign", lane_key: "someone:telegram" },
        here(),
      ),
    ).toBe(false);
  });

  /** A snapshot row carries no `stream_id` at all; that must not match. */
  it("does not treat a missing stream id as this stream", () => {
    expect(
      confirmationBelongsHere(
        { task_id: "run-foreign", lane_key: "someone:telegram" },
        here({ streamId: null }),
      ),
    ).toBe(false);
    expect(
      confirmationBelongsHere(
        { stream_id: null, task_id: "run-foreign", lane_key: "someone:cli" },
        here({ streamId: null }),
      ),
    ).toBe(false);
  });
});
