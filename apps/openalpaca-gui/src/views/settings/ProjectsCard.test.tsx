/**
 * Settings → Connection → Projects (storage design §6, §9; ST-7) — driven
 * through the real hooks against a mocked client, so what each press sends
 * and what the window keeps afterwards are what is checked.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiError } from "@/lib/http";
import type { Project, Workspace } from "@/lib/api/workspaces";
import { useProjectStore } from "@/stores/project";
import { useUiStore } from "@/stores/ui";

import {
  MISSING_PROJECT_COPY,
  PROJECTS_EMPTY_COPY,
  ProjectsCard,
} from "./ProjectsCard";

const api = vi.hoisted(() => ({
  list: vi.fn(),
  get: vi.fn(),
  repoint: vi.fn(),
  remove: vi.fn(),
}));

vi.mock("@/lib/api/workspaces", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api/workspaces")>()),
  listProjects: api.list,
  getWorkspace: api.get,
  repointProject: api.repoint,
  deleteProjectData: api.remove,
}));

const picker = vi.hoisted(() => ({ pick: vi.fn() }));

vi.mock("./AddExtensionForm", () => ({ pickDirectory: picker.pick }));

const REPO: Project = {
  id: "6f1c2a3b-4d5e-4f60-8a7b-9c0d1e2f3a4b",
  root: "/Users/me/dev/repo",
  name: "-Users-me-dev-repo",
  missing: false,
};

function recorded(overrides: Partial<Workspace> = {}): Workspace {
  return {
    path: REPO.root,
    root: REPO.root,
    id: REPO.id,
    name: REPO.name,
    missing: false,
    rows: { artifacts: 4, sessions: 2, tasks: 3, messages: 40, followups: 0 },
    active_tasks: 0,
    queued_tasks: 0,
    ...overrides,
  };
}

const MOVED: Project = {
  ...REPO,
  root: "/Users/me/work/repo",
  name: "-Users-me-work-repo",
};

/**
 * The purge's answer. `projectFolder` is `removed.project_folder` on a real
 * delete: `false` when not every memory partition could be dropped, so the
 * daemon kept `projects/<name>/` and the project stays registered.
 */
function plan(dryRun: boolean, projectFolder = true) {
  return {
    dry_run: dryRun,
    applied: !dryRun,
    project: REPO,
    entries: [
      {
        entry: "projects/-Users-me-dev-repo/",
        holds:
          "project.json — the project's id and path; OpenAlpaca forgets the project",
        retention: "store metadata",
        action: "delete" as const,
      },
    ],
    counts: {
      sessions: 2,
      messages: 41,
      tool_calls: 0,
      followups: 0,
      tasks: 3,
      spans: 0,
      run_events: 0,
      uploads: 0,
      artifacts: 4,
      memory_notes: 12,
    },
    kept: { uploads: 0, artifacts: 0 },
    removed: dryRun
      ? null
      : {
          session_dirs: 2,
          memory_partitions: projectFolder ? 1 : 0,
          project_folder: projectFolder,
        },
  };
}

/**
 * Another project the card's root now reaches (R1): the folder was renamed by
 * case, or moved with a link left behind, and a turn there registered the
 * new spelling as a project of its own.
 */
const OTHER: Project = {
  id: "c3d4e5f6-a7b8-4c9d-8e0f-1a2b3c4d5e6f",
  root: "/Users/me/dev/Repo",
  name: "-Users-me-dev-Repo-1a2b3c4d",
  missing: false,
};

const GONE: Project = {
  id: "0a9b8c7d-6e5f-4a3b-9c2d-1e0f9a8b7c6d",
  root: "/Users/me/old/notes",
  name: "-Users-me-old-notes",
  missing: true,
};

function renderCard(currentRoot: string | null = null): QueryClient {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <ProjectsCard currentRoot={currentRoot} />
    </QueryClientProvider>,
  );
  return client;
}

beforeEach(() => {
  api.list.mockReset();
  api.get.mockReset();
  api.repoint.mockReset();
  api.remove.mockReset();
  picker.pick.mockReset();
  useProjectStore.setState({ path: null });
  useUiStore.setState({ toast: null });
});

describe("ProjectsCard (storage design §6, ST-7)", () => {
  it("lists every project by its root, with its folder name under projects/", async () => {
    api.list.mockResolvedValue([REPO, GONE]);
    renderCard();

    expect(await screen.findByText(REPO.root)).toBeInTheDocument();
    expect(screen.getByText(REPO.name)).toBeInTheDocument();
    expect(screen.getByText(GONE.root)).toBeInTheDocument();
    expect(screen.getByText(GONE.name)).toBeInTheDocument();
  });

  it("marks a missing project, and only that one, and says its data is kept", async () => {
    api.list.mockResolvedValue([REPO, GONE]);
    renderCard();

    await screen.findByText(GONE.root);
    expect(screen.getAllByText("missing")).toHaveLength(1);
    expect(screen.getByText(MISSING_PROJECT_COPY)).toBeInTheDocument();
  });

  it("marks the project this window's turns go to", async () => {
    api.list.mockResolvedValue([REPO, GONE]);
    renderCard(REPO.root);

    await screen.findByText(REPO.root);
    expect(screen.getAllByText("this window")).toHaveLength(1);
  });

  it("says there are no projects yet rather than drawing an empty card", async () => {
    api.list.mockResolvedValue([]);
    renderCard();

    expect(await screen.findByText(PROJECTS_EMPTY_COPY)).toBeInTheDocument();
  });

  it("says a failed read failed", async () => {
    api.list.mockRejectedValue(new Error("daemon unreachable"));
    renderCard();

    expect(
      await screen.findByText("Could not load — daemon unreachable"),
    ).toBeInTheDocument();
  });
});

describe("Re-point… (storage design §6, ST-20)", () => {
  it("shows what is recorded at the root, re-points on the press, and the window follows", async () => {
    const user = userEvent.setup();
    useProjectStore.setState({ path: REPO.root });
    api.list.mockResolvedValue([REPO]);
    api.get.mockResolvedValue(recorded());
    api.repoint.mockResolvedValue({
      old_path: REPO.root,
      new_path: MOVED.root,
      moved: recorded().rows,
      project: MOVED,
    });
    renderCard(REPO.root);

    await user.click(await screen.findByRole("button", { name: "Re-point…" }));
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText(
      "Recorded there: 2 conversations · 40 messages · 3 runs · 4 files.",
    );
    expect(api.get).toHaveBeenCalledWith(REPO.root, expect.anything());
    expect(api.repoint).not.toHaveBeenCalled();

    await user.type(
      within(dialog).getByLabelText("New project folder"),
      MOVED.root,
    );
    await user.click(within(dialog).getByRole("button", { name: "Re-point" }));

    await waitFor(() =>
      expect(api.repoint).toHaveBeenCalledWith(REPO.root, MOVED.root),
    );
    await waitFor(() =>
      expect(useProjectStore.getState().path).toBe(MOVED.root),
    );
    // The store is written synchronously in `onSuccess`; React's re-render
    // that unmounts the dialog may land a tick later.
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(useUiStore.getState().toast).toBe(
      `Re-pointed to ${MOVED.root}: 2 conversations · 40 messages · 3 runs · 4 files.`,
    );
    // The list is re-read after the write.
    await waitFor(() => expect(api.list).toHaveBeenCalledTimes(2));
  });

  it("leaves a window on another project where it is", async () => {
    const user = userEvent.setup();
    useProjectStore.setState({ path: "/Users/me/other" });
    api.list.mockResolvedValue([REPO]);
    api.get.mockResolvedValue(recorded());
    api.repoint.mockResolvedValue({
      old_path: REPO.root,
      new_path: MOVED.root,
      moved: recorded().rows,
      project: MOVED,
    });
    renderCard("/Users/me/other");

    await user.click(await screen.findByRole("button", { name: "Re-point…" }));
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText(/^Recorded there:/);
    await user.type(
      within(dialog).getByLabelText("New project folder"),
      MOVED.root,
    );
    await user.click(within(dialog).getByRole("button", { name: "Re-point" }));

    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(useProjectStore.getState().path).toBe("/Users/me/other");
  });

  it("keeps the dialog open on the daemon's refusal and shows its sentence", async () => {
    const user = userEvent.setup();
    const sentence =
      "/Users/me/work/repo is already a project of its own, or OpenAlpaca's records already name it. Re-pointing onto it would merge two projects: delete that project's data first, or re-point onto another path";
    useProjectStore.setState({ path: REPO.root });
    api.list.mockResolvedValue([REPO]);
    api.get.mockResolvedValue(recorded());
    api.repoint.mockRejectedValue(
      new ApiError(sentence, 409, "WORKSPACE_EXISTS"),
    );
    renderCard(REPO.root);

    await user.click(await screen.findByRole("button", { name: "Re-point…" }));
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText(/^Recorded there:/);
    await user.type(
      within(dialog).getByLabelText("New project folder"),
      MOVED.root,
    );
    await user.click(within(dialog).getByRole("button", { name: "Re-point" }));

    expect(await within(dialog).findByText(sentence)).toBeInTheDocument();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(useProjectStore.getState().path).toBe(REPO.root);
  });

  it("follows the project and says so when Cancel was pressed while the re-point was out, closing no dialog opened since", async () => {
    // The press cannot be taken back: the daemon finishes the write whether
    // or not the dialog is still open, so the window must still follow.
    const user = userEvent.setup();
    let answer: (value: unknown) => void = () => {};
    useProjectStore.setState({ path: REPO.root });
    api.list.mockResolvedValue([REPO]);
    api.get.mockResolvedValue(recorded());
    api.repoint.mockReturnValue(
      new Promise((resolve) => {
        answer = resolve;
      }),
    );
    renderCard(REPO.root);

    await user.click(await screen.findByRole("button", { name: "Re-point…" }));
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText(/^Recorded there:/);
    await user.type(
      within(dialog).getByLabelText("New project folder"),
      MOVED.root,
    );
    await user.click(within(dialog).getByRole("button", { name: "Re-point" }));
    await within(dialog).findByRole("button", { name: "Re-pointing…" });

    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    // The owner opens the dialog again before the first answer lands.
    await user.click(screen.getByRole("button", { name: "Re-point…" }));
    await screen.findByRole("dialog");

    answer({
      old_path: REPO.root,
      new_path: MOVED.root,
      moved: recorded().rows,
      project: MOVED,
    });

    await waitFor(() =>
      expect(useProjectStore.getState().path).toBe(MOVED.root),
    );
    expect(useUiStore.getState().toast).toBe(
      `Re-pointed to ${MOVED.root}: 2 conversations · 40 messages · 3 runs · 4 files.`,
    );
    await waitFor(() => expect(api.list).toHaveBeenCalledTimes(2));
    // The late answer is not the new dialog's to close.
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("refuses to re-point when the daemon reads the root as another project, and says whose", async () => {
    const user = userEvent.setup();
    api.list.mockResolvedValue([REPO]);
    api.get.mockResolvedValue(
      recorded({ root: OTHER.root, id: OTHER.id, name: OTHER.name }),
    );
    renderCard();

    await user.click(await screen.findByRole("button", { name: "Re-point…" }));
    const dialog = await screen.findByRole("dialog");
    await waitFor(() =>
      expect(
        within(dialog).queryByText("Counting what is recorded there…"),
      ).toBeNull(),
    );
    await user.type(
      within(dialog).getByLabelText("New project folder"),
      MOVED.root,
    );
    expect(
      within(dialog).getByRole("button", { name: "Re-point" }),
    ).toBeDisabled();
    expect(
      within(dialog).getByText(
        `${REPO.root} now reaches another project, ${OTHER.root}: a re-point would move that one, not this one. Nothing was changed — close this and look at the list again.`,
      ),
    ).toBeInTheDocument();
    expect(within(dialog).queryByText(/^Recorded there:/)).toBeNull();
    expect(api.repoint).not.toHaveBeenCalled();
  });

  it("refuses to re-point when the root no longer reaches any project", async () => {
    const user = userEvent.setup();
    api.list.mockResolvedValue([REPO]);
    api.get.mockResolvedValue(
      recorded({
        root: null,
        id: null,
        name: null,
        rows: {
          artifacts: 0,
          sessions: 0,
          tasks: 0,
          messages: 0,
          followups: 0,
        },
      }),
    );
    renderCard();

    await user.click(await screen.findByRole("button", { name: "Re-point…" }));
    const dialog = await screen.findByRole("dialog");
    await waitFor(() =>
      expect(
        within(dialog).queryByText("Counting what is recorded there…"),
      ).toBeNull(),
    );
    await user.type(
      within(dialog).getByLabelText("New project folder"),
      MOVED.root,
    );
    expect(
      within(dialog).getByRole("button", { name: "Re-point" }),
    ).toBeDisabled();
    expect(
      within(dialog).getByText(
        `${REPO.root} no longer reaches this project: a re-point would not move it. Nothing was changed — close this and look at the list again.`,
      ),
    ).toBeInTheDocument();
  });

  it("fills the folder from the native picker", async () => {
    const user = userEvent.setup();
    picker.pick.mockResolvedValue(MOVED.root);
    api.list.mockResolvedValue([REPO]);
    api.get.mockResolvedValue(recorded());
    renderCard();

    await user.click(await screen.findByRole("button", { name: "Re-point…" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Choose…" }));

    await waitFor(() =>
      expect(within(dialog).getByLabelText("New project folder")).toHaveValue(
        MOVED.root,
      ),
    );
  });
});

describe("Delete data… (storage design §6, ST-19)", () => {
  it("shows the plan from one dry run, deletes on the press, and the window lets go of the project", async () => {
    const user = userEvent.setup();
    useProjectStore.setState({ path: REPO.root });
    api.list.mockResolvedValue([REPO]);
    api.remove.mockImplementation(async (_path: string, dryRun: boolean) =>
      plan(dryRun),
    );
    renderCard(REPO.root);

    await user.click(
      await screen.findByRole("button", { name: "Delete data…" }),
    );
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText(
      "Deletes 2 conversations · 41 messages · 3 runs · 4 by-products · 12 memory notes.",
    );
    expect(api.remove).toHaveBeenCalledTimes(1);
    expect(api.remove).toHaveBeenLastCalledWith(REPO.root, true);

    await user.click(
      within(dialog).getByRole("button", { name: "Delete data" }),
    );

    await waitFor(() => expect(api.remove).toHaveBeenCalledTimes(2));
    expect(api.remove).toHaveBeenLastCalledWith(REPO.root, false);
    await waitFor(() => expect(useProjectStore.getState().path).toBeNull());
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(useUiStore.getState().toast).toBe(
      `Deleted the data of ${REPO.root}. This window has no project now.`,
    );
    await waitFor(() => expect(api.list).toHaveBeenCalledTimes(2));
  });

  it("leaves a window on another project where it is", async () => {
    const user = userEvent.setup();
    useProjectStore.setState({ path: "/Users/me/other" });
    api.list.mockResolvedValue([REPO]);
    api.remove.mockImplementation(async (_path: string, dryRun: boolean) =>
      plan(dryRun),
    );
    renderCard("/Users/me/other");

    await user.click(
      await screen.findByRole("button", { name: "Delete data…" }),
    );
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText(/^Deletes 2 conversations/);
    await user.click(
      within(dialog).getByRole("button", { name: "Delete data" }),
    );

    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(useProjectStore.getState().path).toBe("/Users/me/other");
    expect(useUiStore.getState().toast).toBe(
      `Deleted the data of ${REPO.root}.`,
    );
  });

  it("lets go of the project and says so when Escape closed the dialog while the delete was out", async () => {
    // Up to 3 s of quiesce plus the delete: an Escape then looks like a
    // cancel, but the daemon finishes, and a window still sending the root
    // would register the deleted project again on its next turn.
    const user = userEvent.setup();
    let answer: (value: unknown) => void = () => {};
    useProjectStore.setState({ path: REPO.root });
    api.list.mockResolvedValue([REPO]);
    api.remove.mockImplementation((_path: string, dryRun: boolean) =>
      dryRun
        ? Promise.resolve(plan(true))
        : new Promise((resolve) => {
            answer = resolve;
          }),
    );
    renderCard(REPO.root);

    await user.click(
      await screen.findByRole("button", { name: "Delete data…" }),
    );
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText(/^Deletes 2 conversations/);
    await user.click(
      within(dialog).getByRole("button", { name: "Delete data" }),
    );
    await within(dialog).findByRole("button", { name: "Deleting…" });

    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();

    answer(plan(false));

    await waitFor(() => expect(useProjectStore.getState().path).toBeNull());
    expect(useUiStore.getState().toast).toBe(
      `Deleted the data of ${REPO.root}. This window has no project now.`,
    );
    await waitFor(() => expect(api.list).toHaveBeenCalledTimes(2));
  });

  it("says the folder was kept when the daemon kept it, and the window keeps the project it still is", async () => {
    // S5a's fix round: a partition that could not be dropped keeps
    // projects/<name>/, so the project stays registered and listed, and a
    // second delete finishes it. Nothing here may say the project is gone.
    const user = userEvent.setup();
    useProjectStore.setState({ path: REPO.root });
    api.list.mockResolvedValue([REPO]);
    api.remove.mockImplementation(async (_path: string, dryRun: boolean) =>
      plan(dryRun, false),
    );
    renderCard(REPO.root);

    await user.click(
      await screen.findByRole("button", { name: "Delete data…" }),
    );
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText(/^Deletes 2 conversations/);
    await user.click(
      within(dialog).getByRole("button", { name: "Delete data" }),
    );

    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(api.remove).toHaveBeenLastCalledWith(REPO.root, false);
    expect(useProjectStore.getState().path).toBe(REPO.root);
    expect(useUiStore.getState().toast).toBe(
      `Deleted the data of ${REPO.root}, but not all of it could be removed: its folder under projects/ was kept, so the project is still listed. Delete its data again to finish.`,
    );
    // The list is re-read, and the daemon still lists the project.
    await waitFor(() => expect(api.list).toHaveBeenCalledTimes(2));
    expect(await screen.findByText(REPO.root)).toBeInTheDocument();
  });

  it("refuses to delete when the plan the daemon read is another project's, and says whose", async () => {
    // R1: the plan names the project the daemon resolved the root to. When
    // that is not the card's project, drawing its counts under this card's
    // root and enabling the press would delete the wrong project's data.
    const user = userEvent.setup();
    api.list.mockResolvedValue([REPO]);
    api.remove.mockImplementation(async (_path: string, dryRun: boolean) => ({
      ...plan(dryRun),
      project: OTHER,
    }));
    renderCard();

    await user.click(
      await screen.findByRole("button", { name: "Delete data…" }),
    );
    const dialog = await screen.findByRole("dialog");
    await waitFor(() =>
      expect(
        within(dialog).queryByText("Reading what would be deleted…"),
      ).toBeNull(),
    );
    expect(
      within(dialog).getByRole("button", { name: "Delete data" }),
    ).toBeDisabled();
    expect(
      within(dialog).getByText(
        `${REPO.root} now reaches another project, ${OTHER.root} (projects/${OTHER.name}): this would delete that one's data, not this one's. Nothing was deleted — close this and look at the list again.`,
      ),
    ).toBeInTheDocument();
    expect(within(dialog).queryByText(/^Deletes /)).toBeNull();
    expect(api.remove).toHaveBeenCalledTimes(1);
    expect(api.remove).toHaveBeenLastCalledWith(REPO.root, true);
  });

  it("keeps the dialog open on WORKSPACE_BUSY and shows the daemon's sentence", async () => {
    const user = userEvent.setup();
    // S5a refuses the dry run with this too, when a run is already in flight;
    // here the run started after the plan was read, and the delete's own
    // transaction refused it.
    const sentence =
      "1 run(s) under /Users/me/dev/repo are queued, running or paused; delete its data once they finish";
    useProjectStore.setState({ path: REPO.root });
    api.list.mockResolvedValue([REPO]);
    api.remove.mockImplementation(async (_path: string, dryRun: boolean) => {
      if (dryRun) return plan(true);
      throw new ApiError(sentence, 409, "WORKSPACE_BUSY");
    });
    renderCard(REPO.root);

    await user.click(
      await screen.findByRole("button", { name: "Delete data…" }),
    );
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText(/^Deletes 2 conversations/);
    await user.click(
      within(dialog).getByRole("button", { name: "Delete data" }),
    );

    expect(await within(dialog).findByText(sentence)).toBeInTheDocument();
    expect(useProjectStore.getState().path).toBe(REPO.root);
  });
});
