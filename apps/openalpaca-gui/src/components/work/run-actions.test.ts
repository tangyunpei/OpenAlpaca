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

  it("disables `Start now` and names GAP-06 in the tooltip", () => {
    const action = pauseAction("queued");
    expect(action.enabled).toBe(false);
    expect(action.gap).toBe("GAP-06");
    expect(action.title).toContain("rerun");
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

  it("disables Queue follow-up — there is no write route (GAP-03)", () => {
    expect(byId.get("queue")?.enabled).toBe(false);
    expect(byId.get("queue")?.gap).toBe("GAP-03");
  });

  it("keeps Steer enabled but marked: it aims the composer, GAP-02 is the send", () => {
    const steer = byId.get("steer");
    expect(steer?.enabled).toBe(true);
    expect(steer?.gap).toBe("GAP-02");
    expect(steer?.title).toContain("steer");
  });
});

describe("terminalRunActions", () => {
  it("offers Jump and a disabled Re-run", () => {
    const actions = terminalRunActions();
    expect(actions.map((action) => action.id)).toEqual(["jump", "rerun"]);
    expect(actions[0]?.enabled).toBe(true);
    expect(actions[1]?.enabled).toBe(false);
    expect(actions[1]?.gap).toBe("GAP-06");
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
});

describe("unavailableActionNotes", () => {
  it("lists only the disabled verbs, each naming its proposed route", () => {
    const notes = unavailableActionNotes(liveRunActions("queued"));
    expect(notes).toHaveLength(2);
    expect(notes[0]).toContain("Start now");
    expect(notes[0]).toContain("cancel|pause|resume");
    expect(notes[1]).toContain("Queue follow-up");
    expect(notes.join(" ")).not.toContain("Steer");
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

  it("stays silent for navigation and for actions that never fire", () => {
    expect(actionToast("jump", "x")).toBeNull();
    expect(actionToast("rerun", "x")).toBeNull();
  });
});
