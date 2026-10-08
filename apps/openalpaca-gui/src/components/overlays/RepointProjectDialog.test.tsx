import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { Project, WorkspaceCounts } from "@/lib/api/workspaces";
import { RELATIVE_PATH_COPY } from "@/stores/project";

import {
  RepointProjectDialog,
  repointTarget,
  rowsLine,
  type RepointProjectDialogProps,
} from "./RepointProjectDialog";

const REPO: Project = {
  id: "6f1c2a3b-4d5e-4f60-8a7b-9c0d1e2f3a4b",
  root: "/Users/me/dev/repo",
  name: "-Users-me-dev-repo",
  missing: false,
};

const ROWS: WorkspaceCounts = {
  artifacts: 4,
  sessions: 2,
  tasks: 3,
  messages: 40,
  followups: 0,
};

function renderDialog(overrides: Partial<RepointProjectDialogProps> = {}) {
  const props: RepointProjectDialogProps = {
    project: REPO,
    rows: ROWS,
    resolvedAs: { id: REPO.id, root: REPO.root },
    runsInFlight: 0,
    reading: false,
    readError: null,
    value: "",
    onChange: vi.fn(),
    onBrowse: vi.fn(),
    refusal: null,
    repointing: false,
    onCancel: vi.fn(),
    onConfirm: vi.fn(),
    ...overrides,
  };
  render(<RepointProjectDialog {...props} />);
  return props;
}

describe("rowsLine", () => {
  it("names the non-zero counts in the owner's words, with singulars", () => {
    expect(rowsLine(ROWS)).toBe(
      "2 conversations · 40 messages · 3 runs · 4 files",
    );
    expect(
      rowsLine({
        artifacts: 1,
        sessions: 1,
        tasks: 0,
        messages: 1,
        followups: 1,
      }),
    ).toBe("1 conversation · 1 message · 1 file · 1 queued follow-up");
  });

  it("says nothing is recorded yet when every count is zero", () => {
    expect(
      rowsLine({
        artifacts: 0,
        sessions: 0,
        tasks: 0,
        messages: 0,
        followups: 0,
      }),
    ).toBe("nothing yet");
  });
});

describe("repointTarget", () => {
  it("is the normalized folder, or null for a blank, relative or unchanged one", () => {
    expect(repointTarget("  ", REPO)).toBeNull();
    expect(repointTarget("work/repo", REPO)).toBeNull();
    expect(repointTarget("/Users/me/dev/repo/", REPO)).toBeNull();
    expect(repointTarget(" /Users/me/work/repo/ ", REPO)).toBe(
      "/Users/me/work/repo",
    );
  });
});

describe("RepointProjectDialog", () => {
  it("shows what is recorded at the root before anything is sent", () => {
    renderDialog();

    expect(screen.getByRole("dialog")).toHaveTextContent(REPO.root);
    expect(screen.getByTestId("repoint-counts")).toHaveTextContent(
      "Recorded there: 2 conversations · 40 messages · 3 runs · 4 files.",
    );
  });

  it("says when the project's folder is gone", () => {
    renderDialog({ project: { ...REPO, missing: true } });

    expect(screen.getByRole("dialog")).toHaveTextContent(
      `Its folder is gone from ${REPO.root}.`,
    );
  });

  it("keeps Re-point off until the counts have been read", () => {
    renderDialog({ reading: true, value: "/Users/me/work/repo" });

    expect(screen.getByTestId("repoint-counts")).toHaveTextContent(
      "Counting what is recorded there…",
    );
    expect(screen.getByRole("button", { name: "Re-point" })).toBeDisabled();
  });

  it("keeps Re-point off when the counts could not be read, and says why", () => {
    // S5b M2: with no count, the busy gate cannot show a run in flight
    // either, so the press waits for a read that answered.
    renderDialog({
      rows: undefined,
      resolvedAs: undefined,
      readError: "daemon unreachable",
      value: "/Users/me/work/repo",
    });

    expect(screen.getByTestId("repoint-counts")).toHaveTextContent(
      "Could not count what is recorded there: daemon unreachable",
    );
    expect(screen.getByRole("button", { name: "Re-point" })).toBeDisabled();
  });

  it("refuses a press while a run there is in flight, and says why", () => {
    renderDialog({ runsInFlight: 2, value: "/Users/me/work/repo" });

    expect(screen.getByRole("dialog")).toHaveTextContent(
      "2 runs there are still in flight — a re-point is refused while one is; retry once it finishes.",
    );
    expect(screen.getByRole("button", { name: "Re-point" })).toBeDisabled();
  });

  it("says a relative folder is never a project", () => {
    renderDialog({ value: "work/repo" });

    expect(screen.getByText(RELATIVE_PATH_COPY)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Re-point" })).toBeDisabled();
  });

  it("enables Re-point for another absolute folder and re-points only on the press", async () => {
    const user = userEvent.setup();
    const props = renderDialog({ value: "/Users/me/work/repo" });

    const press = screen.getByRole("button", { name: "Re-point" });
    expect(press).toBeEnabled();
    expect(props.onConfirm).not.toHaveBeenCalled();

    await user.click(press);
    expect(props.onConfirm).toHaveBeenCalledTimes(1);
  });

  it("disables Re-point while the re-point is in flight", () => {
    renderDialog({ value: "/Users/me/work/repo", repointing: true });

    expect(screen.getByRole("button", { name: "Re-pointing…" })).toBeDisabled();
  });

  it("shows the daemon's refusal as written", () => {
    const sentence =
      "/Users/me/work/repo is already a project of its own, or OpenAlpaca's records already name it. Re-pointing onto it would merge two projects: delete that project's data first, or re-point onto another path";
    renderDialog({ value: "/Users/me/work/repo", refusal: sentence });

    expect(screen.getByText(sentence)).toBeInTheDocument();
  });

  it("focuses the folder field, offers the picker, and Escape cancels", async () => {
    const user = userEvent.setup();
    const props = renderDialog();

    expect(screen.getByLabelText("New project folder")).toHaveFocus();
    await user.click(screen.getByRole("button", { name: "Choose…" }));
    expect(props.onBrowse).toHaveBeenCalledTimes(1);

    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(props.onCancel).toHaveBeenCalledTimes(1);
    expect(props.onConfirm).not.toHaveBeenCalled();
  });
});
