/**
 * The file panel's `Open` and `Reveal in Finder`, wired to the artifact the
 * slot read: the daemon's open route for its id, the shell's `reveal_path` for
 * its `path`. Only the Tauri `invoke` and `fetch` are doubled.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
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

/** A file an agent wrote into its project (storage design §13): a pointer row. */
const projectScript: Artifact = {
  ...row,
  name: "scripts/release.sh",
  kind: "code",
  mime_type: "text/plain",
  path: "/Users/dev/app/scripts/release.sh",
  project_root: "/Users/dev/app",
  rel_path: "scripts/release.sh",
  version_count: 0,
  in_project: true,
  openable: false,
};

/** The row `GET /v1/artifacts/art-1` answers with; `row` unless a test sets it. */
let served: Artifact = row;

/** What `GET /v1/artifacts/art-1/content` answers; a 404 unless a test sets it. */
let content: () => Response = notFound;

let requests: string[] = [];

function notFound(): Response {
  return new Response(JSON.stringify({ error: "not found" }), {
    status: 404,
  });
}

beforeEach(() => {
  served = row;
  content = notFound;
  requests = [];
  resetConnection();
  useUiStore.setState({
    toast: null,
    pins: {},
    panelTab: "preview",
    libraryResults: false,
  });
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
        return new Response(JSON.stringify(served));
      }
      if (url.endsWith("/v1/artifacts/art-1/content")) return content();
      // The panel's picker lists the library.
      if (url.includes("/v1/artifacts?")) {
        return new Response(JSON.stringify({ artifacts: [served], total: 1 }));
      }
      return notFound();
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

describe("FilePanelSlot — Library ↗ for a file written into the project", () => {
  it("opens the Library on the results side, on Preview", async () => {
    served = projectScript;
    useUiStore.setState({
      view: "chat",
      panelArtifactId: "art-1",
      panelTab: "history",
    });
    const user = userEvent.setup();
    renderSlot();
    await screen.findByRole("button", { name: /scripts\/release\.sh/ });

    await user.click(screen.getByRole("button", { name: "Library ↗" }));

    expect(useUiStore.getState()).toMatchObject({
      view: "library",
      openArtifactId: "art-1",
      libraryResults: true,
      libraryTab: "preview",
    });
  });
});

describe("FilePanelSlot — a file written into the project", () => {
  it("draws Reveal and no Open for a kind the daemon does not open", async () => {
    served = projectScript;
    vi.stubGlobal("isTauri", true);
    renderSlot();

    expect(
      await screen.findByRole("button", { name: revealLabel() }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Open" })).toBeNull();
  });

  it("shows Preview alone and says the file is in the project", async () => {
    served = projectScript;
    useUiStore.setState({ panelTab: "history" });
    renderSlot();

    expect(await screen.findByText("in the project")).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Preview" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.queryByRole("tab", { name: "Diff" })).toBeNull();
    expect(screen.queryByRole("tab", { name: "History" })).toBeNull();
  });

  it("shows the hidden characters an agent put in the path, with their count", async () => {
    // The path is the agent's: a bidi override and a zero-width space in it
    // would make the panel name a different file.
    const name = "scripts/\u{202E}release\u{200B}.sh";
    served = { ...projectScript, name, rel_path: name };
    renderSlot();

    const header = await screen.findByRole("button", { name: /release/ });
    const escapes = Array.from(
      header.querySelectorAll("[data-hidden-char]"),
      (element) => element.getAttribute("data-hidden-char"),
    );
    expect(escapes).toEqual(["U+202E", "U+200B"]);
    const shown = within(header).getByText("release", { exact: false });
    expect(shown).toHaveTextContent("scripts/<U+202E>release<U+200B>.sh");
    // An ellipsis could cut off an escape the count says is there.
    expect(shown).toHaveClass("wrap-anywhere");
    expect(shown).not.toHaveClass("truncate");
    expect(
      within(header).getByText(
        "Note: this request contains 2 hidden characters, shown as <U+…>.",
      ),
    ).toBeInTheDocument();
  });

  it("draws no hidden-character note for a plain path", async () => {
    served = projectScript;
    renderSlot();

    const header = await screen.findByRole("button", {
      name: /scripts\/release\.sh/,
    });
    expect(header.querySelector("[data-hidden-char]")).toBeNull();
    expect(screen.queryByText(/hidden character/)).toBeNull();
    expect(within(header).getByText("scripts/release.sh")).toHaveClass(
      "truncate",
    );
  });
});

describe("FilePanelSlot — the Preview of a file written into the project (X15, X14)", () => {
  function text(body: string): Response {
    return new Response(body, {
      status: 200,
      headers: { "content-type": "text/plain" },
    });
  }

  it("draws a 410's hidden characters escaped, with their count", async () => {
    // The path is the agent's, and the daemon's refusal repeats it.
    const name = "scripts/\u{202E}release\u{200B}.sh";
    served = { ...projectScript, name, rel_path: name, missing: true };
    content = () =>
      new Response(
        JSON.stringify({
          error: {
            code: "ARTIFACT_GONE",
            message: `artifact art-1 is gone: /Users/dev/app/${name} no longer exists`,
          },
        }),
        { status: 410 },
      );
    renderSlot();

    expect(
      await screen.findByText(
        "This file is no longer at scripts/<U+202E>release<U+200B>.sh in the project. Its row is kept, and the file is shown again once it is back. Note: this request contains 2 hidden characters, shown as <U+…>.",
      ),
    ).toBeInTheDocument();
    expect(document.body.textContent).not.toMatch(/[\u{202E}\u{200B}]/u);
  });

  it("reads the row and the library again once a marked file is read", async () => {
    let back = false;
    served = { ...projectScript, missing: true };
    content = () => {
      back = true;
      served = { ...projectScript, missing: false };
      return text("echo release\n");
    };
    renderSlot();

    await waitFor(() => expect(back).toBe(true));
    // The read cleared the mark on the daemon (R37): the row and the
    // panel's list of the library are read again.
    await waitFor(() =>
      expect(
        requests.filter((r) => r.endsWith("/v1/artifacts/art-1")).length,
      ).toBeGreaterThan(1),
    );
    await waitFor(() =>
      expect(
        requests.filter((r) => r.includes("/v1/artifacts?")).length,
      ).toBeGreaterThan(1),
    );
  });

  it("says a marked image could not be read when it does not load (X14)", async () => {
    served = {
      ...projectScript,
      name: "docs/diagram.png",
      rel_path: "docs/diagram.png",
      path: "/Users/dev/app/docs/diagram.png",
      kind: "image",
      mime_type: "image/png",
      openable: true,
      missing: true,
    };
    renderSlot();

    fireEvent.error(await screen.findByAltText("docs/diagram.png"));

    expect(
      await screen.findByText(
        "This file could not be read the last time it was asked for: it was not at its path in the project, or the project's folder is gone or has moved. Its row is kept, and the image is loaded again the next time this preview is opened.",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/Export or Open/)).not.toBeInTheDocument();
  });
});
