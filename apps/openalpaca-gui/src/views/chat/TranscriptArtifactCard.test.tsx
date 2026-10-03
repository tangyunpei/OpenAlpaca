/**
 * The inline card an `artifact_written` frame produces, through the transcript
 * that orders it.
 *
 * Its `Open` goes through the real `apiFetch` stack and its `Reveal in Finder`
 * through the real `lib/reveal`; only the Tauri `invoke` and `fetch` are
 * doubled.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { resetConnection } from "@/lib/connection";
import { revealLabel } from "@/lib/reveal";
import { useUiStore } from "@/stores/ui";

import { Transcript } from "./Transcript";
import { buildTranscript, type WrittenArtifact } from "./transcript-model";
import { initialChatStreamState } from "@/lib/chat-stream";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const CONNECTION = {
  baseUrl: "http://127.0.0.1:9999",
  token: "test-token",
  instanceId: "7f3a1122",
};

const written: WrittenArtifact = {
  artifactId: "art-1",
  name: "connector-audit-findings.md",
  kind: "markdown",
  version: 2,
  path: "/home/.openalpaca/artifacts/2026-09-05-run/01-connector-audit-findings.md",
  taskId: "task-1",
  at: "2026-09-05T13:41:00Z",
};

function items(...artifacts: WrittenArtifact[]) {
  return buildTranscript({
    history: [],
    reports: [],
    artifacts,
    confirmations: [],
    resolutions: [],
    steers: [],
    stream: initialChatStreamState,
    pending: null,
  });
}

/** The shell's answer to `reveal_path`; every other command is discovery. */
let revealAnswer: () => Promise<unknown> = async () => null;
let posted: string[] = [];

beforeEach(() => {
  useUiStore.setState({ view: "chat", openArtifactId: null, toast: null });
  resetConnection();
  posted = [];
  revealAnswer = async () => null;
  vi.mocked(invoke).mockImplementation(async (command: string) =>
    command === "reveal_path" ? revealAnswer() : CONNECTION,
  );
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: unknown, init?: RequestInit) => {
      posted.push(`${init?.method ?? "GET"} ${String(input)}`);
      return new Response(JSON.stringify({ id: "art-1", status: "opened" }), {
        status: 200,
      });
    }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.mocked(invoke).mockReset();
});

function renderTranscript(...artifacts: WrittenArtifact[]) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return render(
    <QueryClientProvider client={client}>
      <Transcript items={items(...artifacts)} dense={false} />
    </QueryClientProvider>,
  );
}

describe("the written-artifact card", () => {
  it("names the file, its kind badge and the version the frame carried", () => {
    renderTranscript(written);

    expect(screen.getByText("connector-audit-findings.md")).toBeInTheDocument();
    expect(screen.getByText("v2")).toBeInTheDocument();
    expect(screen.getByText(/File written/)).toBeInTheDocument();
    // `markdown` is the daemon's spelling; the badge is the design's `MD`.
    expect(screen.getByText("MD")).toBeInTheDocument();
  });

  it("opens the file in the Library on the Preview tab", async () => {
    const user = userEvent.setup();
    renderTranscript(written);

    await user.click(screen.getByRole("button", { name: "Open in Library" }));

    const state = useUiStore.getState();
    expect(state.view).toBe("library");
    expect(state.openArtifactId).toBe("art-1");
    expect(state.libraryTab).toBe("preview");
  });

  it("shows a supersede as its own card rather than mutating the first", () => {
    renderTranscript(written, {
      ...written,
      version: 3,
      at: "2026-09-05T13:44:00Z",
    });
    expect(screen.getAllByText("connector-audit-findings.md")).toHaveLength(2);
    expect(screen.getByText("v2")).toBeInTheDocument();
    expect(screen.getByText("v3")).toBeInTheDocument();
  });

  it("offers Open always, and Reveal only inside the app's window", () => {
    const { unmount } = renderTranscript(written);
    expect(screen.getByRole("button", { name: "Open" })).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: revealLabel() }),
    ).not.toBeInTheDocument();
    unmount();

    vi.stubGlobal("isTauri", true);
    renderTranscript(written);
    expect(screen.getByRole("button", { name: "Open" })).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: revealLabel() }),
    ).toBeInTheDocument();
  });

  it("opens the file in its default app through the daemon", async () => {
    const user = userEvent.setup();
    renderTranscript(written);

    await user.click(screen.getByRole("button", { name: "Open" }));

    await waitFor(() =>
      expect(useUiStore.getState().toast).toBe(
        "connector-audit-findings.md opened in its default app",
      ),
    );
    expect(posted).toEqual(["POST http://127.0.0.1:9999/v1/files/art-1/open"]);
    // Open is not the Library: the view stays where it is.
    expect(useUiStore.getState().view).toBe("chat");
  });

  it("asks the shell to reveal the path the frame carried, and shows its refusal", async () => {
    vi.stubGlobal("isTauri", true);
    // Tauri rejects a command's `Err(String)` with the bare string.
    revealAnswer = () =>
      Promise.reject(
        "/home/.openalpaca/artifacts/2026-09-05-run/01-connector-audit-findings.md is not there any more, so it cannot be shown in its folder.",
      );
    const user = userEvent.setup();
    renderTranscript(written);

    await user.click(screen.getByRole("button", { name: revealLabel() }));

    expect(invoke).toHaveBeenCalledWith("reveal_path", { path: written.path });
    await waitFor(() =>
      expect(useUiStore.getState().toast).toBe(
        "/home/.openalpaca/artifacts/2026-09-05-run/01-connector-audit-findings.md is not there any more, so it cannot be shown in its folder.",
      ),
    );
    expect(posted).toEqual([]);
  });
});
