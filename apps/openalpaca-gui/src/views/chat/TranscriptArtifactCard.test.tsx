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
  inProject: false,
  openable: true,
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

describe("the written-artifact card for a file in the project", () => {
  const projectFile: WrittenArtifact = {
    artifactId: "art-7",
    name: "scripts/release.sh",
    kind: "code",
    version: 1,
    path: "/Users/dev/app/scripts/release.sh",
    taskId: "task-1",
    at: "2026-10-08T13:41:00Z",
    inProject: true,
    openable: false,
  };

  it("names the file in the project, with no version", () => {
    renderTranscript(projectFile);

    expect(screen.getByText("scripts/release.sh")).toBeInTheDocument();
    expect(
      screen.getByText(/File written · in the project/),
    ).toBeInTheDocument();
    expect(screen.queryByText("v1")).not.toBeInTheDocument();
  });

  it("offers Reveal and no Open for a kind the daemon does not open", () => {
    vi.stubGlobal("isTauri", true);
    renderTranscript(projectFile);

    expect(
      screen.queryByRole("button", { name: "Open" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: revealLabel() }),
    ).toBeInTheDocument();
  });

  it("offers Open for a project file of a kind the store keeps", () => {
    renderTranscript({
      ...projectFile,
      name: "docs/notes.md",
      kind: "markdown",
      openable: true,
    });
    expect(screen.getByRole("button", { name: "Open" })).toBeInTheDocument();
  });

  it("keys each write of a project file by its stamp, so two writes are two cards", () => {
    const keys = items(projectFile, {
      ...projectFile,
      at: "2026-10-08T13:44:00Z",
    })
      .filter((item) => item.kind === "artifact")
      .map((item) => item.key);
    expect(new Set(keys).size).toBe(2);
  });

  it("opens the Library on the project's results", async () => {
    useUiStore.setState({ libraryResults: false });
    const user = userEvent.setup();
    renderTranscript(projectFile);

    await user.click(screen.getByRole("button", { name: "Open in Library" }));

    expect(useUiStore.getState()).toMatchObject({
      view: "library",
      openArtifactId: "art-7",
      libraryResults: true,
    });
  });

  it("shows the hidden characters an agent put in the path, with their count", () => {
    // The path is the agent's: a bidi override and a zero-width space in it
    // would make the card name a different file.
    renderTranscript({
      ...projectFile,
      name: "scripts/\u{202E}release\u{200B}.sh",
    });

    const escapes = Array.from(
      document.querySelectorAll("[data-hidden-char]"),
      (element) => element.getAttribute("data-hidden-char"),
    );
    expect(escapes).toEqual(["U+202E", "U+200B"]);
    const shown = screen.getByText("release", { exact: false });
    expect(shown).toHaveTextContent("scripts/<U+202E>release<U+200B>.sh");
    // An ellipsis could cut off an escape the count says is there.
    expect(shown).toHaveClass("wrap-anywhere");
    expect(shown).not.toHaveClass("truncate");
    expect(
      screen.getByText(
        "Note: this request contains 2 hidden characters, shown as <U+…>.",
      ),
    ).toBeInTheDocument();
  });

  it("draws no hidden-character note for a plain path", () => {
    renderTranscript(projectFile);

    expect(document.querySelector("[data-hidden-char]")).toBeNull();
    expect(screen.queryByText(/hidden character/)).toBeNull();
    expect(screen.getByText("scripts/release.sh")).toHaveClass("truncate");
  });
});
