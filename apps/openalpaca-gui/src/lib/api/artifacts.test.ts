/**
 * The `/v1/artifacts*` client, over the real `apiFetch` stack.
 *
 * Only the two edges are doubled — the Tauri discovery command and `fetch` — so
 * the query string, the method and the body under test are the ones the daemon
 * would actually receive.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";

import { bootstrapConnection, resetConnection } from "@/lib/connection";
import { ApiError } from "@/lib/http";

import {
  artifactContentUrl,
  fileContentUrl,
  getArtifact,
  getArtifactDiff,
  listArtifactVersions,
  listArtifacts,
  setArtifactPinned,
  type Artifact,
} from "./artifacts";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => ({
    baseUrl: "http://127.0.0.1:9999",
    token: "tok en/+",
    instanceId: "7f3a1122",
  })),
}));

interface Recorded {
  url: string;
  method: string;
  body: unknown;
}

let requests: Recorded[] = [];
let reply: (url: string) => Response;

function json(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), { status });
}

const row: Artifact = {
  id: "art-1",
  name: "findings.md",
  kind: "markdown",
  mime_type: "text/markdown",
  size_bytes: 26,
  task_id: "task-1",
  task_title: "connector audit",
  agent_id: "agent-1",
  agent_template_id: "review_agent",
  version: 2,
  version_count: 2,
  summary: "+1 −0",
  metadata: null,
  created_at: "2026-09-05T10:00:00Z",
  updated_at: "2026-09-05T10:05:00Z",
  origin: "produced",
  pinned: false,
  missing: false,
  path: "/home/.openalpaca/artifacts/loose/2026-09-05/findings.md",
  project_root: null,
  rel_path: "loose/2026-09-05/findings.md",
  in_project: false,
  openable: true,
};

beforeEach(() => {
  requests = [];
  resetConnection();
  reply = () => json({ artifacts: [row], total: 1 });
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: unknown, init?: RequestInit) => {
      const url = String(input);
      requests.push({
        url,
        method: init?.method ?? "GET",
        body: typeof init?.body === "string" ? JSON.parse(init.body) : null,
      });
      return reply(url);
    }),
  );
});

describe("listArtifacts", () => {
  it("sends the §4.9 query string and returns the page envelope", async () => {
    const page = await listArtifacts({
      taskId: "task-1",
      kind: "markdown",
      pinned: true,
      q: "find ings",
      limit: 50,
      offset: 100,
    });

    expect(page.total).toBe(1);
    expect(page.artifacts[0]?.id).toBe("art-1");

    const url = new URL(requests[0]?.url ?? "");
    expect(url.pathname).toBe("/v1/artifacts");
    expect(url.searchParams.get("task_id")).toBe("task-1");
    expect(url.searchParams.get("kind")).toBe("markdown");
    expect(url.searchParams.get("pinned")).toBe("true");
    expect(url.searchParams.get("q")).toBe("find ings");
    expect(url.searchParams.get("limit")).toBe("50");
    expect(url.searchParams.get("offset")).toBe("100");
  });

  it("omits every filter it was not given", async () => {
    await listArtifacts();
    const url = new URL(requests[0]?.url ?? "");
    expect(url.search).toBe("");
  });

  it("asks for missing rows only when told to", async () => {
    await listArtifacts({ includeMissing: true });
    const url = new URL(requests[0]?.url ?? "");
    expect(url.searchParams.get("include_missing")).toBe("true");
  });

  it("asks for one side of the Library when told to", async () => {
    await listArtifacts({ inProject: false });
    await listArtifacts({ inProject: true, taskId: "task-1", limit: 500 });

    const byProducts = new URL(requests[0]?.url ?? "");
    const results = new URL(requests[1]?.url ?? "");
    expect(byProducts.searchParams.get("in_project")).toBe("false");
    expect(results.searchParams.get("in_project")).toBe("true");
    expect(results.searchParams.get("task_id")).toBe("task-1");
    expect(results.searchParams.get("limit")).toBe("500");
  });
});

describe("getArtifact", () => {
  it("reads one row and carries the superset fields", async () => {
    reply = () => json(row);
    const artifact = await getArtifact("art-1");
    expect(artifact.rel_path).toBe("loose/2026-09-05/findings.md");
    expect(artifact.missing).toBe(false);
    expect(requests[0]?.url).toContain("/v1/artifacts/art-1");
  });

  it("carries the row's side and whether Open is offered", async () => {
    reply = () =>
      json({
        ...row,
        name: "scripts/release.sh",
        rel_path: "scripts/release.sh",
        in_project: true,
        openable: false,
      });
    const artifact = await getArtifact("art-1");
    expect(artifact.in_project).toBe(true);
    expect(artifact.openable).toBe(false);
  });

  it("surfaces a 404 as an ApiError the caller can branch on", async () => {
    reply = () =>
      json(
        { error: { code: "ARTIFACT_NOT_FOUND", message: "no such artifact" } },
        404,
      );
    await expect(getArtifact("ghost")).rejects.toMatchObject({
      status: 404,
      code: "ARTIFACT_NOT_FOUND",
    });
  });
});

describe("a row from a daemon older than result files (d7-app-4)", () => {
  /** What a pre-S4a `GET /v1/artifacts` serves: no `in_project`, no `openable`. */
  const { in_project: _side, openable: _open, ...older } = row;

  it("is read as a by-product that opens, in a list", async () => {
    reply = () => json({ artifacts: [older], total: 1 });

    const page = await listArtifacts();

    expect(page.artifacts[0]).toMatchObject({
      in_project: false,
      openable: true,
    });
  });

  it("is read as a by-product that opens, on its own", async () => {
    reply = () => json(older);

    await expect(getArtifact("art-1")).resolves.toMatchObject({
      in_project: false,
      openable: true,
    });
  });

  it("keeps what a newer daemon says", async () => {
    const script = { ...row, in_project: true, openable: false };
    reply = (url) =>
      url.includes("/v1/artifacts?") || url.endsWith("/v1/artifacts")
        ? json({ artifacts: [script], total: 1 })
        : json(script);

    expect((await listArtifacts()).artifacts[0]).toMatchObject({
      in_project: true,
      openable: false,
    });
    await expect(getArtifact("art-1")).resolves.toMatchObject({
      in_project: true,
      openable: false,
    });
  });
});

describe("listArtifactVersions", () => {
  it("unwraps the `{ versions }` envelope", async () => {
    reply = () =>
      json({
        versions: [
          {
            version: 2,
            note: "",
            author_agent_id: null,
            created_at: "2026-09-05T10:05:00Z",
            size_bytes: 8,
            added_lines: 1,
            removed_lines: 0,
          },
        ],
      });
    const versions = await listArtifactVersions("art-1");
    expect(versions).toHaveLength(1);
    expect(versions[0]?.version).toBe(2);
    expect(requests[0]?.url).toContain("/v1/artifacts/art-1/versions");
  });
});

describe("getArtifactDiff", () => {
  it("asks for the pair and returns the unified patch", async () => {
    reply = () =>
      json({
        from: 1,
        to: 2,
        added_lines: 1,
        removed_lines: 1,
        format: "unified",
        patch: "--- v1\n+++ v2\n@@ -1 +1 @@\n-two\n+four\n",
      });
    const diff = await getArtifactDiff("art-1", 1, 2);
    expect(diff.patch).toContain("+four");

    const url = new URL(requests[0]?.url ?? "");
    expect(url.pathname).toBe("/v1/artifacts/art-1/diff");
    expect(url.searchParams.get("from")).toBe("1");
    expect(url.searchParams.get("to")).toBe("2");
  });

  it("keeps the 409 code so the tab can say which refusal it was", async () => {
    reply = () =>
      json(
        {
          error: {
            code: "DIFF_TOO_LARGE",
            message: "v2 is 9 MiB; the diff cap is 8 MiB",
          },
        },
        409,
      );
    const error = await getArtifactDiff("art-1", 1, 2).catch(
      (cause: unknown) => cause,
    );
    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).code).toBe("DIFF_TOO_LARGE");
    expect((error as ApiError).message).toContain("8 MiB");
  });
});

describe("setArtifactPinned", () => {
  it("PUTs the pin and returns what the server decided", async () => {
    reply = () => json({ id: "art-1", pinned: true });
    const result = await setArtifactPinned("art-1", true);
    expect(result).toEqual({ id: "art-1", pinned: true });
    expect(requests[0]?.method).toBe("PUT");
    expect(requests[0]?.url).toContain("/v1/artifacts/art-1/pin");
    expect(requests[0]?.body).toEqual({ pinned: true });
  });
});

describe("content URLs", () => {
  it("has no URL before the daemon connection is known", () => {
    expect(artifactContentUrl("art-1")).toBeNull();
    expect(fileContentUrl("file-1")).toBeNull();
  });

  it("builds a loadable `?token=` URL for an artifact and a file", async () => {
    await bootstrapConnection();

    expect(artifactContentUrl("art-1")).toBe(
      "http://127.0.0.1:9999/v1/artifacts/art-1/content?token=tok%20en%2F%2B",
    );
    expect(artifactContentUrl("art-1", 1)).toBe(
      "http://127.0.0.1:9999/v1/artifacts/art-1/versions/1/content?token=tok%20en%2F%2B",
    );
    expect(fileContentUrl("file-1")).toBe(
      "http://127.0.0.1:9999/v1/files/file-1/content?token=tok%20en%2F%2B",
    );
  });

  it("escapes an id that would otherwise break out of its path segment", async () => {
    await bootstrapConnection();
    expect(artifactContentUrl("a/b")).toContain("/v1/artifacts/a%2Fb/content");
  });
});
