/**
 * The Library end to end, over the real data layer.
 *
 * Only two edges are doubled — the Tauri discovery command and `fetch` — so the
 * query keys, the request URLs and the `?token=` preview URL under test are the
 * production ones. Nothing here is a fixture standing in for the daemon: every
 * row rendered came out of a response body.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Artifact } from "@/lib/api/artifacts";
import { resetConnection } from "@/lib/connection";
import { useUiStore } from "@/stores/ui";

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
