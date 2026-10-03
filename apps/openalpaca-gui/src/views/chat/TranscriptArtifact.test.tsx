/**
 * A file a turn carried reads the same badge in the transcript as in the
 * Library (G3a, Round G). The Library badges the kind the daemon stored —
 * for an upload, `ArtifactKind::for_mime` of its type — and a transcript chip
 * read from `content_json` carries the type but not the kind, so the card
 * derives the kind by that same projection instead of guessing its own.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import type { Artifact, ArtifactKind } from "@/lib/api/artifacts";

import { LibraryRow } from "../library/LibraryRow";
import { TranscriptArtifact } from "./TranscriptArtifact";

afterEach(cleanup);

/** The text of the first badge a render drew. */
function badge(container: HTMLElement): string | null {
  return container.querySelector("span[aria-hidden]")?.textContent ?? null;
}

/** The card a history row's file part draws: name and type, no kind. */
function cardBadge(filename: string, mimeType: string): string | null {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const { container } = render(
    <QueryClientProvider client={client}>
      <TranscriptArtifact
        attachment={{ fileId: "file-1", filename, mimeType, kind: null }}
      />
    </QueryClientProvider>,
  );
  const text = badge(container);
  cleanup();
  return text;
}

/** The Library's row for the same upload, as `GET /v1/artifacts` serves it. */
function libraryBadge(
  name: string,
  mimeType: string,
  kind: ArtifactKind,
): string | null {
  const artifact: Artifact = {
    id: "file-1",
    name,
    kind,
    mime_type: mimeType,
    size_bytes: 16,
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
    origin: "upload",
    pinned: false,
    missing: false,
    path: `/home/.openalpaca/uploads/${name}`,
    project_root: null,
    rel_path: name,
  };
  const { container } = render(
    <LibraryRow
      artifact={artifact}
      active={false}
      pinned={false}
      onSelect={() => {}}
    />,
  );
  const text = badge(container);
  cleanup();
  return text;
}

describe("a transcript file's badge is the Library's (G3a)", () => {
  // `kind` is what the upload writer stores for that type (R25).
  it.each([
    ["notes.txt", "text/plain", "code", "TXT"],
    ["findings.md", "text/markdown", "markdown", "MD"],
    ["build.weird", "text/x-weird", "code", "WEI"],
  ] as const)(
    "%s (%s, stored as %s) reads %s in both places",
    (name, mimeType, kind, expected) => {
      expect(libraryBadge(name, mimeType, kind)).toBe(expected);
      expect(cardBadge(name, mimeType)).toBe(expected);
    },
  );
});
