import { describe, expect, it } from "vitest";

import {
  actionToast,
  gapTooltip,
  liveRunActions,
  pauseAction,
  runActions,
  terminalRunActions,
  unavailableActionNotes,
} from "./run-actions";

describe("pauseAction", () => {
  it("labels by status, per §3.19", () => {
    expect(pauseAction("running").label).toBe("Pause");
    expect(pauseAction("paused").label).toBe("Resume");
    expect(pauseAction("queued").label).toBe("Start now");
  });

  it("wires pause and resume to the real endpoint", () => {
    expect(pauseAction("running").enabled).toBe(true);
    expect(pauseAction("paused").enabled).toBe(true);
  });

  // GAP-06 closed: `POST /v1/tasks/{id}/action { action: "start" }` dispatches
  // the queued row under its own id (D5), so the control is real and carries
  // no gap or apologetic tooltip.
  it("keeps `Start now` enabled and unmarked — the dispatch exists now", () => {
    const action = pauseAction("queued");
    expect(action.enabled).toBe(true);
    expect(action.gap).toBeUndefined();
    expect(action.title).toBeUndefined();
  });
});

describe("liveRunActions", () => {
  const actions = liveRunActions("running");
  const byId = new Map(actions.map((action) => [action.id, action]));

  it("renders the design's five controls in order", () => {
    expect(actions.map((action) => action.id)).toEqual([
      "pause",
      "steer",
      "queue",
      "jump",
      "cancel",
    ]);
  });

  it("keeps Cancel a real, enabled danger control", () => {
    expect(byId.get("cancel")?.enabled).toBe(true);
    expect(byId.get("cancel")?.tone).toBe("danger");
  });

  // GAP-03 closed: `POST /v1/lanes/{lane_key}/followups` parks the text on the
  // lane, so the control carries no gap and no apologetic tooltip any more.
  it("keeps Queue follow-up enabled and unmarked — the write route exists now", () => {
    const queue = byId.get("queue");
    expect(queue?.enabled).toBe(true);
    expect(queue?.gap).toBeUndefined();
    expect(queue?.title).toBeUndefined();
  });

  /**
   * Queueing is *not* gated on `steerable`: a follow-up lands on the lane and
   * is claimed when the current workflow finalizes, so a run that has stopped
   * taking steering messages can still have work parked behind it.
   */
  it("leaves Queue follow-up enabled even when Steer is not", () => {
    const queue = liveRunActions("running", "Run has finished.").find(
      (action) => action.id === "queue",
    );
    expect(queue?.enabled).toBe(true);
  });

  // GAP-02 closed: the send behind the composer is `POST /v1/tasks/{id}/steer`,
  // so the control carries no gap and no apologetic tooltip any more.
  it("keeps Steer enabled and unmarked — the addressed send exists now", () => {
    const steer = byId.get("steer");
    expect(steer?.enabled).toBe(true);
    expect(steer?.gap).toBeUndefined();
    expect(steer?.title).toBeUndefined();
  });

  /**
   * R40 — the run row's own `steerable` hint. A run the daemon would refuse
   * gets a disabled control whose tooltip says why, rather than a button that
   * answers 404 the first time it is used.
   */
  it("disables Steer with the reason when the run cannot take one", () => {
    const steer = liveRunActions(
      "running",
      "Started from another channel.",
    ).find((action) => action.id === "steer");
    expect(steer?.enabled).toBe(false);
    expect(steer?.title).toBe("Started from another channel.");
    // Not a missing API — no gap id, so it stays out of the gap footnote.
    expect(steer?.gap).toBeUndefined();
  });

  it("leaves the other four controls alone when Steer is disabled", () => {
    const actions = liveRunActions("running", "Run has finished.");
    expect(actions.map((action) => action.id)).toEqual([
      "pause",
      "steer",
      "queue",
      "jump",
      "cancel",
    ]);
    expect(actions.find((a) => a.id === "cancel")?.enabled).toBe(true);
    expect(unavailableActionNotes(actions).join(" ")).not.toContain("Steer");
  });
});

describe("terminalRunActions", () => {
  // GAP-06's other half: `POST /v1/tasks/{id}/rerun` dispatches a new run from
  // this one's goal, so `Re-run` is a real control on a finished run.
  it("offers Jump and a live Re-run", () => {
    const actions = terminalRunActions();
    expect(actions.map((action) => action.id)).toEqual(["jump", "rerun"]);
    expect(actions[0]?.enabled).toBe(true);
    expect(actions[1]?.enabled).toBe(true);
    expect(actions[1]?.gap).toBeUndefined();
    expect(actions[1]?.title).toBeUndefined();
  });
});

describe("runActions", () => {
  it("picks the terminal set for every terminal status", () => {
    for (const status of ["done", "cancelled", "failed"] as const) {
      expect(runActions(status).map((action) => action.id)).toEqual([
        "jump",
        "rerun",
      ]);
    }
  });

  it("picks the live set otherwise", () => {
    expect(runActions("queued")).toHaveLength(5);
  });

  it("passes the steer reason through to the live set", () => {
    const steer = runActions("running", "Run has finished.").find(
      (action) => action.id === "steer",
    );
    expect(steer?.enabled).toBe(false);
    expect(steer?.title).toBe("Run has finished.");
  });
});

describe("unavailableActionNotes", () => {
  // Nothing in the action bar is gapped any more — GAP-06 was the last one —
  // so the footnote is empty on every run. The mechanism stays: it is what a
  // future disabled verb would report through.
  it("has nothing to say now that every run verb reaches the daemon", () => {
    for (const status of ["queued", "running", "paused", "done"] as const) {
      expect(unavailableActionNotes(runActions(status))).toEqual([]);
    }
  });

  it("still lists a gapped action, naming its proposed route", () => {
    const notes = unavailableActionNotes([
      {
        id: "start",
        label: "Start now",
        tone: "secondary",
        enabled: false,
        gap: "GAP-14",
      },
    ]);
    expect(notes).toHaveLength(1);
    expect(notes[0]).toContain("Start now");
    expect(notes[0]).toContain("/v1/status");
  });

  it("is empty when every action works", () => {
    expect(
      unavailableActionNotes([
        { id: "cancel", label: "Cancel", tone: "danger", enabled: true },
      ]),
    ).toEqual([]);
  });
});

describe("gapTooltip", () => {
  it("carries both the note and the proposal", () => {
    const tooltip = gapTooltip("GAP-08c");
    expect(tooltip).toContain("Spend is not capped daily by design");
    expect(tooltip).toContain("/v1/usage/summary");
  });
});

describe("actionToast", () => {
  it("uses §4.4's copy for the three real verbs", () => {
    expect(actionToast("pause", "Connector audit")).toBe(
      "Connector audit paused",
    );
    expect(actionToast("resume", "Connector audit")).toBe(
      "Connector audit resumed",
    );
    expect(actionToast("cancel", "Connector audit")).toBe(
      "Connector audit cancelled",
    );
  });

  it("names the run that started, for D5's same-id dispatch", () => {
    expect(actionToast("start", "Connector audit")).toBe(
      "Connector audit started",
    );
  });

  // `rerun` produces a run the user has not seen, so its toast is written by
  // the controller from the *response*, not from the card's title.
  it("stays silent for navigation and for the verb that answers a new id", () => {
    expect(actionToast("jump", "x")).toBeNull();
    expect(actionToast("rerun", "x")).toBeNull();
  });
});
