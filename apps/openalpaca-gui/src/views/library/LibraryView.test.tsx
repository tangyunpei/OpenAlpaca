/**
 * The Library end to end, over the real data layer.
 *
 * Only two edges are doubled — the Tauri discovery command and `fetch` — so the
 * query keys, the request URLs and the `?token=` preview URL under test are the
 * production ones. Nothing here is a fixture standing in for the daemon: every
 * row rendered came out of a response body.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Artifact } from "@/lib/api/artifacts";
import { resetConnection } from "@/lib/connection";
import { revealLabel } from "@/lib/reveal";
import { useUiStore } from "@/stores/ui";

import { RESULTS_EMPTY } from "./LibraryList";
import LibraryView from "./LibraryView";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => ({
    baseUrl: "http://127.0.0.1:9999",
    token: "test-token",
    instanceId: "7f3a1122",
  })),
}));

const findings: Artifact = {
  id: "art-1",
  name: "connector-audit-findings.md",
  kind: "markdown",
  mime_type: "text/markdown",
  size_bytes: 42,
  task_id: "task-1",
  task_title: "connector audit",
  agent_id: "agent-1",
  agent_template_id: "review_agent",
  version: 2,
  version_count: 2,
  summary: null,
  metadata: null,
  created_at: "2026-09-05T10:00:00Z",
  updated_at: "2026-09-05T10:05:00Z",
  origin: "produced",
  pinned: false,
  missing: false,
  path: "/home/.openalpaca/artifacts/2026-09-05-run/01-connector-audit-findings.md",
  project_root: null,
  rel_path: "2026-09-05-run/01-connector-audit-findings.md",
  in_project: false,
  openable: true,
};

const chart: Artifact = {
  ...findings,
  id: "art-2",
  name: "spend.png",
  kind: "image",
  mime_type: "image/png",
  version: 1,
  version_count: 1,
};

interface Recorded {
  url: string;
  method: string;
  body: unknown;
}

let requests: Recorded[] = [];
/** URL → response. Each test overrides the pieces it cares about. */
let routes: { match: string; reply: () => Response }[] = [];

function json(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), { status });
}

function text(body: string): Response {
  return new Response(body, {
    status: 200,
    headers: { "content-type": "text/markdown" },
  });
}

function route(match: string, reply: () => Response): void {
  routes.unshift({ match, reply });
}

function listing(artifacts: Artifact[], total = artifacts.length): void {
  route("/v1/artifacts?", () => json({ artifacts, total }));
  route("/v1/artifacts?q", () => json({ artifacts, total }));
}

beforeEach(() => {
  requests = [];
  routes = [];
  resetConnection();
  useUiStore.setState({
    libraryKind: "All",
    libraryResults: false,
    openArtifactId: null,
    libraryTab: "preview",
    pins: {},
    toast: null,
  });

  // Defaults every test starts from; `route` prepends more specific ones.
  route("/v1/artifacts", () => json({ artifacts: [], total: 0 }));

  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: unknown, init?: RequestInit) => {
      const url = String(input);
      requests.push({
        url,
        method: init?.method ?? "GET",
        body: typeof init?.body === "string" ? JSON.parse(init.body) : null,
      });
      // Longest pattern wins, so `…/art-1/versions` cannot be swallowed by
      // the `…/art-1` route registered for the row itself.
      const hit = routes
        .filter((entry) => url.includes(entry.match))
        .sort((a, b) => b.match.length - a.match.length)[0];
      return hit === undefined
        ? json({ error: "not found" }, 404)
        : hit.reply();
    }),
  );
});

function renderView() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return render(
    <QueryClientProvider client={client}>
      <LibraryView />
    </QueryClientProvider>,
  );
}

/** The requests the view actually issued, for asserting a query string. */
function urlsFor(fragment: string): string[] {
  return requests.map((r) => r.url).filter((url) => url.includes(fragment));
}

describe("LibraryList — what the daemon serves", () => {
  it("renders the real chrome and the served count", async () => {
    listing([findings, chart], 2);
    renderView();

    expect(
      screen.getByRole("heading", { name: "Library" }),
    ).toBeInTheDocument();
    expect(await screen.findByText("2 files")).toBeInTheDocument();
    expect(
      await screen.findByText("connector-audit-findings.md"),
    ).toBeInTheDocument();
  });

  it("says the library is empty in the design's own voice", async () => {
    renderView();
    expect(
      await screen.findByText(
        "Nothing in the library yet. Files the agents produce land here.",
      ),
    ).toBeInTheDocument();
  });

  it("names the failure rather than showing an empty library", async () => {
    route("/v1/artifacts", () =>
      json({ error: { code: "DB_ERROR", message: "database is locked" } }, 500),
    );
    renderView();

    expect(await screen.findByText(/could not be loaded/i)).toBeInTheDocument();
    expect(screen.getByText(/database is locked/)).toBeInTheDocument();
  });

  it("filters the loaded page by kind without inventing a count", async () => {
    listing([findings, chart], 2);
    const user = userEvent.setup();
    renderView();
    await screen.findByText("connector-audit-findings.md");

    await user.click(screen.getByRole("button", { name: "Media" }));
    expect(screen.getByText("spend.png")).toBeInTheDocument();
    expect(
      screen.queryByText("connector-audit-findings.md"),
    ).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Plans" }));
    expect(screen.getByText(/No plans files/i)).toBeInTheDocument();
  });

  it("pages the rest of the library on request", async () => {
    listing([findings], 2);
    const user = userEvent.setup();
    renderView();
    await screen.findByText("connector-audit-findings.md");

    route("limit=200&offset=1", () => json({ artifacts: [chart], total: 2 }));
    await user.click(screen.getByRole("button", { name: /Load more/ }));

    await waitFor(() =>
      expect(screen.getByText("spend.png")).toBeInTheDocument(),
    );
    expect(urlsFor("offset=1")).toHaveLength(1);
  });
});

describe("LibraryList — by-products and results (storage design §13)", () => {
  const written: Artifact = {
    ...findings,
    id: "art-9",
    name: "src/main.rs",
    kind: "code",
    mime_type: "text/plain",
    task_id: null,
    task_title: null,
    path: "/Users/dev/app/src/main.rs",
    project_root: "/Users/dev/app",
    rel_path: "src/main.rs",
    version: 1,
    version_count: 0,
    in_project: true,
    openable: true,
  };

  it("lists by-products by default, asking the daemon for that side only", async () => {
    listing([findings], 1);
    renderView();

    expect(
      await screen.findByText("connector-audit-findings.md"),
    ).toBeInTheDocument();
    expect(urlsFor("in_project=false")).not.toHaveLength(0);
    expect(urlsFor("in_project=true")).toHaveLength(0);
  });

  it("lists the files agents wrote into a project behind its chip", async () => {
    listing([findings], 1);
    route("in_project=true", () => json({ artifacts: [written], total: 1 }));
    const user = userEvent.setup();
    renderView();
    await screen.findByText("connector-audit-findings.md");

    await user.click(
      screen.getByRole("button", { name: "Results in the project" }),
    );

    expect(await screen.findByText("src/main.rs")).toBeInTheDocument();
    expect(
      screen.queryByText("connector-audit-findings.md"),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Results in the project" }),
    ).toHaveAttribute("aria-pressed", "true");
    expect(useUiStore.getState().libraryResults).toBe(true);
  });

  it("says no file was written into a project yet when that side is empty", async () => {
    route("in_project=true", () => json({ artifacts: [], total: 0 }));
    useUiStore.setState({ libraryResults: true });
    renderView();

    expect(await screen.findByText(RESULTS_EMPTY)).toBeInTheDocument();
  });
});

describe("LibraryDetail", () => {
  beforeEach(() => {
    listing([findings, chart], 2);
    route("/v1/artifacts/art-1/versions", () =>
      json({
        versions: [
          {
            version: 2,
            note: "tightened the summary",
            author_agent_id: "review_agent",
            created_at: "2026-09-05T10:05:00Z",
            size_bytes: 42,
            added_lines: 3,
            removed_lines: 1,
          },
          {
            version: 1,
            note: "",
            author_agent_id: null,
            created_at: "2026-09-05T10:00:00Z",
            size_bytes: 30,
            added_lines: null,
            removed_lines: null,
          },
        ],
      }),
    );
    route("/v1/artifacts/art-1/content", () =>
      text("# Findings\n\nAll good.\n"),
    );
    route("/v1/artifacts/art-1", () => json(findings));
    route("/v1/artifacts/art-2", () => json(chart));
  });

  it("invites a selection when nothing is open", () => {
    renderView();
    expect(
      screen.getByText("Select a file to see it here."),
    ).toBeInTheDocument();
  });

  it("shows the row's own metadata and its text", async () => {
    useUiStore.setState({ openArtifactId: "art-1" });
    renderView();

    expect(
      await screen.findByRole("heading", {
        name: "connector-audit-findings.md",
      }),
    ).toBeInTheDocument();
    expect(screen.getByText("v2 of 2")).toBeInTheDocument();
    expect(screen.getByText("review_agent")).toBeInTheDocument();
    // The markdown is rendered, not printed: `# Findings` is the document's h1.
    await waitFor(() =>
      expect(screen.getByText("Findings")).toBeInTheDocument(),
    );
    expect(screen.getByText("All good.")).toBeInTheDocument();
  });

  it("loads an image straight from the content route with a token", async () => {
    useUiStore.setState({ openArtifactId: "art-2" });
    renderView();

    const image = await screen.findByAltText("spend.png");
    expect(image).toHaveAttribute(
      "src",
      "http://127.0.0.1:9999/v1/artifacts/art-2/content?token=test-token",
    );
  });

  it("says the file is gone rather than rendering a blank pane", async () => {
    const gone: Artifact = { ...findings, missing: true };
    route("/v1/artifacts/art-1", () => json(gone));
    useUiStore.setState({ openArtifactId: "art-1" });
    renderView();

    expect(await screen.findByText(/no longer on disk/i)).toBeInTheDocument();
    // The row is still real, so its identity is still shown.
    expect(
      screen.getByRole("heading", { name: "connector-audit-findings.md" }),
    ).toBeInTheDocument();
  });

  it("explains a 404 instead of hanging on an empty detail", async () => {
    route("/v1/artifacts/art-1", () =>
      json(
        { error: { code: "ARTIFACT_NOT_FOUND", message: "no such artifact" } },
        404,
      ),
    );
    useUiStore.setState({ openArtifactId: "art-1" });
    renderView();

    expect(
      await screen.findByText(/not in the library any more/i),
    ).toBeInTheDocument();
  });

  it("lists the versions newest first, with the lines each one changed", async () => {
    useUiStore.setState({ openArtifactId: "art-1", libraryTab: "history" });
    renderView();

    const rows = await screen.findAllByText(/^v[12]$/);
    expect(rows.map((row) => row.textContent)).toEqual(["v2", "v1"]);
    expect(screen.getByText("tightened the summary")).toBeInTheDocument();
    expect(screen.getByText("+3")).toBeInTheDocument();
    expect(screen.getByText("−1")).toBeInTheDocument();
    // v2 names its author; v1 has none, which is §4.8's hand edit rather than
    // a blank line.
    expect(screen.getAllByText("review_agent").length).toBeGreaterThan(0);
    expect(screen.getByText("edited by hand")).toBeInTheDocument();
  });

  it("draws the patch the diff route returned", async () => {
    route("/v1/artifacts/art-1/diff", () =>
      json({
        from: 1,
        to: 2,
        added_lines: 1,
        removed_lines: 1,
        format: "unified",
        patch: "--- v1\n+++ v2\n@@ -1 +1 @@\n-rough\n+tightened\n",
      }),
    );
    useUiStore.setState({ openArtifactId: "art-1", libraryTab: "diff" });
    renderView();

    expect(await screen.findByText("+tightened")).toBeInTheDocument();
    const url = urlsFor("/diff")[0] ?? "";
    expect(url).toContain("from=1");
    expect(url).toContain("to=2");
  });

  it("renders a 409 refusal's own message, never an empty diff", async () => {
    route("/v1/artifacts/art-1/diff", () =>
      json(
        {
          error: {
            code: "DIFF_TOO_LARGE",
            message: "v2 is 9 MiB; the diff cap is 8 MiB",
          },
        },
        409,
      ),
    );
    useUiStore.setState({ openArtifactId: "art-1", libraryTab: "diff" });
    renderView();

    expect(
      await screen.findByText(/the diff cap is 8 MiB/),
    ).toBeInTheDocument();
  });

  it("has nothing to diff for a single-version artifact, and says so", async () => {
    useUiStore.setState({ openArtifactId: "art-2", libraryTab: "diff" });
    renderView();

    expect(await screen.findByText(/only one version/i)).toBeInTheDocument();
    expect(urlsFor("/diff")).toHaveLength(0);
  });

  it("pins through the daemon and takes the server's answer", async () => {
    route("/v1/artifacts/art-1/pin", () => json({ id: "art-1", pinned: true }));
    useUiStore.setState({ openArtifactId: "art-1" });
    const user = userEvent.setup();
    renderView();

    await user.click(await screen.findByRole("button", { name: "☆ Pin" }));

    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "★ Pinned" }),
      ).toBeInTheDocument(),
    );
    const pin = requests.find((r) => r.url.includes("/pin"));
    expect(pin?.method).toBe("PUT");
    expect(pin?.body).toEqual({ pinned: true });
    expect(useUiStore.getState().isPinned("art-1")).toBe(true);
  });

  it("reverts the star when the daemon refuses the pin", async () => {
    route("/v1/artifacts/art-1/pin", () =>
      json({ error: { code: "DB_ERROR", message: "database is locked" } }, 500),
    );
    useUiStore.setState({ openArtifactId: "art-1" });
    const user = userEvent.setup();
    renderView();

    await user.click(await screen.findByRole("button", { name: "☆ Pin" }));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "☆ Pin" })).toBeInTheDocument(),
    );
    expect(useUiStore.getState().isPinned("art-1")).toBe(false);
  });

  it("marks a row the server says is pinned", async () => {
    listing([{ ...findings, pinned: true }], 1);
    renderView();

    const row = await screen.findByRole("button", {
      name: /connector-audit-findings\.md/,
    });
    expect(within(row).getByLabelText("Pinned")).toBeInTheDocument();
  });
});

describe("LibraryDetail — a file written into a project", () => {
  const notes: Artifact = {
    ...findings,
    id: "art-9",
    name: "docs/notes.md",
    task_id: null,
    task_title: null,
    path: "/Users/dev/app/docs/notes.md",
    project_root: "/Users/dev/app",
    rel_path: "docs/notes.md",
    version: 1,
    version_count: 0,
    in_project: true,
    openable: true,
  };

  beforeEach(() => {
    route("in_project=true", () => json({ artifacts: [notes], total: 1 }));
    route("/v1/artifacts/art-9/versions", () => json({ versions: [] }));
    route("/v1/artifacts/art-9/content", () =>
      text("# Notes\n\nKept in the project.\n"),
    );
    route("/v1/artifacts/art-9", () => json(notes));
    useUiStore.setState({ openArtifactId: "art-9", libraryResults: true });
  });

  it("shows its Preview even when History was the last tab, and offers no other", async () => {
    useUiStore.setState({ libraryTab: "history" });
    renderView();

    expect(await screen.findByText("Kept in the project.")).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Preview" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.queryByRole("tab", { name: "History" })).toBeNull();
    expect(screen.queryByText("No version history for this file.")).toBeNull();
  });
});

describe("Results — a file marked missing (d7-app-1, R-T26)", () => {
  const connection = {
    baseUrl: "http://127.0.0.1:9999",
    token: "test-token",
    instanceId: "7f3a1122",
  };
  const sentence =
    "/Users/dev/app: this project's folder is gone or has moved; re-point the project to read its files.";
  const marked: Artifact = {
    ...findings,
    id: "art-9",
    name: "docs/notes.md",
    task_id: null,
    task_title: null,
    path: "/Users/dev/app/docs/notes.md",
    project_root: "/Users/dev/app",
    rel_path: "docs/notes.md",
    version: 1,
    version_count: 0,
    in_project: true,
    openable: true,
    missing: true,
  };

  /** Whether the content route has found the file again (R37 clears the mark). */
  let back = false;

  beforeEach(() => {
    back = false;
    const row = () => ({ ...marked, missing: !back });
    route("in_project=true", () => json({ artifacts: [row()], total: 1 }));
    route("/v1/artifacts/art-9/versions", () => json({ versions: [] }));
    route("/v1/artifacts/art-9", () => json(row()));
    useUiStore.setState({ openArtifactId: "art-9", libraryResults: true });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.mocked(invoke).mockImplementation(async () => connection);
  });

  it("lists a marked result file with a missing tag, reads it again and drops the tag once it is back", async () => {
    route("/v1/artifacts/art-9/content", () => {
      back = true;
      return text("# Notes\n\nIt is back.\n");
    });
    renderView();

    expect(await screen.findByText("It is back.")).toBeInTheDocument();
    expect(urlsFor("include_missing=true")).not.toHaveLength(0);
    expect(urlsFor("/v1/artifacts/art-9/content")).toHaveLength(1);
    // The read cleared the mark on the daemon; the rows are read again and
    // the tag goes.
    await waitFor(() =>
      expect(screen.queryByText("missing")).not.toBeInTheDocument(),
    );
    expect(
      requests.filter((r) => r.url.endsWith("/v1/artifacts/art-9")).length,
    ).toBeGreaterThan(1);
  });

  it("draws the missing tag on a marked row", async () => {
    route("/v1/artifacts/art-9/content", () =>
      json(
        {
          error: {
            code: "ARTIFACT_GONE",
            message:
              "artifact art-9 is gone: /Users/dev/app/docs/notes.md no longer exists",
          },
        },
        410,
      ),
    );
    renderView();

    expect(await screen.findByText("missing")).toBeInTheDocument();
    expect(
      await screen.findByText(
        "This file is no longer at docs/notes.md in the project. Its row is kept, and the file is shown again once it is back.",
      ),
    ).toBeInTheDocument();
  });

  it("shows the daemon's sentence when the project's folder is gone or has moved", async () => {
    route("/v1/artifacts/art-9/content", () =>
      json(
        { error: { code: "ARTIFACT_PROJECT_MISSING", message: sentence } },
        410,
      ),
    );
    renderView();

    expect(await screen.findByText(sentence)).toBeInTheDocument();
    expect(screen.queryByText(/no longer at/)).not.toBeInTheDocument();
  });

  it("toasts Reveal's refusal for a missing project's file as the shell words it", async () => {
    vi.stubGlobal("isTauri", true);
    route("/v1/artifacts/art-9/content", () =>
      json(
        { error: { code: "ARTIFACT_PROJECT_MISSING", message: sentence } },
        410,
      ),
    );
    vi.mocked(invoke).mockImplementation(async (command: string) =>
      command === "reveal_path" ? Promise.reject(sentence) : connection,
    );
    const user = userEvent.setup();
    renderView();

    await user.click(
      await screen.findByRole("button", { name: revealLabel() }),
    );

    await waitFor(() => expect(useUiStore.getState().toast).toBe(sentence));
  });
});

describe("LibraryDetail — Open and Reveal", () => {
  /** What the mocked discovery command answers, as the factory above does. */
  const connection = {
    baseUrl: "http://127.0.0.1:9999",
    token: "test-token",
    instanceId: "7f3a1122",
  };

  beforeEach(() => {
    listing([findings], 1);
    route("/v1/artifacts/art-1/content", () => text("# Findings\n"));
    route("/v1/artifacts/art-1", () => json(findings));
    useUiStore.setState({ openArtifactId: "art-1" });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.mocked(invoke).mockImplementation(async () => connection);
  });

  it("opens the file through the daemon's open route, as `Reveal` used to", async () => {
    route("/v1/files/art-1/open", () =>
      json({ id: "art-1", status: "opened" }),
    );
    const user = userEvent.setup();
    renderView();

    await user.click(await screen.findByRole("button", { name: "Open" }));

    await waitFor(() =>
      expect(useUiStore.getState().toast).toBe(
        "connector-audit-findings.md opened in its default app",
      ),
    );
    const open = requests.find((r) => r.url.includes("/v1/files/art-1/open"));
    expect(open?.method).toBe("POST");
  });

  it("toasts the daemon's reason when the open route fails", async () => {
    route("/v1/files/art-1/open", () =>
      json(
        {
          error: {
            code: "OPEN_FAILED",
            message: "No application knows how to open this file",
          },
        },
        500,
      ),
    );
    const user = userEvent.setup();
    renderView();

    await user.click(await screen.findByRole("button", { name: "Open" }));

    await waitFor(() =>
      expect(useUiStore.getState().toast).toBe(
        "Could not open — No application knows how to open this file",
      ),
    );
  });

  it("draws no Reveal in the Vite preview, where there is no shell", async () => {
    renderView();

    expect(
      await screen.findByRole("button", { name: "Open" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: revealLabel() }),
    ).not.toBeInTheDocument();
  });

  it("asks the shell to reveal the artifact's path, and toasts its refusal", async () => {
    vi.stubGlobal("isTauri", true);
    const refusal = `${findings.path} is not there any more, so it cannot be shown in its folder.`;
    vi.mocked(invoke).mockImplementation(async (command: string) =>
      command === "reveal_path" ? Promise.reject(refusal) : connection,
    );
    const user = userEvent.setup();
    renderView();

    await user.click(
      await screen.findByRole("button", { name: revealLabel() }),
    );

    expect(invoke).toHaveBeenCalledWith("reveal_path", {
      path: findings.path,
    });
    await waitFor(() => expect(useUiStore.getState().toast).toBe(refusal));
    // Revealing is the shell's; nothing went to the daemon's open route.
    expect(requests.some((r) => r.url.includes("/open"))).toBe(false);
  });
});

describe("LibraryDetail — toasts name an agent's file escaped (d7-app-3, S6a M2)", () => {
  const connection = {
    baseUrl: "http://127.0.0.1:9999",
    token: "test-token",
    instanceId: "7f3a1122",
  };
  const hidden: Artifact = {
    ...findings,
    id: "art-h",
    name: "\u{202E}notes\u{200B}.md",
  };
  const shown = "<U+202E>notes<U+200B>.md";

  beforeEach(() => {
    listing([hidden], 1);
    route("/v1/artifacts/art-h/versions", () => json({ versions: [] }));
    route("/v1/artifacts/art-h/content", () => text("# Notes\n"));
    route("/v1/artifacts/art-h", () => json(hidden));
    useUiStore.setState({ openArtifactId: "art-h" });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.mocked(invoke).mockImplementation(async () => connection);
  });

  it("says the file opened", async () => {
    route("/v1/files/art-h/open", () =>
      json({ id: "art-h", status: "opened" }),
    );
    const user = userEvent.setup();
    renderView();

    await user.click(await screen.findByRole("button", { name: "Open" }));

    await waitFor(() =>
      expect(useUiStore.getState().toast).toBe(
        `${shown} opened in its default app`,
      ),
    );
  });

  it("quotes the open route's refusal", async () => {
    route("/v1/files/art-h/open", () =>
      json(
        {
          error: {
            code: "OPEN_NOT_OFFERED",
            message:
              "docs/\u{202E}run\u{200B}.sh is not a kind of file OpenAlpaca opens in another app",
          },
        },
        409,
      ),
    );
    const user = userEvent.setup();
    renderView();

    await user.click(await screen.findByRole("button", { name: "Open" }));

    await waitFor(() =>
      expect(useUiStore.getState().toast).toBe(
        "Could not open — docs/<U+202E>run<U+200B>.sh is not a kind of file OpenAlpaca opens in another app",
      ),
    );
  });

  it("quotes the shell's Reveal refusal", async () => {
    vi.stubGlobal("isTauri", true);
    vi.mocked(invoke).mockImplementation(async (command: string) =>
      command === "reveal_path"
        ? Promise.reject(
            "/w/docs/\u{202E}notes\u{200B}.md is not there any more, so it cannot be shown in its folder.",
          )
        : connection,
    );
    const user = userEvent.setup();
    renderView();

    await user.click(
      await screen.findByRole("button", { name: revealLabel() }),
    );

    await waitFor(() =>
      expect(useUiStore.getState().toast).toBe(
        "/w/docs/<U+202E>notes<U+200B>.md is not there any more, so it cannot be shown in its folder.",
      ),
    );
  });

  it("says the file was pinned", async () => {
    route("/v1/artifacts/art-h/pin", () => json({ id: "art-h", pinned: true }));
    const user = userEvent.setup();
    renderView();

    await user.click(await screen.findByRole("button", { name: "☆ Pin" }));

    await waitFor(() =>
      expect(useUiStore.getState().toast).toBe(`${shown} pinned`),
    );
  });

  it("quotes Export's failure", async () => {
    route("/v1/files/art-h/content", () =>
      json(
        {
          error: {
            code: "ARTIFACT_GONE",
            message:
              "artifact art-h is gone: /w/\u{202E}notes\u{200B}.md no longer exists",
          },
        },
        410,
      ),
    );
    const user = userEvent.setup();
    renderView();

    await user.click(await screen.findByRole("button", { name: "Export" }));

    await waitFor(() =>
      expect(useUiStore.getState().toast).toBe(
        "Export failed — artifact art-h is gone: /w/<U+202E>notes<U+200B>.md no longer exists",
      ),
    );
  });
});

describe("LibraryDetail — Export", () => {
  const projectFile: Artifact = {
    ...findings,
    id: "art-9",
    name: "docs/notes.md",
    task_id: null,
    task_title: null,
    path: "/Users/dev/app/docs/notes.md",
    project_root: "/Users/dev/app",
    rel_path: "docs/notes.md",
    version: 1,
    version_count: 0,
    in_project: true,
    openable: true,
  };
  /** The `download` name of every anchor the view clicked. */
  let saved: string[] = [];
  let click: { mockRestore: () => void } | null = null;
  /** What the environment had before the test defined the two statics. */
  let createDescriptor: PropertyDescriptor | undefined;
  let revokeDescriptor: PropertyDescriptor | undefined;

  beforeEach(() => {
    saved = [];
    route("in_project=true", () =>
      json({ artifacts: [projectFile], total: 1 }),
    );
    route("/v1/artifacts/art-9/versions", () => json({ versions: [] }));
    route("/v1/artifacts/art-9/content", () => text("# Notes\n"));
    route("/v1/artifacts/art-9", () => json(projectFile));
    // Export's bytes: the files route, which S4a.15 reads through the resolver.
    route("/v1/files/art-9/content", () => text("# Notes\n"));
    useUiStore.setState({ openArtifactId: "art-9", libraryResults: true });
    createDescriptor = Object.getOwnPropertyDescriptor(URL, "createObjectURL");
    revokeDescriptor = Object.getOwnPropertyDescriptor(URL, "revokeObjectURL");
    // jsdom has no object URLs; the two statics are defined for the test only.
    Object.defineProperty(URL, "createObjectURL", {
      value: vi.fn(() => "blob:notes"),
      configurable: true,
      writable: true,
    });
    Object.defineProperty(URL, "revokeObjectURL", {
      value: vi.fn(),
      configurable: true,
      writable: true,
    });
    click = vi
      .spyOn(HTMLAnchorElement.prototype, "click")
      .mockImplementation(function (this: HTMLAnchorElement) {
        saved.push(this.download);
      });
  });

  afterEach(() => {
    // Only this block's spy: the module's `invoke` double keeps its answer.
    click?.mockRestore();
    // Put back what was there, or nothing when nothing was.
    if (createDescriptor === undefined)
      Reflect.deleteProperty(URL, "createObjectURL");
    else Object.defineProperty(URL, "createObjectURL", createDescriptor);
    if (revokeDescriptor === undefined)
      Reflect.deleteProperty(URL, "revokeObjectURL");
    else Object.defineProperty(URL, "revokeObjectURL", revokeDescriptor);
  });

  it("says an agent's file was exported with its name escaped (S6a M2)", async () => {
    const hidden = { ...projectFile, name: "docs/\u{202E}notes\u{200B}.md" };
    route("in_project=true", () => json({ artifacts: [hidden], total: 1 }));
    route("/v1/artifacts/art-9", () => json(hidden));
    const user = userEvent.setup();
    renderView();

    await user.click(await screen.findByRole("button", { name: "Export" }));

    await waitFor(() =>
      expect(useUiStore.getState().toast).toBe(
        "docs/<U+202E>notes<U+200B>.md exported",
      ),
    );
  });

  it("saves a file written into the project under its own name, with no folder", async () => {
    const user = userEvent.setup();
    renderView();

    await user.click(await screen.findByRole("button", { name: "Export" }));

    await waitFor(() =>
      expect(useUiStore.getState().toast).toBe("docs/notes.md exported"),
    );
    expect(saved).toEqual(["notes.md"]);
    expect(urlsFor("/v1/files/art-9/content")).toHaveLength(1);
  });
});
