/**
 * A project's life-cycle writes (storage design §6) and what each one makes
 * stale. A re-point re-addresses a project's conversations, runs, files and
 * notes under its new root, and a delete removes them, so the surfaces that
 * read them must re-ask; a dry run changes nothing and must not make them.
 *
 * The hooks run for real against a mocked client and a client holding one
 * answered entry per domain, and the test reads which entries the write
 * invalidated — the shape of `useSettings.test.tsx`.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import {
  useChooseProject,
  useDeleteProjectData,
  useProjects,
  useRepointProject,
} from "@/hooks/useWorkspaces";
import type { Project } from "@/lib/api/workspaces";
import { qk } from "@/lib/query-keys";

const PROJECT = vi.hoisted<Project>(() => ({
  id: "0b5c1d2e-3f40-4a5b-8c6d-7e8f9a0b1c2d",
  root: "/Users/me/repo",
  name: "-Users-me-repo",
  missing: false,
}));

vi.mock("@/lib/api/workspaces", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api/workspaces")>()),
  listProjects: vi.fn(async () => [PROJECT]),
  chooseProject: vi.fn(async () => PROJECT),
  repointProject: vi.fn(async (oldPath: string, newPath: string) => ({
    old_path: oldPath,
    new_path: newPath,
    moved: { artifacts: 0, sessions: 1, tasks: 0, messages: 2, followups: 0 },
    project: { ...PROJECT, root: newPath },
  })),
  deleteProjectData: vi.fn(async (_path: string, dryRun: boolean) => ({
    dry_run: dryRun,
    applied: !dryRun,
    project: PROJECT,
    entries: [],
    counts: {
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
    },
    kept: { uploads: 0, artifacts: 0 },
    removed: dryRun
      ? null
      : { session_dirs: 0, memory_partitions: 0, project_folder: true },
  })),
}));

/** Every domain a life-cycle write may touch, plus one it must not. */
const PROJECTS = qk.workspaces.list();
const STATUS = qk.status("/Users/me/repo");
const CONTENT = [
  qk.artifacts.all(),
  qk.sessions.all(),
  qk.tasks.all(),
  qk.memory.all(),
] as const;
const UNTOUCHED = qk.followups.all();

/**
 * `staleTime: Infinity` so a cached entry is fresh, as the app's 30 s default
 * makes it: only `refetchOnMount: "always"` can then re-read the list.
 */
function seeded(): QueryClient {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false, gcTime: Infinity, staleTime: Infinity },
      mutations: { retry: false },
    },
  });
  for (const key of [PROJECTS, STATUS, ...CONTENT, UNTOUCHED]) {
    client.setQueryData(key, []);
  }
  return client;
}

function wrapper(client: QueryClient) {
  return function Wrapper({ children }: { children: React.ReactNode }) {
    return (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
  };
}

function stale(client: QueryClient, key: readonly unknown[]): boolean {
  return client.getQueryState(key)?.isInvalidated === true;
}

describe("useProjects", () => {
  it("re-reads the list on mount even when one is cached, since no event names a new project", async () => {
    const client = seeded();
    const { result } = renderHook(() => useProjects(), {
      wrapper: wrapper(client),
    });

    await waitFor(() => expect(result.current.data).toEqual([PROJECT]));
  });
});

describe("what each life-cycle write makes stale", () => {
  it("Use refreshes the project list and every window's status, and no content", async () => {
    const client = seeded();
    const { result } = renderHook(() => useChooseProject(), {
      wrapper: wrapper(client),
    });

    result.current.mutate("/Users/me/repo/src");

    await waitFor(() => expect(stale(client, PROJECTS)).toBe(true));
    expect(stale(client, STATUS)).toBe(true);
    for (const key of CONTENT) expect(stale(client, key)).toBe(false);
    expect(stale(client, UNTOUCHED)).toBe(false);
  });

  it("a re-point refreshes everything the project's root addresses", async () => {
    const client = seeded();
    const { result } = renderHook(() => useRepointProject(), {
      wrapper: wrapper(client),
    });

    result.current.mutate({
      from: "/Users/me/repo",
      to: "/Users/me/work/repo",
    });

    await waitFor(() => expect(stale(client, PROJECTS)).toBe(true));
    expect(stale(client, STATUS)).toBe(true);
    for (const key of CONTENT) expect(stale(client, key)).toBe(true);
    expect(stale(client, UNTOUCHED)).toBe(false);
  });

  it("a dry run deletes nothing, so it refreshes nothing", async () => {
    const client = seeded();
    const { result } = renderHook(() => useDeleteProjectData(), {
      wrapper: wrapper(client),
    });

    result.current.mutate({ path: "/Users/me/repo", dryRun: true });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    for (const key of [PROJECTS, STATUS, ...CONTENT, UNTOUCHED]) {
      expect(stale(client, key)).toBe(false);
    }
  });

  it("a delete refreshes everything that named the project", async () => {
    const client = seeded();
    const { result } = renderHook(() => useDeleteProjectData(), {
      wrapper: wrapper(client),
    });

    result.current.mutate({ path: "/Users/me/repo", dryRun: false });

    await waitFor(() => expect(stale(client, PROJECTS)).toBe(true));
    expect(stale(client, STATUS)).toBe(true);
    for (const key of CONTENT) expect(stale(client, key)).toBe(true);
    expect(stale(client, UNTOUCHED)).toBe(false);
  });
});
