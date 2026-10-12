import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { Project, ProjectDataDeletion } from "@/lib/api/workspaces";

import {
  DeleteProjectDataDialog,
  deleteCountsLine,
  keptLine,
  type DeleteProjectDataDialogProps,
} from "./DeleteProjectDataDialog";

const REPO: Project = {
  id: "6f1c2a3b-4d5e-4f60-8a7b-9c0d1e2f3a4b",
  root: "/Users/me/dev/repo",
  name: "-Users-me-dev-repo",
  missing: false,
};

const ZERO = {
  sessions: 0,
  messages: 0,
  tool_calls: 0,
  followups: 0,
  tasks: 0,
  spans: 0,
  run_events: 0,
  uploads: 0,
  artifacts: 0,
  memory_notes: 0,
};

const PLAN: ProjectDataDeletion = {
  dry_run: true,
  applied: false,
  project: REPO,
  // Two of the nine rows S5a's `plan_entries` writes, as it words them.
  entries: [
    {
      entry: "projects/-Users-me-dev-repo/",
      holds:
        "project.json — the project's id and path; OpenAlpaca forgets the project",
      retention: "store metadata",
      action: "delete",
    },
    {
      entry: "uploads/ (root store)",
      holds:
        "1 uploads another conversation still shows, moved here under kept/",
      retention:
        "swept: an upload attached to no message is deleted once past the grace period",
      action: "keep",
    },
  ],
  counts: {
    ...ZERO,
    sessions: 2,
    messages: 41,
    tasks: 3,
    artifacts: 4,
    memory_notes: 12,
  },
  kept: { uploads: 1, artifacts: 0 },
  removed: null,
};

function renderDialog(overrides: Partial<DeleteProjectDataDialogProps> = {}) {
  const props: DeleteProjectDataDialogProps = {
    project: REPO,
    plan: PLAN,
    planError: null,
    refusal: null,
    deleting: false,
    onCancel: vi.fn(),
    onConfirm: vi.fn(),
    ...overrides,
  };
  render(<DeleteProjectDataDialog {...props} />);
  return props;
}

describe("deleteCountsLine", () => {
  it("names the non-zero counts in the owner's words, with singulars", () => {
    expect(deleteCountsLine(PLAN.counts)).toBe(
      "Deletes 2 conversations · 41 messages · 3 runs · 4 by-products · 12 memory notes.",
    );
    expect(deleteCountsLine({ ...ZERO, sessions: 1, uploads: 1 })).toBe(
      "Deletes 1 conversation · 1 upload.",
    );
  });

  it("says when nothing but the project's folder is recorded", () => {
    expect(deleteCountsLine(ZERO)).toBe(
      "Nothing is recorded for this project but its folder under projects/.",
    );
  });
});

describe("keptLine", () => {
  it("is null when nothing is kept, and names what is otherwise", () => {
    expect(keptLine({ uploads: 0, artifacts: 0 })).toBeNull();
    expect(keptLine({ uploads: 1, artifacts: 2 })).toBe(
      "Keeps 1 upload and 2 by-products another conversation still uses, moved out of the project — a by-product as its newest version only.",
    );
  });

  it("names the by-product rule only when a by-product is kept", () => {
    // S5b M3: an upload is kept whole; the newest-version clause is about
    // by-products alone.
    expect(keptLine({ uploads: 1, artifacts: 0 })).toBe(
      "Keeps 1 upload another conversation still uses, moved out of the project.",
    );
    expect(keptLine({ uploads: 0, artifacts: 1 })).toBe(
      "Keeps 1 by-product another conversation still uses, moved out of the project — a by-product as its newest version only.",
    );
  });
});

describe("DeleteProjectDataDialog", () => {
  it("says what goes, and that the project's own folder is never touched", () => {
    renderDialog();
    const dialog = screen.getByRole("dialog");

    expect(dialog).toHaveTextContent(REPO.root);
    expect(dialog).toHaveTextContent(
      "its conversations, runs, by-products, uploads, the memory notes of every owner, and its folder under projects/ in the home store",
    );
    expect(dialog).toHaveTextContent(
      "The project’s own folder is never touched. This cannot be undone.",
    );
  });

  it("draws the plan as the daemon served it", () => {
    renderDialog();

    expect(screen.getByTestId("delete-plan")).toHaveTextContent(
      "Deletes 2 conversations · 41 messages · 3 runs · 4 by-products · 12 memory notes.",
    );
    expect(
      screen.getByText(/^Keeps 1 upload another conversation/),
    ).toBeInTheDocument();
    const entries = within(
      screen.getByRole("list", { name: "What the delete does" }),
    ).getAllByRole("listitem");
    expect(entries).toHaveLength(2);
    expect(entries[0]).toHaveTextContent("delete");
    expect(entries[0]).toHaveTextContent("projects/-Users-me-dev-repo/");
    expect(entries[1]).toHaveTextContent("keep");
    expect(entries[1]).toHaveTextContent("another conversation still shows");
  });

  it("keeps Delete data off until the plan has arrived", () => {
    renderDialog({ plan: undefined });

    expect(screen.getByTestId("delete-plan")).toHaveTextContent(
      "Reading what would be deleted…",
    );
    expect(screen.getByRole("button", { name: "Delete data" })).toBeDisabled();
  });

  it("keeps Delete data off when the plan could not be read, and says why", () => {
    renderDialog({
      plan: undefined,
      planError: "nothing of yours is recorded there",
    });

    expect(
      screen.getByText(
        "Could not read what would be deleted: nothing of yours is recorded there",
      ),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete data" })).toBeDisabled();
  });

  it("deletes only on the press", async () => {
    const user = userEvent.setup();
    const props = renderDialog();

    const press = screen.getByRole("button", { name: "Delete data" });
    expect(press).toBeEnabled();
    expect(props.onConfirm).not.toHaveBeenCalled();
    await user.click(press);
    expect(props.onConfirm).toHaveBeenCalledTimes(1);
  });

  it("disables Delete data while the delete is in flight", () => {
    renderDialog({ deleting: true });

    expect(screen.getByRole("button", { name: "Deleting…" })).toBeDisabled();
  });

  it("shows the daemon's refusal as written", () => {
    const sentence =
      "1 run(s) under /Users/me/dev/repo are queued, running or paused; delete its data once they finish";
    renderDialog({ refusal: sentence });

    expect(screen.getByText(sentence)).toBeInTheDocument();
  });

  it("focuses Cancel, not Delete data, and Escape cancels", () => {
    const props = renderDialog();

    expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus();
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(props.onCancel).toHaveBeenCalledTimes(1);
    expect(props.onConfirm).not.toHaveBeenCalled();
  });

  it("uses the existing danger ghost variant, not a new filled one (T25)", () => {
    renderDialog();
    const press = screen.getByRole("button", { name: "Delete data" });

    expect(press.className).toContain("text-red-ink");
    expect(press.className).toContain("bg-transparent");
  });
});
