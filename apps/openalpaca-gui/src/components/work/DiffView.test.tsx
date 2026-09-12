import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ArtifactDiffTab, DiffView } from "./DiffView";

const PATCH = [
  "@@ -1,3 +1,4 @@",
  " keep me",
  "-drop me",
  "+add me",
  "+add me too",
].join("\n");

describe("DiffView", () => {
  it("labels the version pair and counts the lines itself", () => {
    render(<DiffView patch={PATCH} size="compact" />);
    expect(screen.getByText("v1 → v2")).toBeInTheDocument();
    expect(screen.getByText("+2")).toBeInTheDocument();
    expect(screen.getByText("−1")).toBeInTheDocument();
  });

  it("paints added, removed and context lines differently", () => {
    const { container } = render(<DiffView patch={PATCH} size="compact" />);
    const added = container.querySelector(".bg-green-diff");
    const removed = container.querySelector(".bg-red-diff");
    expect(added?.textContent).toBe("+add me");
    expect(removed?.textContent).toBe("−drop me");
    // The context line keeps its leading space so a copied diff stays valid.
    const context = container.querySelector(".text-muted-fg");
    expect(context?.textContent).toBe(" keep me");
  });

  it("keeps the hunk header visible so two regions cannot merge silently", () => {
    render(<DiffView patch={PATCH} size="full" />);
    expect(screen.getByText("@@ -1,3 +1,4 @@")).toBeInTheDocument();
  });

  it("shows the times only when they are known", () => {
    const { rerender } = render(<DiffView patch={PATCH} size="full" />);
    expect(screen.queryByText(/·/)).not.toBeInTheDocument();
    rerender(
      <DiffView patch={PATCH} size="full" fromTime="14:02" toTime="14:31" />,
    );
    expect(screen.getByText("14:02 · 14:31")).toBeInTheDocument();
  });
});

describe("ArtifactDiffTab", () => {
  it("renders the daemon's patch", () => {
    render(
      <ArtifactDiffTab
        size="full"
        diff={{
          from: 1,
          to: 2,
          added_lines: 2,
          removed_lines: 1,
          format: "unified",
          patch: PATCH,
        }}
      />,
    );
    expect(screen.getByText("v1 → v2")).toBeInTheDocument();
  });

  /**
   * The two 409s (`NOT_DIFFABLE`, `DIFF_TOO_LARGE`) and "there is only one
   * version" all land here. Each says which it was: an empty diff pane would
   * read as "nothing changed", which is a different claim entirely.
   */
  it("says why there is no patch instead of drawing an empty one", () => {
    render(
      <ArtifactDiffTab
        diff={null}
        note="This kind cannot be diffed — it is not text."
        size="compact"
      />,
    );
    expect(
      screen.getByText("No earlier version to compare against."),
    ).toBeInTheDocument();
    expect(
      screen.getByText("This kind cannot be diffed — it is not text."),
    ).toBeInTheDocument();
    expect(screen.queryByText("v1 → v2")).not.toBeInTheDocument();
  });
});
