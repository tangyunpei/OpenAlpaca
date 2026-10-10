/**
 * `openRunFiles` — a run card's `+ n more in Library ↗` — opens the Library on
 * the by-products side: a run's outcome lists its by-products only (storage
 * design R38), so the side a reader left the Library on must not hide the file
 * it opens.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useUiStore } from "@/stores/ui";

import type { Run } from "./run-model";
import { useRunController } from "./useRunController";

// As every sibling hook test: nothing here is asked of the shell, and a
// discovery call, if one happened, gets an answer rather than a throw.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => ({
    baseUrl: "http://127.0.0.1:9999",
    token: "test-token",
    instanceId: "7f3a1122",
  })),
}));

const run: Run = {
  id: "task-1",
  title: "connector audit",
  status: "done",
  meta: "11m 04s",
  started: "10:00:00",
  stamp: "10:30",
  note: null,
  laneKey: "user:gui",
  artifactCount: 1,
  artifacts: [{ id: "art-1", name: "findings.md", kind: "md", stamp: "10:20" }],
  finishedAt: new Date("2026-09-05T10:30:00Z"),
  costUsd: 0.41,
  subagentCount: 2,
  steerable: false,
  startedElsewhere: false,
};

function renderController() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return renderHook(() => useRunController(), {
    wrapper: ({ children }: { children: React.ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    ),
  });
}

beforeEach(() => {
  useUiStore.setState({
    view: "work",
    libraryKind: "Docs",
    libraryResults: true,
    openArtifactId: null,
  });
});

describe("useRunController — a run's files", () => {
  it("opens the run's first file on the by-products side, every kind shown", () => {
    const { result } = renderController();

    act(() => result.current.openRunFiles(run));

    expect(useUiStore.getState()).toMatchObject({
      view: "library",
      openArtifactId: "art-1",
      libraryKind: "All",
      libraryResults: false,
    });
  });

  it("opens the by-products side when no file of the run has an id", () => {
    const { result } = renderController();

    act(() => result.current.openRunFiles({ ...run, artifacts: [] }));

    expect(useUiStore.getState()).toMatchObject({
      view: "library",
      openArtifactId: null,
      libraryResults: false,
    });
  });
});
