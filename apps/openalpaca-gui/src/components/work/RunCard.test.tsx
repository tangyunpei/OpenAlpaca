import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { unavailable } from "@/lib/unavailable";

import { RunCard } from "./RunCard";
import type { Run } from "./run-model";

const TIMELINE = unavailable("GAP-09");

const run = (patch: Partial<Run> = {}): Run => ({
  id: "b41c8e02",
  title: "Audit the connector surface",
  status: "running",
  meta: "11m 04s · 5/8 steps",
  started: "14:22:41",
  stamp: null,
  note: "cargo tree is waiting on you",
  laneKey: "local:gui",
  artifactCount: 0,
  artifacts: [],
  finishedAt: null,
  ...patch,
});

const card = (
  patch: Partial<Run> = {},
  props: Partial<Parameters<typeof RunCard>[0]> = {},
) =>
  render(
    <RunCard
      run={run(patch)}
      timeline={TIMELINE}
      onAction={vi.fn()}
      {...props}
    />,
  );

describe("RunCard (§3.19)", () => {
  it("raises a running card and flattens a queued one", () => {
    const { container, unmount } = card({ status: "running" });
    expect(container.querySelector("article")?.className).toContain(
      "bg-raised",
    );
    unmount();

    const queued = card({ status: "queued" });
    expect(queued.container.querySelector("article")?.className).toContain(
      "bg-inactive",
    );
  });

  it("shows the five live controls, with the gapped two disabled", () => {
    card({ status: "running" });
    expect(screen.getByRole("button", { name: "Pause" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Steer" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Jump to chat" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeEnabled();
    expect(
      screen.getByRole("button", { name: "Queue follow-up" }),
    ).toBeDisabled();
  });

  it("offers `Start now` on a queued run, disabled and explained", () => {
    card({ status: "queued" });
    const start = screen.getByRole("button", { name: "Start now" });
    expect(start).toBeDisabled();
    expect(start).toHaveAttribute("title", expect.stringContaining("rerun"));
  });

  it("replaces the bar with a note and a disabled Re-run when terminal", () => {
    card({ status: "done", note: "wrote 2 files" });
    expect(screen.getByText("wrote 2 files")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Re-run" })).toBeDisabled();
    expect(
      screen.queryByRole("button", { name: "Cancel" }),
    ).not.toBeInTheDocument();
  });

  it("colours the note dot red only while this run is blocked (§4.4)", () => {
    const { container, unmount } = card(
      { status: "running" },
      { blocked: true },
    );
    expect(container.querySelector(".bg-red")).not.toBeNull();
    unmount();

    const resolved = card({ status: "running" }, { blocked: false });
    expect(resolved.container.querySelector(".bg-red")).toBeNull();
    expect(resolved.container.querySelector(".bg-green")).not.toBeNull();
  });

  it("hides the parallel-work block on a run that is not running or paused", () => {
    card({ status: "queued" });
    expect(screen.queryByText("Parallel work")).not.toBeInTheDocument();
  });

  it("lists the files the outcome reported and names the missing API", () => {
    card(
      {
        artifactCount: 2,
        artifacts: [
          { id: null, name: "findings.md", kind: "md", stamp: "14:31" },
          { id: null, name: "audit.csv", kind: "table", stamp: null },
        ],
      },
      { filesNote: "Artifact API not yet available" },
    );
    expect(screen.getByText("Files · 2")).toBeInTheDocument();
    expect(screen.getByText("findings.md")).toBeInTheDocument();
    expect(
      screen.getByText("Artifact API not yet available"),
    ).toBeInTheDocument();
  });

  it("does not make a file row clickable when it has no id", () => {
    card({
      artifactCount: 1,
      artifacts: [{ id: null, name: "findings.md", kind: "md", stamp: null }],
    });
    expect(
      screen.queryByRole("button", { name: /findings\.md/ }),
    ).not.toBeInTheDocument();
  });

  it("reports the action and the run it belongs to", async () => {
    const onAction = vi.fn();
    card({ status: "running" }, { onAction });
    await userEvent.click(screen.getByRole("button", { name: "Pause" }));
    expect(onAction).toHaveBeenCalledWith(
      "pause",
      expect.objectContaining({ id: "b41c8e02" }),
    );
  });
});
