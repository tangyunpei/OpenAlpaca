/**
 * Settings → Connection → Projects (storage design §6, §9; ST-7) — driven
 * through the real hooks against a mocked client, so what each press sends
 * and what the window keeps afterwards are what is checked.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Project } from "@/lib/api/workspaces";
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

const REPO: Project = {
  id: "6f1c2a3b-4d5e-4f60-8a7b-9c0d1e2f3a4b",
  root: "/Users/me/dev/repo",
  name: "-Users-me-dev-repo",
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
