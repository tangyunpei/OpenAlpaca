/**
 * `RunReportCard` — P3, the body that printed its markdown raw.
 *
 * The daemon's `outcome_summary` is the first 500 characters of a report a
 * model wrote, so it arrives with `## Summary`, `**Basics**` and backticks in
 * it. DESIGN_SPEC §3.12 draws this body as one paragraph — the card is a
 * one-glance summary and the full report is the completion message below it —
 * so the markup is reduced away, not rendered.
 */

import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { RunReportCard } from "./RunReportCard";

function card(summary: string | null) {
  return render(
    <RunReportCard
      status="done"
      time="15:52"
      runId="f7308e1c"
      duration="2m 04s"
      title="Alpaca fibre notes"
      summary={summary}
    />,
  );
}

describe("RunReportCard — the summary is prose, not markup (P3)", () => {
  it("strips the markup a model writes its report in", () => {
    const { container } = card(
      "## Summary\n\n**Basics**: the fibre is hollow.\n\n- wrote `01-notes.md`\n- 3 sources",
    );
    const body = container.querySelector("p.whitespace-pre-line");
    expect(body?.textContent).toBe(
      "Summary\nBasics: the fibre is hollow.\nwrote 01-notes.md\n3 sources",
    );
    // Nothing is *rendered* as markup either: the card has no headings, no
    // lists and no code chips — it is one paragraph.
    expect(container.querySelector("h2")).toBeNull();
    expect(container.querySelector("ul")).toBeNull();
    expect(container.querySelector("code")).toBeNull();
  });

  it("leaves a plain summary exactly as the daemon sent it", () => {
    card("Wrote the notes and checked three sources.");
    expect(
      screen.getByText("Wrote the notes and checked three sources."),
    ).toBeInTheDocument();
  });

  it("shows no paragraph at all for a run that reported nothing", () => {
    const { container } = card(null);
    expect(container.querySelector("p.whitespace-pre-line")).toBeNull();
    expect(screen.getByText("Alpaca fibre notes")).toBeInTheDocument();
  });

  /** A summary that is only markup has nothing to say, and says nothing. */
  it("draws no empty paragraph for a summary with no words in it", () => {
    const { container } = card("---");
    expect(container.textContent).not.toContain("---");
    expect(container.querySelector("p.whitespace-pre-line")).toBeNull();
  });
});
