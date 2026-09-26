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

import {
  MEMORY_ACTION_CONFLICT,
  MEMORY_CONFLICT_COPY,
  MEMORY_EMPTY_COPY,
} from "./copy";
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
  /** The notes this one names in `supersedes` (design 09 §3.2 S8). */
  supersedes?: string[];
  /** A suggestion's `## Knowledge`, as the listing serves it. */
  knowledge?: string;
  /** A suggestion's reason, from its evidence sentence. */
  reason?: string | null;
  observed_at?: string | null;
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
/** How the next search answers when it cannot promise the whole answer. */
let searchShape: { partial: boolean; index_state: "ready" | "rebuilding" } = {
  partial: false,
  index_state: "ready",
};
/** While set, `GET /v1/status` waits on it — the daemon has not answered yet. */
let statusGate: Promise<void> | null = null;

/** The daemon's own account of a forget with nothing cited (`retained_note`). */
const RETAINED_NOTE =
  "The note files, their history and their index rows are gone, and the sources they cited will not be re-extracted. Session logs and any external backup are separate data and are not touched by a forget.";

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
    supersedes: n.supersedes ?? [],
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
    supersedes: n.supersedes ?? [],
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
    // Exact dedup (§7b P8): an active note of the same kind, title and body
    // in the same partition is answered, not written again.
    const same = visible(headers).find(
      (n) =>
        n.status === "active" &&
        n.scope === (input.scope ?? "global") &&
        n.kind === (input.kind ?? "fact") &&
        n.title === input.title &&
        n.body === input.body,
    );
    if (same !== undefined) {
      return json({
        id: same.id,
        revision: same.revision,
        scope: same.scope,
        origin: same.origin,
        superseded: [],
        dedup: "existing",
      });
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
    return json({ results, count: results.length, ...searchShape });
  }
  if (path === "/v1/memories/suggestions") {
    // Design 09 §3.5: a pending candidate naming one note that is still
    // there to replace, with both texts.
    const seen = visible(headers);
    const items = seen
      .filter((n) => n.status === "candidate" && n.supersedes?.length === 1)
      .flatMap((n) => {
        const target = seen.find(
          (t) => t.id === n.supersedes?.[0] && t.status === "active",
        );
        if (target === undefined) return [];
        return [
          {
            id: n.id,
            revision: n.revision,
            title: n.title,
            knowledge: n.knowledge ?? n.body,
            reason: n.reason ?? null,
            created_at: n.created_at,
            observed_at: n.observed_at ?? null,
            origin: n.origin,
            scope: n.scope,
            sources: n.sources,
            older_count: 0,
            target: {
              id: target.id,
              revision: target.revision,
              title: target.title,
              excerpt: target.body.trim(),
              origin: target.origin,
              scope: target.scope,
            },
          },
        ];
      });
    return json({ items });
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
  // `MemoryService::undo` reads the origin before it compares revisions; a
  // candidate of any origin is undoable — that is Keep mine (design 09 §3.3).
  if (
    tail === "/undo" &&
    found.origin !== "extracted_user" &&
    found.status !== "candidate"
  ) {
    return refusal(
      409,
      "MEMORY_NOT_UNDOABLE",
      `note ${id} is not undoable: its origin is \`${found.origin}\` — neither learned from your words (\`extracted_user\`) nor a pending suggestion; forget it instead`,
    );
  }
  if (expected !== found.revision) {
    return refusal(
      409,
      "MEMORY_REVISION_CONFLICT",
      `note ${id} is at revision ${found.revision}`,
    );
  }
  const retained = {
    transcript: false,
    session_logs: true,
    note: RETAINED_NOTE,
  };
  if (tail === "/undo") {
    // A withdrawal: what the note had replaced is back; a candidate retired
    // nothing.
    notes = notes.filter((n) => n.id !== id);
    const restored =
      found.status === "candidate" ? [] : (found.supersedes ?? []);
    return json({ id, status: "withdrawn", restored });
  }
  if (method === "DELETE") {
    notes = notes.filter((n) => n.id !== id);
    return json({ id, status: "forgotten", retained });
  }
  // POST /v1/memories/{id}
  const update = body as {
    action: string;
    patch?: { title?: string; body?: string };
    target_revision?: string;
    expected_revision?: unknown;
  };
  if (update.expected_revision !== undefined) {
    return refusal(
      400,
      "MEMORY_INVALID_INPUT",
      "expected_revision is not a body field",
    );
  }
  // S5: a suggestion's confirm binds the note it replaces.
  const targets = found.supersedes ?? [];
  let dismissed: string[] = [];
  if (update.action === "confirm" && targets.length > 0) {
    const target = notes.find((n) => n.id === targets[0]);
    if (update.target_revision === undefined) {
      return refusal(400, "MEMORY_INVALID_INPUT", "needs target_revision");
    }
    if (target === undefined) {
      return refusal(404, "MEMORY_NOT_FOUND", `no note ${targets[0] ?? ""}`);
    }
    if (target.revision !== update.target_revision) {
      return refusal(
        409,
        "MEMORY_REVISION_CONFLICT",
        `note ${target.id} is at revision ${target.revision}`,
      );
    }
    // A target retired some other way leaves nothing to replace
    // (`partition.rs`, the confirm arm).
    if (target.status !== "active") {
      return refusal(
        400,
        "MEMORY_INVALID_INPUT",
        `note ${target.id}, which note ${id} would replace, is no longer active (it is ${target.status}): there is nothing left for this suggestion to replace — undo it to dismiss it (\`openalpaca memory undo ${id}\`, or Keep mine in the Memory view)`,
      );
    }
    // S6: the other suggestions for the same note are dismissed.
    dismissed = notes
      .filter(
        (n) =>
          n.id !== id &&
          n.status === "candidate" &&
          n.supersedes?.[0] === targets[0],
      )
      .map((n) => n.id);
    notes = notes.filter((n) => !dismissed.includes(n.id));
    target.status = "archived"; // out of the active list: superseded
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
  return json({
    id,
    revision: found.revision,
    status: found.status,
    superseded: update.action === "confirm" ? targets : [],
    dismissed,
  });
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
  searchShape = { partial: false, index_state: "ready" };
  statusGate = null;
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
      if (url.pathname === "/v1/status" && statusGate !== null) {
        await statusGate;
      }
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

  it("waits for the daemon's project before honouring This project, and says nothing meanwhile", async () => {
    projectRoot = "/Users/me/repo";
    useProjectStore.setState({ path: "/Users/me/repo" });
    useUiStore.setState({ memoryScope: "project" });
    note({ id: "n-1", title: "Prefers tabs" });
    note({ id: "n-2", title: "Build with bun", scope: "project" });
    let answer: () => void = () => undefined;
    statusGate = new Promise((resolve) => {
      answer = resolve;
    });
    renderView();

    // Not answered yet is not "no project": Global is listed, and the line
    // that says the window has none is not drawn.
    expect(
      await screen.findByRole("button", { name: "Prefers tabs" }),
    ).toBeInTheDocument();
    expect(screen.queryByText(/This window has no project/)).toBeNull();
    expect(sent("GET", "/v1/memories")[0]?.url.searchParams.get("scope")).toBe(
      "global",
    );

    await act(async () => {
      answer();
      await Promise.resolve();
    });
    expect(
      await screen.findByRole("button", { name: "Build with bun" }),
    ).toBeInTheDocument();
    expect(screen.queryByText(/This window has no project/)).toBeNull();
    expect(
      sent("GET", "/v1/memories").at(-1)?.url.searchParams.get("scope"),
    ).toBe("project");
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

  it("shows the Markdown as written on the Raw tab", async () => {
    const body = "Hello **there**\n\n- one\n- two\n";
    note({ id: "n-1", title: "Plain", body });
    renderView();
    const user = await openNote("Plain");

    await user.click(screen.getByRole("tab", { name: "Raw" }));
    const raw = await screen.findByLabelText("Raw Markdown");
    expect(raw.tagName).toBe("PRE");
    expect(raw.textContent).toBe(body);
    expect(raw.querySelector("strong")).toBeNull();
    expect(screen.getByRole("button", { name: "Edit" })).toBeEnabled();
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

  it("says why an undo was refused once the note was made yours elsewhere", async () => {
    note({
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

    // Edited in another window: a new revision, and the note is the owner's.
    editBehindTheViewsBack("n-1", "Prefers a flat white.\n");
    const moved = notes[0];
    if (moved !== undefined) moved.origin = "user";
    await user.click(
      within(section).getByRole("button", { name: "Undo Prefers espresso" }),
    );

    await waitFor(() =>
      expect(useUiStore.getState().toast).toMatch(
        /^Could not undo — note n-1 is not undoable/,
      ),
    );
    expect(sent("POST", "/v1/memories/n-1/undo")).toHaveLength(1);
    expect(notes.map((n) => n.id)).toEqual(["n-1"]);
  });

  it("says the note moved when an undo is sent against an old revision", async () => {
    note({
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

    // Rewritten by the learner itself: still learned, at a new revision.
    editBehindTheViewsBack("n-1", "Prefers a double espresso.\n");
    await user.click(
      within(section).getByRole("button", { name: "Undo Prefers espresso" }),
    );

    await waitFor(() =>
      expect(useUiStore.getState().toast).toBe(MEMORY_ACTION_CONFLICT),
    );
    expect(sent("POST", "/v1/memories/n-1/undo")).toHaveLength(1);
    expect(notes.map((n) => n.id)).toEqual(["n-1"]);
  });

  it("says the note moved when an archive is sent against an old revision", async () => {
    note({ id: "n-1", title: "Old address" });
    renderView();
    const user = await openNote("Old address");
    editBehindTheViewsBack("n-1", "# Old address\n\nMoved in May.\n");
    const current = notes[0]?.revision ?? "";

    await user.click(screen.getByRole("button", { name: "Archive" }));
    await waitFor(() =>
      expect(useUiStore.getState().toast).toBe(MEMORY_ACTION_CONFLICT),
    );
    expect(notes[0]?.status).toBe("active");
    expect(sent("POST", "/v1/memories/n-1")).toHaveLength(1);

    // Never retried behind the owner's back: the view re-reads the note, and
    // the owner's next click is sent against the version now on screen.
    await user.click(screen.getByRole("button", { name: "Archive" }));
    await waitFor(() => expect(notes[0]?.status).toBe("archived"));
    const [refused, again] = sent("POST", "/v1/memories/n-1");
    expect(refused?.headers.get("If-Match")).not.toBe(`"${current}"`);
    expect(again?.headers.get("If-Match")).toBe(`"${current}"`);
  });

  it("says the note moved when a forget is sent against an old revision", async () => {
    note({ id: "n-1", title: "Old address" });
    renderView();
    const user = await openNote("Old address");
    editBehindTheViewsBack("n-1", "# Old address\n\nMoved in May.\n");

    await user.click(screen.getByRole("button", { name: "Forget" }));
    await user.click(screen.getByRole("button", { name: "Confirm forget" }));
    await waitFor(() =>
      expect(useUiStore.getState().toast).toBe(MEMORY_ACTION_CONFLICT),
    );
    expect(sent("DELETE", "/v1/memories/n-1")).toHaveLength(1);
    expect(notes.map((n) => n.id)).toEqual(["n-1"]);
    expect(screen.queryByText(/is forgotten/)).toBeNull();
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
    // Where the note was, what the forget did and did not erase — the
    // daemon's own words.
    expect(
      await screen.findByText("“Old address” is forgotten."),
    ).toBeInTheDocument();
    expect(screen.getByText(RETAINED_NOTE)).toBeInTheDocument();
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
    expect(screen.queryByText(/index is rebuilding/)).toBeNull();
    expect(screen.queryByText(/stopped at its limit/)).toBeNull();
  });

  it("says when a search answer is partial or came from a file scan", async () => {
    note({ id: "n-1", title: "Editor", body: "Prefers helix.\n" });
    searchShape = { partial: true, index_state: "rebuilding" };
    renderView();
    const user = userEvent.setup();

    await user.type(await screen.findByLabelText("Search memory"), "helix");
    expect(
      await screen.findByText(
        "The search index is rebuilding; these come from a slower file scan.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        /The search stopped at its limit, so there may be more; a narrower\s+query finds them\./,
      ),
    ).toBeInTheDocument();
    expect(screen.getByText("Prefers helix.")).toBeInTheDocument();
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

  it("opens the note already there when the same note is added again", async () => {
    note({
      id: "n-1",
      title: "Standup at nine",
      body: "Daily, 09:00.",
      kind: "procedure",
    });
    renderView();
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Add note" }));
    const form = screen.getByRole("region", { name: "Add a note" });
    await user.type(within(form).getByLabelText("Title"), "Standup at nine");
    await user.selectOptions(within(form).getByLabelText("Kind"), "procedure");
    await user.type(within(form).getByLabelText("Body"), "Daily, 09:00.");
    await user.click(within(form).getByRole("button", { name: "Save note" }));

    expect(
      await screen.findByRole("heading", { name: "Standup at nine", level: 2 }),
    ).toBeInTheDocument();
    expect(useUiStore.getState().toast).toBe(
      "Already remembered — opened the note that says so",
    );
    expect(useUiStore.getState().openMemoryId).toBe("n-1");
    expect(notes).toHaveLength(1);
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

  // ── Suggestions (Learning v2, design 09 §3.5) ──────────────────────────

  /** A note the owner saved, and a learned change to it waiting for them. */
  function savedAndSuggested() {
    const saved = note({
      id: "n-1",
      title: "My commute",
      body: "The user takes the bus.\n",
    });
    const suggestion = note({
      id: "n-2",
      title: "Commute",
      body: "# Commute\n\n## Knowledge\nSince March the user cycles.\n",
      knowledge: "Since March the user cycles.",
      reason: "The user said they cycle to the office now.",
      status: "candidate",
      origin: "extracted_user",
      observed_at: "2026-09-23T18:02:11Z",
      supersedes: ["n-1"],
    });
    return { saved, suggestion };
  }

  it("asks about a suggestion with both texts, the reason and whose words", async () => {
    savedAndSuggested();
    renderView();
    const section = await screen.findByRole("region", { name: "Suggestions" });
    expect(section).toHaveTextContent(
      "Replace “My commute” (saved by you) with “Commute”?",
    );
    expect(section).toHaveTextContent("YoursThe user takes the bus.");
    expect(section).toHaveTextContent("NewSince March the user cycles.");
    expect(section).toHaveTextContent(
      "WhyThe user said they cycle to the office now.",
    );
    expect(section).toHaveTextContent("From what you said on 2026-09-23");
  });

  it("labels a suggestion's texts by who wrote them", async () => {
    // An inferred change to a learned note: the note it replaces is not the
    // owner's own words.
    note({
      id: "n-1",
      title: "Editor",
      body: "The user uses vim.\n",
      origin: "extracted_user",
    });
    note({
      id: "n-2",
      title: "Editor change",
      knowledge: "The user uses Helix.",
      status: "candidate",
      origin: "extracted_other",
      observed_at: "2026-09-22T09:00:00Z",
      supersedes: ["n-1"],
    });
    // Suggestions whose own text changed after learning wrote them.
    note({ id: "n-3", title: "Commute", body: "The user takes the bus.\n" });
    note({
      id: "n-4",
      title: "Commute change",
      knowledge: "The user cycles.",
      status: "candidate",
      origin: "user",
      observed_at: "2026-09-23T18:02:11Z",
      supersedes: ["n-3"],
    });
    note({ id: "n-5", title: "Lunch", body: "Noodles.\n" });
    note({
      id: "n-6",
      title: "Lunch change",
      knowledge: "Rice.",
      status: "candidate",
      origin: "external",
      observed_at: "2026-09-21T12:00:00Z",
      supersedes: ["n-5"],
    });
    renderView();
    const section = await screen.findByRole("region", { name: "Suggestions" });
    const [inferred, edited, external] =
      within(section).getAllByRole("listitem");
    expect(inferred).toHaveTextContent(
      "Replace “Editor” (learned) with “Editor change”?",
    );
    expect(inferred).toHaveTextContent("CurrentThe user uses vim.");
    expect(inferred).not.toHaveTextContent("Yours");
    expect(inferred).toHaveTextContent(
      "Inferred from the conversation on 2026-09-22",
    );
    expect(edited).toHaveTextContent("YoursThe user takes the bus.");
    expect(edited).toHaveTextContent("Edited by you · suggested on 2026-09-23");
    expect(external).toHaveTextContent(
      "Edited outside OpenAlpaca · suggested on 2026-09-21",
    );
  });

  it("replaces with the suggestion's revision in If-Match and its target's as target_revision", async () => {
    const { saved, suggestion } = savedAndSuggested();
    renderView();
    const user = userEvent.setup();
    const section = await screen.findByRole("region", { name: "Suggestions" });
    const [read, target] = [suggestion.revision, saved.revision];
    await user.click(
      within(section).getByRole("button", { name: "Replace My commute" }),
    );

    await waitFor(() =>
      expect(sent("POST", "/v1/memories/n-2")).toHaveLength(1),
    );
    const confirm = sent("POST", "/v1/memories/n-2")[0];
    expect(confirm?.headers.get("If-Match")).toBe(`"${read}"`);
    expect(confirm?.body).toEqual({
      action: "confirm",
      target_revision: target,
    });
    await waitFor(() =>
      expect(useUiStore.getState().toast).toBe(
        "Replaced — “My commute” is kept as history",
      ),
    );
    await waitFor(() =>
      expect(screen.queryByRole("region", { name: "Suggestions" })).toBeNull(),
    );
  });

  it("keeps the owner's note with the undo route — a withdrawal", async () => {
    const { suggestion } = savedAndSuggested();
    renderView();
    const user = userEvent.setup();
    const section = await screen.findByRole("region", { name: "Suggestions" });
    await user.click(
      within(section).getByRole("button", { name: "Keep mine: My commute" }),
    );

    await waitFor(() =>
      expect(sent("POST", "/v1/memories/n-2/undo")).toHaveLength(1),
    );
    expect(
      sent("POST", "/v1/memories/n-2/undo")[0]?.headers.get("If-Match"),
    ).toBe(`"${suggestion.revision}"`);
    expect(sent("POST", "/v1/memories/n-2")).toHaveLength(0);
    await waitFor(() =>
      expect(useUiStore.getState().toast).toBe("Kept your note"),
    );
    expect(notes.map((n) => n.id)).toEqual(["n-1"]);
  });

  it("says a suggestion's candidate copy in the owner's terms, and replaces from the detail", async () => {
    const { saved } = savedAndSuggested();
    renderView();
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Candidates" }));
    const list = await screen.findByRole("list", { name: "Notes" });
    await user.click(
      await within(list).findByRole("button", { name: "Commute" }),
    );
    expect(
      await screen.findByText(
        "From what you said — would replace “My commute”. It is not used in answers until you confirm it.",
      ),
    ).toBeInTheDocument();
    const replace = await screen.findByRole("button", { name: "Replace" });
    await waitFor(() => expect(replace).toBeEnabled());
    await user.click(replace);
    await waitFor(() =>
      expect(sent("POST", "/v1/memories/n-2")).toHaveLength(1),
    );
    expect(sent("POST", "/v1/memories/n-2")[0]?.body).toEqual({
      action: "confirm",
      target_revision: saved.revision,
    });
  });

  it("keeps the owner's note from the detail once the note it would replace was retired another way", async () => {
    // The card lists only suggestions whose note is still in force, so the
    // detail is where this one is dismissed; its Replace is refused and its
    // refusal sends the owner to Keep mine.
    const { saved, suggestion } = savedAndSuggested();
    saved.status = "archived";
    renderView();
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Candidates" }));
    const list = await screen.findByRole("list", { name: "Notes" });
    await user.click(
      await within(list).findByRole("button", { name: "Commute" }),
    );
    expect(screen.queryByRole("region", { name: "Suggestions" })).toBeNull();

    const replace = await screen.findByRole("button", { name: "Replace" });
    await waitFor(() => expect(replace).toBeEnabled());
    await user.click(replace);
    await waitFor(() =>
      expect(useUiStore.getState().toast).toContain(
        "or Keep mine in the Memory view",
      ),
    );

    await user.click(screen.getByRole("button", { name: "Keep mine" }));
    await waitFor(() =>
      expect(sent("POST", "/v1/memories/n-2/undo")).toHaveLength(1),
    );
    expect(
      sent("POST", "/v1/memories/n-2/undo")[0]?.headers.get("If-Match"),
    ).toBe(`"${suggestion.revision}"`);
    await waitFor(() =>
      expect(useUiStore.getState().toast).toBe("Kept your note"),
    );
    expect(notes.map((n) => n.id)).toEqual(["n-1"]);
    expect(useUiStore.getState().openMemoryId).toBeNull();
  });

  it("names what a recently learned replacement replaced, and says it is back after the undo", async () => {
    note({ id: "n-1", title: "Old editor", status: "archived" });
    note({
      id: "n-2",
      title: "Editor preference",
      kind: "preference",
      origin: "extracted_user",
      supersedes: ["n-1"],
    });
    renderView();
    const user = userEvent.setup();
    const section = await screen.findByRole("region", {
      name: "Recently learned",
    });
    const row = within(section).getByRole("listitem");
    await waitFor(() =>
      expect(row).toHaveTextContent(
        "Learned from what you said · replaced “Old editor” · Undo",
      ),
    );
    await user.click(
      within(row).getByRole("button", { name: "Undo Editor preference" }),
    );
    await waitFor(() =>
      expect(useUiStore.getState().toast).toBe("Undone — “Old editor” is back"),
    );
  });

  // ── Hidden characters where the owner decides (lib/hidden-chars) ──────

  /** Every escape drawn set apart under `root`, in order. */
  const escapesIn = (root: Element) =>
    Array.from(
      root.querySelectorAll("[data-hidden-char]"),
      (node) => node.textContent,
    );
  const PLANTED = /[\u202E\u200B\u2800\uFE0F\uFFFC]/;

  /**
   * A suggestion's card is where the owner decides whether learning's text
   * replaces theirs, and both texts come from a file or from the model:
   * every hidden character in the titles, both texts and the reason is a
   * highlighted `<U+XXXX>`, never drawn raw, and one line under them counts
   * them. Replace still answers the notes as they were read.
   */
  it("shows a suggestion's hidden characters as <U+XXXX>, and counts them", async () => {
    const saved = note({
      id: "n-1",
      title: "My\u202E commute",
      body: "The user takes the\u200B bus.\n",
    });
    const suggestion = note({
      id: "n-2",
      title: "Com\u2800mute",
      body: "# Commute\n",
      knowledge: "Since March\uFE0F the user cycles.",
      reason: "The user said\uFFFC so.",
      status: "candidate",
      origin: "extracted_user",
      observed_at: "2026-09-23T18:02:11Z",
      supersedes: ["n-1"],
    });
    renderView();
    const user = userEvent.setup();
    const section = await screen.findByRole("region", { name: "Suggestions" });

    expect(section.textContent).not.toMatch(PLANTED);
    expect(escapesIn(section)).toEqual([
      "<U+202E>",
      "<U+2800>",
      "<U+200B>",
      "<U+FE0F>",
      "<U+FFFC>",
    ]);
    expect(section).toHaveTextContent(
      "Replace “My<U+202E> commute” (saved by you) with “Com<U+2800>mute”?",
    );
    expect(section).toHaveTextContent("YoursThe user takes the<U+200B> bus.");
    expect(section).toHaveTextContent(
      "Note: this request contains 5 hidden characters, shown as <U+…>.",
    );

    const [read, target] = [suggestion.revision, saved.revision];
    await user.click(
      within(section).getByRole("button", {
        name: "Replace My<U+202E> commute",
      }),
    );
    await waitFor(() =>
      expect(sent("POST", "/v1/memories/n-2")).toHaveLength(1),
    );
    const confirm = sent("POST", "/v1/memories/n-2")[0];
    expect(confirm?.headers.get("If-Match")).toBe(`"${read}"`);
    expect(confirm?.body).toEqual({
      action: "confirm",
      target_revision: target,
    });
  });

  it("says nothing about hidden characters on a suggestion that holds none", async () => {
    savedAndSuggested();
    renderView();
    const section = await screen.findByRole("region", { name: "Suggestions" });
    expect(escapesIn(section)).toEqual([]);
    expect(section).not.toHaveTextContent("hidden character");
  });

  /** Undo takes back a note learning wrote: its title is the model's. */
  it("shows a recently learned note's hidden characters beside its Undo", async () => {
    note({ id: "n-1", title: "Old\u200B editor", status: "archived" });
    note({
      id: "n-2",
      title: "Prefers\u202E espresso",
      kind: "preference",
      origin: "extracted_user",
      supersedes: ["n-1"],
    });
    renderView();
    const user = userEvent.setup();
    const section = await screen.findByRole("region", {
      name: "Recently learned",
    });
    const row = within(section).getByRole("listitem");
    await waitFor(() =>
      expect(row).toHaveTextContent(
        "Prefers<U+202E> espresso" +
          "Learned from what you said · replaced “Old<U+200B> editor” · Undo" +
          "Note: this request contains 2 hidden characters, shown as <U+…>.",
      ),
    );
    expect(row.textContent).not.toMatch(PLANTED);
    expect(escapesIn(row)).toEqual(["<U+202E>", "<U+200B>"]);

    await user.click(
      within(row).getByRole("button", {
        name: "Undo Prefers<U+202E> espresso",
      }),
    );
    await waitFor(() =>
      expect(sent("POST", "/v1/memories/n-2/undo")).toHaveLength(1),
    );
  });

  /**
   * A candidate is confirmed from its detail: its title and its text — the
   * model's — show every hidden character, and one line under the text
   * counts them (the note's title included). Confirm sends what it sent.
   */
  it("shows a candidate's hidden characters where it is confirmed", async () => {
    const candidate = note({
      id: "n-1",
      title: "Likes\u202E dark mode",
      body: "The user\u200B likes dark\u2800 themes.\n",
      status: "candidate",
      origin: "extracted_other",
    });
    renderView();
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Candidates" }));
    const list = await screen.findByRole("list", { name: "Notes" });
    await user.click(
      await within(list).findByRole("button", {
        name: "Likes\u202E dark mode",
      }),
    );
    const heading = await screen.findByRole("heading", {
      name: "Likes<U+202E> dark mode",
      level: 2,
    });
    const detail = heading.closest("section");
    if (detail === null) throw new Error("no detail");
    expect(detail.textContent).not.toMatch(PLANTED);
    expect(escapesIn(heading)).toEqual(["<U+202E>"]);
    expect(detail).toHaveTextContent(
      "The user<U+200B> likes dark<U+2800> themes.",
    );
    expect(detail).toHaveTextContent(
      "Note: this request contains 3 hidden characters, shown as <U+…>.",
    );

    const read = candidate.revision;
    await user.click(screen.getByRole("button", { name: "Confirm" }));
    await waitFor(() =>
      expect(sent("POST", "/v1/memories/n-1")).toHaveLength(1),
    );
    expect(sent("POST", "/v1/memories/n-1")[0]?.body).toEqual({
      action: "confirm",
    });
    expect(sent("POST", "/v1/memories/n-1")[0]?.headers.get("If-Match")).toBe(
      `"${read}"`,
    );
  });

  /**
   * Only where a decision is offered: a note that is already the owner's is
   * read as written — a joiner or a right-to-left mark can be its content.
   */
  it("shows an active note's text as written, with no count", async () => {
    note({
      id: "n-1",
      title: "Family",
      body: "👩\u200D👧 at home.\n",
    });
    renderView();
    await openNote("Family");
    const detail = screen
      .getByRole("heading", { name: "Family", level: 2 })
      .closest("section");
    if (detail === null) throw new Error("no detail");
    expect(detail.textContent).toContain("👩\u200D👧 at home.");
    expect(detail).not.toHaveTextContent("hidden character");
  });
});
