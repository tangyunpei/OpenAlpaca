/**
 * Settings → Connection → Projects (storage design §6, §9; ST-7) — driven
 * through the real hooks against a mocked client, so what each press sends
 * and what the window keeps afterwards are what is checked.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
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
