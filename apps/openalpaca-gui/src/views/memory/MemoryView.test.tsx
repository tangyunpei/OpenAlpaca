/**
 * The Memory view end to end, over the real data layer (memory redesign,
 * design §7 *Surfaces*, plan 3.9).
 *
 * Only the two edges are doubled — the Tauri discovery command and `fetch` —
 * and `fetch` is answered by a small fake daemon that keeps its notes in
 * memory and enforces what the real one enforces: a revision is checked from
 * `If-Match` and nowhere else, a stale one is a `409`, an undo is refused once
 * the note is no longer `extracted_user`. So what these tests assert about the
 * wire is what the daemon would receive, and a flow that only works against a
 * lenient fixture cannot pass.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { resetConnection } from "@/lib/connection";
import { useProjectStore } from "@/stores/project";
import { useUiStore } from "@/stores/ui";

import { MEMORY_CONFLICT_COPY, MEMORY_EMPTY_COPY } from "./copy";
import MemoryView from "./MemoryView";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => ({
    baseUrl: "http://127.0.0.1:9999",
    token: "test-token",
    instanceId: "7f3a1122",
  })),
}));

// ── A fake daemon ───────────────────────────────────────────────────────────

interface FakeNote {
  id: string;
  revision: string;
  title: string;
  body: string;
  kind: string;
  scope: "global" | "project";
  status: "active" | "candidate" | "archived";
  origin: string;
  created_at: string;
  updated_at: string;
  sources: { type: string; id: string; observed_at: string | null }[];
  /** Older kept copies, newest first, each `{revision, title, body}`. */
  history: { revision: string; title: string; body: string }[];
  /** The file is longer than the 32 KiB a read serves (external files only). */
  truncated?: boolean;
}

interface Recorded {
  url: URL;
  method: string;
  body: Record<string, unknown> | null;
  headers: Headers;
}

let requests: Recorded[] = [];
let notes: FakeNote[] = [];
let projectRoot: string | null = null;
let revisionCounter = 0;

function nextRevision(): string {
  revisionCounter += 1;
  return revisionCounter.toString(16).padStart(64, "0");
}

function json(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), { status });
}

function refusal(status: number, code: string, message: string): Response {
  return json({ error: { code, message } }, status);
}

function note(fields: Partial<FakeNote> & { id: string; title: string }) {
  const created: FakeNote = {
    revision: nextRevision(),
    body: `# ${fields.title}\n`,
    kind: "fact",
    scope: "global",
    status: "active",
    origin: "user",
    created_at: "2026-09-24T10:00:00Z",
    updated_at: "2026-09-24T10:00:00Z",
    sources: [],
    history: [],
    ...fields,
  };
  notes.push(created);
  return created;
}

function listItem(n: FakeNote) {
  return {
    id: n.id,
    revision: n.revision,
    title: n.title,
    kind: n.kind,
    scope: n.scope,
    status: n.status,
    effective_status: n.status,
    origin: n.origin,
    updated_at: n.updated_at,
    created_at: n.created_at,
    pinned: false,
  };
}

function readOutput(n: FakeNote, kept?: { title: string; body: string }) {
  return {
    ...listItem(n),
    title: kept?.title ?? n.title,
    body: kept?.body ?? n.body,
    frontmatter: { updated_at: n.updated_at },
    truncated: n.truncated ?? false,
    path: `notes/${n.id}--${n.title.toLowerCase().replace(/\W+/g, "-")}.md`,
    sources: n.sources,
    sources_status: "intact",
    supersedes: [],
    superseded_by: [],
    freshness: "verified",
    conflict: false,
  };
}

/** `If-Match: "<rev>"` → `<rev>`, the way the daemon reads it. */
function ifMatch(headers: Headers): string | null {
  const raw = headers.get("If-Match");
  return raw === null ? null : raw.replace(/^"|"$/g, "");
}

/** Every note this request's header lets it see (design §3 rule 2). */
function visible(headers: Headers): FakeNote[] {
  const project =
    headers.has("x-workspace-path") && projectRoot !== null
      ? projectRoot
      : null;
  return notes.filter((n) => n.scope === "global" || project !== null);
}

function daemon(url: URL, method: string, headers: Headers, body: unknown) {
  const path = url.pathname;
  if (path === "/v1/status") {
    return json({
      project_root: headers.has("x-workspace-path") ? projectRoot : null,
    });
  }
  if (path === "/v1/memories" && method === "GET") {
    const scope = url.searchParams.get("scope");
    const status = url.searchParams.get("status") ?? "active";
    const items = visible(headers)
      .filter((n) => scope === null || n.scope === scope)
      .filter((n) => n.status === status)
      .map(listItem);
    return json({ items, next_cursor: null });
  }
  if (path === "/v1/memories" && method === "POST") {
    const input = body as Record<string, string>;
    if (input.scope === "project" && projectRoot === null) {
      return refusal(400, "MEMORY_INVALID_INPUT", "no project partition");
    }
    const created = note({
      id: `n-${notes.length + 1}`,
      title: input.title ?? "",
      body: input.body ?? "",
      kind: input.kind ?? "fact",
      scope: (input.scope as "global" | "project") ?? "global",
    });
    return json(
      {
        id: created.id,
        revision: created.revision,
        scope: created.scope,
        origin: "user",
        superseded: [],
        dedup: "created",
      },
      201,
    );
  }
  if (path === "/v1/memories/search") {
    const q = (url.searchParams.get("q") ?? "").toLowerCase();
    const scope = url.searchParams.get("scope");
    const results = visible(headers)
      .filter((n) => scope === null || n.scope === scope)
      .filter((n) => n.status === "active")
      .filter((n) => `${n.title}\n${n.body}`.toLowerCase().includes(q))
      .map((n) => ({
        ...readOutput(n),
        excerpt:
          n.body.split("\n").find((l) => l.toLowerCase().includes(q)) ?? "",
        heading: null,
        line_range: [1, 1],
        match: ["term"],
        same_key_loser: false,
      }));
    return json({
      results,
      count: results.length,
      partial: false,
      index_state: "ready",
    });
  }
  if (path === "/v1/memories/recently-learned") {
    const items = visible(headers)
      .filter((n) => n.origin === "extracted_user" && n.status === "active")
      .map(listItem);
    return json({ items });
  }

  const match = /^\/v1\/memories\/([^/]+)(\/history|\/undo)?$/.exec(path);
  const id = match?.[1] ?? "";
  const found = visible(headers).find((n) => n.id === id);
  if (match === null) return json({ error: "not found" }, 404);
  if (found === undefined) {
    return refusal(404, "MEMORY_NOT_FOUND", `no note ${id}`);
  }
  const tail = match[2];

  if (tail === "/history") {
    return json({
      revisions: [
        { revision: found.revision, written_at: found.updated_at },
        ...found.history.map((h) => ({
          revision: h.revision,
          written_at: "2026-09-23T09:00:00Z",
        })),
      ],
    });
  }
  if (method === "GET") {
    const wanted = url.searchParams.get("revision");
    if (wanted === null) return json(readOutput(found));
    const kept = found.history.find((h) => h.revision === wanted);
    return kept === undefined
      ? refusal(404, "MEMORY_NOT_FOUND", "revision unavailable")
      : json({ ...readOutput(found, kept), revision: wanted });
  }

  // Every mutation below addresses one note under If-Match.
  const expected = ifMatch(headers);
  if (expected === null) {
    return refusal(428, "MEMORY_IF_MATCH_REQUIRED", "send If-Match");
  }
  if (expected !== found.revision) {
    return refusal(
      409,
      "MEMORY_REVISION_CONFLICT",
      `note ${id} is at revision ${found.revision}`,
    );
  }
  const retained = { transcript: true, session_logs: true, note: "kept" };
  if (tail === "/undo") {
    if (found.origin !== "extracted_user") {
      return refusal(409, "MEMORY_NOT_UNDOABLE", "no longer learned");
    }
    notes = notes.filter((n) => n.id !== id);
    return json({ id, status: "forgotten", retained });
  }
  if (method === "DELETE") {
    notes = notes.filter((n) => n.id !== id);
    return json({ id, status: "forgotten", retained });
  }
  // POST /v1/memories/{id}
  const update = body as {
    action: string;
    patch?: { title?: string; body?: string };
    expected_revision?: unknown;
  };
  if (update.expected_revision !== undefined) {
    return refusal(
      400,
      "MEMORY_INVALID_INPUT",
      "expected_revision is not a body field",
    );
  }
  found.history.unshift({
    revision: found.revision,
    title: found.title,
    body: found.body,
  });
  if (update.action === "edit") {
    found.title = update.patch?.title ?? found.title;
    found.body = update.patch?.body ?? found.body;
    found.origin = "user";
  } else if (update.action === "archive") {
    found.status = "archived";
  } else if (update.action === "confirm") {
    found.status = "active";
    found.origin = "user";
  }
  found.revision = nextRevision();
  return json({ id, revision: found.revision, status: found.status });
}

/** An edit made behind the view's back — another window, or the model. */
function editBehindTheViewsBack(id: string, body: string): void {
  const found = notes.find((n) => n.id === id);
  if (found === undefined) throw new Error(`no note ${id}`);
  found.history.unshift({
    revision: found.revision,
    title: found.title,
    body: found.body,
  });
  found.body = body;
  found.revision = nextRevision();
}

const initialUi = useUiStore.getState();

beforeEach(() => {
  requests = [];
  notes = [];
  projectRoot = null;
  revisionCounter = 0;
  resetConnection();
  useProjectStore.setState({ path: null });
  useUiStore.setState({
    ...initialUi,
    view: "memory",
    openMemoryId: null,
    memoryTab: "note",
    memoryScope: "global",
    memoryStatus: "active",
    toast: null,
  });

  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: unknown, init?: RequestInit) => {
      const url = new URL(String(input));
      const method = init?.method ?? "GET";
      const headers = new Headers(init?.headers);
      const body =
        typeof init?.body === "string"
          ? (JSON.parse(init.body) as Record<string, unknown>)
          : null;
      requests.push({ url, method, body, headers });
      return daemon(url, method, headers, body);
    }),
  );
});

let client: QueryClient;

function renderView() {
  client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryView />
    </QueryClientProvider>,
  );
}

/**
 * What a `memory_changed` frame does — the view has no socket of its own in
 * these tests, so the invalidation the bridge would apply is applied here.
 */
async function memoryChanged(): Promise<void> {
  await act(async () => {
    await client.invalidateQueries({ queryKey: ["memory"] });
  });
}

function sent(method: string, pathname: string): Recorded[] {
  return requests.filter(
    (r) => r.method === method && r.url.pathname === pathname,
  );
}

/** Open a note from the list column (not from a recently-learned row). */
async function openNote(title: string) {
  const user = userEvent.setup();
  const list = await screen.findByRole("list", { name: "Notes" });
  await user.click(await within(list).findByRole("button", { name: title }));
  await screen.findByRole("heading", { name: title, level: 2 });
  return user;
}

// ── Tests ───────────────────────────────────────────────────────────────────

describe("the Memory view", () => {
  it("says the design's empty sentence when nothing is remembered", async () => {
    renderView();
    expect(await screen.findByText(MEMORY_EMPTY_COPY)).toBeInTheDocument();
    expect(MEMORY_EMPTY_COPY).toBe(
      "Nothing remembered yet. Ask Alpaca to remember something, or add a note.",
    );
  });

  it("lists the global partition, and sends no project for a window with none", async () => {
    note({ id: "n-1", title: "Prefers tabs" });
    note({ id: "n-2", title: "Build with bun", scope: "project" });
    renderView();

    expect(
      await screen.findByRole("button", { name: "Prefers tabs" }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Build with bun" })).toBeNull();

    const list = sent("GET", "/v1/memories")[0];
    expect(list?.url.searchParams.get("scope")).toBe("global");
    expect(list?.url.searchParams.get("status")).toBe("active");
    expect(list?.headers.has("x-workspace-path")).toBe(false);
  });

  it("offers This project only once the daemon resolved the window's project", async () => {
    renderView();
    const project = await screen.findByRole("button", { name: "This project" });
    expect(project).toBeDisabled();
  });

  it("says so when This project was chosen and the window has none", async () => {
    useUiStore.setState({ memoryScope: "project" });
    renderView();
    expect(
      await screen.findByText(
        /This window has no project, or its path is inside none/,
      ),
    ).toBeInTheDocument();
    await waitFor(() =>
      expect(
        sent("GET", "/v1/memories")[0]?.url.searchParams.get("scope"),
      ).toBe("global"),
    );
  });

  it("lists the project partition with the window's project as the header", async () => {
    projectRoot = "/Users/me/repo";
    useProjectStore.setState({ path: "/Users/me/repo" });
    note({ id: "n-1", title: "Prefers tabs" });
    note({ id: "n-2", title: "Build with bun", scope: "project" });
    renderView();
    const user = userEvent.setup();

    const project = await screen.findByRole("button", { name: "This project" });
    await waitFor(() => expect(project).toBeEnabled());
    await user.click(project);

    expect(
      await screen.findByRole("button", { name: "Build with bun" }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Prefers tabs" })).toBeNull();
    const list = sent("GET", "/v1/memories").at(-1);
    expect(list?.url.searchParams.get("scope")).toBe("project");
    expect(list?.headers.get("x-workspace-path")).toBe("/Users/me/repo");
  });

  it("renders a note's Markdown as elements and never as HTML", async () => {
    note({
      id: "n-1",
      title: "Hostile note",
      body: "Hello **there**\n\n<img src=x onerror=alert(1)>\n\n<script>alert(1)</script>\n",
      origin: "external",
    });
    const { container } = renderView();
    await openNote("Hostile note");

    expect((await screen.findByText("there")).tagName).toBe("STRONG");
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("script")).toBeNull();
    expect(
      screen.getByText("<img src=x onerror=alert(1)>"),
    ).toBeInTheDocument();
  });

  it("names where a note came from, and opens the run it cites", async () => {
    note({
      id: "n-1",
      title: "Deploys on Fridays",
      origin: "extracted_user",
      sources: [
        {
          type: "task",
          id: "7c1e0b2a-4d61-4f0e-9a3c-2b7f1d5e8c90",
          observed_at: null,
        },
        { type: "message", id: "4812", observed_at: null },
      ],
    });
    renderView();
    const user = await openNote("Deploys on Fridays");

    const evidence = screen.getByRole("region", {
      name: "Where this came from",
    });
    expect(
      within(evidence).getByText("Learned from what you said"),
    ).toBeInTheDocument();
    expect(within(evidence).getByText("message 4812")).toBeInTheDocument();

    await user.click(
      within(evidence).getByRole("button", { name: "run 7c1e0b2a" }),
    );
    expect(useUiStore.getState().view).toBe("work");
    expect(useUiStore.getState().selectedRunId).toBe(
      "7c1e0b2a-4d61-4f0e-9a3c-2b7f1d5e8c90",
    );
  });

  it("confirms a candidate with the revision it read, in If-Match", async () => {
    const candidate = note({
      id: "n-1",
      title: "Likes dark mode",
      status: "candidate",
      origin: "extracted_other",
    });
    renderView();
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Candidates" }));
    const list = await screen.findByRole("list", { name: "Notes" });
    await user.click(
      await within(list).findByRole("button", { name: "Likes dark mode" }),
    );
    const read = candidate.revision;
    await user.click(await screen.findByRole("button", { name: "Confirm" }));

    await waitFor(() =>
      expect(sent("POST", "/v1/memories/n-1")).toHaveLength(1),
    );
    const confirm = sent("POST", "/v1/memories/n-1")[0];
    expect(confirm?.body).toEqual({ action: "confirm" });
    expect(confirm?.headers.get("If-Match")).toBe(`"${read}"`);
  });

  it("undoes a recently learned note in one step, from its row", async () => {
    const learned = note({
      id: "n-1",
      title: "Prefers espresso",
      kind: "preference",
      origin: "extracted_user",
    });
    renderView();
    const user = userEvent.setup();

    const section = await screen.findByRole("region", {
      name: "Recently learned",
    });
    const row = within(section).getByRole("listitem");
    expect(row).toHaveTextContent("Prefers espresso");
    expect(row).toHaveTextContent("Learned from what you said · Undo");

    await user.click(
      within(row).getByRole("button", { name: "Undo Prefers espresso" }),
    );
    await waitFor(() =>
      expect(sent("POST", "/v1/memories/n-1/undo")).toHaveLength(1),
    );
    expect(
      sent("POST", "/v1/memories/n-1/undo")[0]?.headers.get("If-Match"),
    ).toBe(`"${learned.revision}"`);
    await waitFor(() =>
      expect(
        screen.queryByRole("region", { name: "Recently learned" }),
      ).toBeNull(),
    );
  });

  it("forgets only on the second click", async () => {
    const n = note({ id: "n-1", title: "Old address" });
    renderView();
    const user = await openNote("Old address");

    await user.click(screen.getByRole("button", { name: "Forget" }));
    expect(sent("DELETE", "/v1/memories/n-1")).toHaveLength(0);

    await user.click(screen.getByRole("button", { name: "Confirm forget" }));
    await waitFor(() =>
      expect(sent("DELETE", "/v1/memories/n-1")).toHaveLength(1),
    );
    expect(sent("DELETE", "/v1/memories/n-1")[0]?.headers.get("If-Match")).toBe(
      `"${n.revision}"`,
    );
  });

  it("keeps the draft and says so when the note changed underneath the editor", async () => {
    note({ id: "n-1", title: "Editor", body: "Uses vim.\n" });
    renderView();
    const user = await openNote("Editor");

    await user.click(screen.getByRole("tab", { name: "Raw" }));
    await user.click(await screen.findByRole("button", { name: "Edit" }));
    const body = screen.getByLabelText("Body");
    await user.clear(body);
    await user.type(body, "Uses helix.");

    // Another writer moves the note before this one saves.
    editBehindTheViewsBack("n-1", "Uses emacs.\n");
    await user.click(screen.getByRole("button", { name: "Save" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(MEMORY_CONFLICT_COPY);
    expect(MEMORY_CONFLICT_COPY).toBe(
      "This note changed since you opened it. Reload to see the current version — your text is kept.",
    );
    expect(screen.getByLabelText("Body")).toHaveValue("Uses helix.");
    // The 409 is the conflict, said in this window's words — not a failed
    // save reading out the daemon's revision hashes.
    expect(screen.queryByText(/Could not save/)).toBeNull();

    await user.click(within(alert).getByRole("button", { name: "Reload" }));
    const current = await screen.findByRole("region", {
      name: "Current version",
    });
    expect(current).toHaveTextContent("Uses emacs.");
    expect(screen.getByLabelText("Body")).toHaveValue("Uses helix.");

    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(sent("POST", "/v1/memories/n-1")).toHaveLength(2),
    );
    const [first, second] = sent("POST", "/v1/memories/n-1");
    expect(first?.headers.get("If-Match")).not.toBe(
      second?.headers.get("If-Match"),
    );
    expect(second?.body).toEqual({
      action: "edit",
      patch: { body: "Uses helix." },
    });
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(notes[0]?.body).toBe("Uses helix.");
  });

  it("will not edit a note cut at 32 KiB, and names the one safe place to edit it", async () => {
    note({
      id: "n-1",
      title: "Long log",
      body: "# Long log\n\nThe first 32 KiB of it.\n",
      origin: "external",
      truncated: true,
    });
    renderView();
    const user = await openNote("Long log");

    await user.click(screen.getByRole("tab", { name: "Raw" }));
    const edit = await screen.findByRole("button", { name: "Edit" });
    expect(edit).toBeDisabled();
    // Saving the cut would write it over the whole file, and the CLI's
    // `memory edit` reads the same cut — only the file itself is safe.
    expect(edit).toHaveAttribute(
      "title",
      "Longer than 32 KiB — edit the file itself.",
    );
  });

  it("compares the current note with a kept revision", async () => {
    note({ id: "n-1", title: "Editor", body: "# Editor\n\nUses vim.\n" });
    editBehindTheViewsBack("n-1", "# Editor\n\nUses helix.\n");
    const kept = notes[0]?.history[0]?.revision ?? "";
    renderView();
    const user = await openNote("Editor");

    await user.click(screen.getByRole("tab", { name: "Diff" }));
    expect(await screen.findByText("+Uses helix.")).toBeInTheDocument();
    expect(screen.getByText("−Uses vim.")).toBeInTheDocument();
    expect(
      sent("GET", "/v1/memories/n-1").some(
        (r) => r.url.searchParams.get("revision") === kept,
      ),
    ).toBe(true);
  });

  it("lists the kept revisions and compares from one", async () => {
    note({ id: "n-1", title: "Editor", body: "Uses vim.\n" });
    editBehindTheViewsBack("n-1", "Uses helix.\n");
    renderView();
    const user = await openNote("Editor");

    await user.click(screen.getByRole("tab", { name: "History" }));
    const kept = await screen.findByRole("list", { name: "Kept versions" });
    const rows = within(kept).getAllByRole("listitem");
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent("current");

    await user.click(
      within(rows[1] as HTMLElement).getByRole("button", { name: /Compare/ }),
    );
    expect(useUiStore.getState().memoryTab).toBe("diff");
    expect(await screen.findByText("+Uses helix.")).toBeInTheDocument();
  });

  it("searches with what was typed, in the scope on screen", async () => {
    note({
      id: "n-1",
      title: "Editor",
      body: "Prefers helix as the editor.\n",
    });
    note({ id: "n-2", title: "Coffee", body: "Espresso.\n" });
    renderView();
    const user = userEvent.setup();

    await user.type(await screen.findByLabelText("Search memory"), "helix");
    expect(
      await screen.findByText("Prefers helix as the editor."),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Coffee" })).toBeNull();
    const search = sent("GET", "/v1/memories/search").at(-1);
    expect(search?.url.searchParams.get("q")).toBe("helix");
    expect(search?.url.searchParams.get("scope")).toBe("global");
  });

  it("adds a note to the scope on screen and opens it", async () => {
    renderView();
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Add note" }));
    const form = screen.getByRole("region", { name: "Add a note" });
    await user.type(within(form).getByLabelText("Title"), "Standup at nine");
    await user.selectOptions(within(form).getByLabelText("Kind"), "procedure");
    await user.type(within(form).getByLabelText("Body"), "Daily, 09:00.");
    await user.click(within(form).getByRole("button", { name: "Save note" }));

    await waitFor(() => expect(sent("POST", "/v1/memories")).toHaveLength(1));
    expect(sent("POST", "/v1/memories")[0]?.body).toEqual({
      title: "Standup at nine",
      body: "Daily, 09:00.",
      kind: "procedure",
      scope: "global",
    });
    expect(
      await screen.findByRole("heading", { name: "Standup at nine", level: 2 }),
    ).toBeInTheDocument();
  });

  it("refetches on memory_changed instead of polling", async () => {
    renderView();
    await screen.findByText(MEMORY_EMPTY_COPY);
    note({ id: "n-1", title: "Written by the model" });
    await memoryChanged();
    expect(
      await screen.findByRole("button", { name: "Written by the model" }),
    ).toBeInTheDocument();
  });
});
