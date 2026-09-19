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

import { ASSISTANT_NAME } from "@/components/chat";

import {
  agentDisplayName,
  confirmationBelongsHere,
  MAIN_LOOP_AGENT_ID,
} from "./useChatSession";

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

/**
 * `agentDisplayName` — the other rule both paths share (G10).
 *
 * The live frame and the restored snapshot row carry the same thing, the agent
 * *template* id, so they must read the same name; before this they did not,
 * because only the live path had ever seen an `agent_status` frame to look a
 * name up in.
 */
describe("agentDisplayName", () => {
  const TEMPLATES = new Map([
    ["lead_agent", "Lead Agent"],
    ["general_agent", "General Purpose Agent"],
  ]);

  it("names a template from the daemon's own list, seen or not", () => {
    // The two paths, side by side: with a live `agent_status` name and
    // without one. Same id, same sentence.
    expect(agentDisplayName("lead_agent", TEMPLATES, "Lead Agent")).toBe(
      "Lead Agent",
    );
    expect(agentDisplayName("lead_agent", TEMPLATES)).toBe("Lead Agent");
  });

  /** There is no titlecasing rule to invent: `general_agent` is not "General Agent". */
  it("uses the daemon's name rather than a rule over the id", () => {
    expect(agentDisplayName("general_agent", TEMPLATES)).toBe(
      "General Purpose Agent",
    );
  });

  it("falls back to a live instance name for an id no template answers", () => {
    // A non-singleton instance (`code_agent::a1b2c3d4`) is in no template
    // list, and a window whose template list has not loaded has only this.
    expect(
      agentDisplayName("code_agent::a1b2c3d4", TEMPLATES, "Code Agent"),
    ).toBe("Code Agent");
    expect(agentDisplayName("lead_agent", new Map(), "Lead Agent")).toBe(
      "Lead Agent",
    );
  });

  it("falls back to the id, and never to an empty name", () => {
    expect(agentDisplayName("mystery_agent", TEMPLATES)).toBe("mystery_agent");
    // `AgentStatusChanged.name` is empty when the instance could not be
    // resolved (GAP-07).
    expect(agentDisplayName("mystery_agent", TEMPLATES, "")).toBe(
      "mystery_agent",
    );
  });
});

/**
 * T4 — the main loop has a name now.
 *
 * A main-loop confirmation card read "unknown is blocked on this": the main
 * loop is not an agent template, so its `ToolContext` carried no `agent_id`
 * and every consumer rendered the `None` as the literal `unknown`. The daemon
 * gives it `orchestrator`; this window knows that is the assistant the person
 * is already talking to and calls it what the transcript calls it.
 */
describe("agentDisplayName — the main loop (T4)", () => {
  it("names the main loop after the assistant, not after its id", () => {
    expect(agentDisplayName(MAIN_LOOP_AGENT_ID, new Map())).toBe(
      ASSISTANT_NAME,
    );
    expect(agentDisplayName("orchestrator", new Map())).toBe("Alpaca");
  });

  /** And it answers before any table, because no table will ever hold it. */
  it("answers even when a template list claims the id", () => {
    expect(
      agentDisplayName(
        MAIN_LOOP_AGENT_ID,
        new Map([["orchestrator", "Something Else"]]),
        "Also Something Else",
      ),
    ).toBe(ASSISTANT_NAME);
  });
});
