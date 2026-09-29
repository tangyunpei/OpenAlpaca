import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { WorkingState } from "@/lib/api/types";

import {
  WORKING_STATE_EMPTY,
  WorkingStateCard,
  refTarget,
} from "./WorkingStateCard";

const RUN_ID = "1a2b3c4d-0000-4000-8000-000000000001";

function lisbon(overrides: Partial<WorkingState> = {}): WorkingState {
  return {
    v: 1,
    topic: "Lisbon \u{202E}trip",
    goal: "Book flights and a hotel for 2–5 October",
    done: ["Friday flight booked"],
    decisions: ["A hotel in Alfama, not Baixa\u{200B}"],
    open: ["Which museum pass to buy"],
    next: "Compare the three Alfama hotels",
    refs: [
      `task ${RUN_ID}`,
      "task 1a2b3c4d",
      "artifact trips/lisbon.md",
      "a note",
    ],
    ...overrides,
  };
}

function renderCard(
  state: WorkingState | null,
  handlers: { onOpenRun?: () => void; onOpenLibrary?: () => void } = {},
) {
  const onOpenRun = vi.fn(handlers.onOpenRun);
  const onOpenLibrary = vi.fn(handlers.onOpenLibrary);
  render(
    <WorkingStateCard
      state={state}
      updatedAt={
        state === null
          ? null
          : new Date(Date.now() - 3 * 60 * 60 * 1000).toISOString()
      }
      onOpenRun={onOpenRun}
      onOpenLibrary={onOpenLibrary}
    />,
  );
  return { onOpenRun, onOpenLibrary };
}

describe("WorkingStateCard (design 11 §5, CQ-7)", () => {
  /**
   * Every string is the summariser's text: a hidden character is drawn as a
   * `<U+XXXX>` escape in its styled span, never acted on — a right-to-left
   * override in the topic must not reorder what the person reads.
   */
  it("draws the state with its hidden characters escaped", () => {
    renderCard(lisbon());
    const card = screen.getByRole("region", { name: "Working state" });

    expect(card.textContent).not.toContain("\u{202E}");
    expect(card.textContent).not.toContain("\u{200B}");
    expect(card).toHaveTextContent("Lisbon <U+202E>trip");
    expect(
      Array.from(card.querySelectorAll("[data-hidden-char]")).map((chip) =>
        chip.getAttribute("data-hidden-char"),
      ),
    ).toEqual(["U+202E", "U+200B"]);

    expect(card).toHaveTextContent(
      "Goal: Book flights and a hotel for 2–5 October",
    );
    expect(within(card).getByText("Friday flight booked")).toBeInTheDocument();
    expect(
      within(card).getByText("Which museum pass to buy"),
    ).toBeInTheDocument();
    expect(card).toHaveTextContent("Next: Compare the three Alfama hotels");
    expect(card).toHaveTextContent("updated 3h ago");
  });

  it("says nothing is summarised yet while the conversation has no state", () => {
    renderCard(null);
    const card = screen.getByRole("region", { name: "Working state" });
    expect(card).toHaveTextContent(WORKING_STATE_EMPTY);
    expect(card).not.toHaveTextContent("updated");
  });

  it("collapses and opens again", () => {
    renderCard(lisbon());
    const toggle = screen.getByRole("button", { name: /Working state/ });
    expect(toggle).toHaveAttribute("aria-expanded", "true");

    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("Friday flight booked")).toBeNull();

    fireEvent.click(toggle);
    expect(screen.getByText("Friday flight booked")).toBeInTheDocument();
  });

  /**
   * A task ref opens its run in Work (the Work view when the id is short), an
   * artifact ref the Library; any other ref is text.
   */
  it("links a task ref to Work and an artifact ref to the Library", () => {
    const { onOpenRun, onOpenLibrary } = renderCard(lisbon());

    fireEvent.click(screen.getByRole("button", { name: `task ${RUN_ID}` }));
    expect(onOpenRun).toHaveBeenLastCalledWith(RUN_ID);
    fireEvent.click(screen.getByRole("button", { name: "task 1a2b3c4d" }));
    expect(onOpenRun).toHaveBeenLastCalledWith(null);
    fireEvent.click(
      screen.getByRole("button", { name: "artifact trips/lisbon.md" }),
    );
    expect(onOpenLibrary).toHaveBeenCalledOnce();

    expect(screen.queryByRole("button", { name: "a note" })).toBeNull();
    expect(screen.getByText("a note")).toBeInTheDocument();
  });

  it("reads a ref's target from its first word", () => {
    expect(refTarget(`Task ${RUN_ID}`)).toEqual({
      kind: "run",
      runId: RUN_ID,
    });
    expect(refTarget("task 1a2b3c4d")).toEqual({ kind: "run", runId: null });
    expect(refTarget("artifact reports/q3.md")).toEqual({ kind: "library" });
    expect(refTarget("reports/q3.md")).toBeNull();
    expect(refTarget("taskforce notes")).toBeNull();
  });
});
