/**
 * The file panel's `Open` and `Reveal in Finder`, wired to the artifact the
 * slot read: the daemon's open route for its id, the shell's `reveal_path` for
 * its `path`. Only the Tauri `invoke` and `fetch` are doubled.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Artifact } from "@/lib/api/artifacts";
import { resetConnection } from "@/lib/connection";
import { revealLabel } from "@/lib/reveal";
import { useUiStore } from "@/stores/ui";

import { FilePanelSlot } from "./FilePanelSlot";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const row: Artifact = {
  id: "art-1",
  name: "01-pasta-night.md",
  kind: "markdown",
  mime_type: "text/markdown",
  size_bytes: 12,
  task_id: null,
  task_title: null,
  agent_id: null,
  agent_template_id: null,
  version: 1,
  version_count: 1,
  summary: null,
  metadata: null,
  created_at: "2026-10-02T10:00:00Z",
  updated_at: "2026-10-02T10:00:00Z",
  origin: "produced",
  pinned: false,
  missing: false,
  path: "/Users/dev/.openalpaca/artifacts/loose/2026-10-02/01-pasta-night.md",
  project_root: null,
  rel_path: "loose/2026-10-02/01-pasta-night.md",
  in_project: false,
  openable: true,
};

let requests: string[] = [];

beforeEach(() => {
  requests = [];
  resetConnection();
  useUiStore.setState({ toast: null, pins: {} });
  vi.mocked(invoke).mockImplementation(async (command: string) =>
    command === "reveal_path"
      ? null
      : {
          baseUrl: "http://127.0.0.1:9999",
          token: "test-token",
          instanceId: "7f3a1122",
        },
  );
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: unknown, init?: RequestInit) => {
      const url = String(input);
      const method = init?.method ?? "GET";
      requests.push(`${method} ${url}`);
      if (url.endsWith("/v1/files/art-1/open")) {
        return new Response(JSON.stringify({ id: "art-1", status: "opened" }));
      }
      if (url.endsWith("/v1/artifacts/art-1")) {
        return new Response(JSON.stringify(row));
      }
      return new Response(JSON.stringify({ error: "not found" }), {
        status: 404,
      });
    }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.mocked(invoke).mockReset();
});

function renderSlot() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  render(
    <QueryClientProvider client={client}>
      <FilePanelSlot artifactId="art-1" />
    </QueryClientProvider>,
  );
}

describe("FilePanelSlot — Open and Reveal", () => {
  it("opens the artifact through the daemon's open route", async () => {
    const user = userEvent.setup();
    renderSlot();

    await user.click(await screen.findByRole("button", { name: "Open" }));

    await waitFor(() =>
      expect(useUiStore.getState().toast).toBe(
        "01-pasta-night.md opened in its default app",
      ),
    );
    expect(requests).toContain(
      "POST http://127.0.0.1:9999/v1/files/art-1/open",
    );
  });

  it("reveals the path the artifact row carries", async () => {
    vi.stubGlobal("isTauri", true);
    const user = userEvent.setup();
    renderSlot();

    await user.click(
      await screen.findByRole("button", { name: revealLabel() }),
    );

    expect(invoke).toHaveBeenCalledWith("reveal_path", { path: row.path });
    // Finder coming forward is the answer: once the shell has replied and
    // every reaction to it has run (a macrotask runs after all of them),
    // nothing was toasted.
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(useUiStore.getState().toast).toBeNull();
  });
});
