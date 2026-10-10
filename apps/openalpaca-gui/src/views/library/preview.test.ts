/**
 * The Preview tab's sentences when it draws nothing. The button that opens a
 * file in its own app is `Open` (Round G, G4), so the notes name it — never
 * `Reveal`, which now means showing the file in its folder.
 */

import { describe, expect, it } from "vitest";

import type { Artifact } from "@/lib/api/artifacts";

import { MAX_PREVIEW_BYTES, previewPlan } from "./preview";

const notes: Artifact = {
  id: "art-1",
  name: "01-notes.md",
  kind: "markdown",
  mime_type: "text/markdown",
  size_bytes: 42,
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
  path: "/home/.openalpaca/artifacts/loose/2026-10-02/01-notes.md",
  project_root: null,
  rel_path: "loose/2026-10-02/01-notes.md",
  in_project: false,
  openable: true,
};

describe("previewPlan — the notes name Open", () => {
  it("sends a binary file to Export or Open", () => {
    const plan = previewPlan({
      ...notes,
      name: "02-model.bin",
      kind: "binary",
      mime_type: "application/octet-stream",
      size_bytes: 2048,
    });

    expect(plan).toEqual({
      mode: "none",
      note: "Binary file, 2.0 KB — Export or Open opens it in its own app.",
    });
  });

  it("sends a file too large to preview to Export or Open", () => {
    const plan = previewPlan({ ...notes, size_bytes: MAX_PREVIEW_BYTES + 1 });

    expect(plan).toEqual({
      mode: "none",
      note: "Too large to preview here (512.0 KB) — Export or Open opens the whole file.",
    });
  });
});

describe("previewPlan — a file written into the project", () => {
  const written: Artifact = {
    ...notes,
    name: "docs/notes.md",
    path: "/Users/dev/app/docs/notes.md",
    project_root: "/Users/dev/app",
    rel_path: "docs/notes.md",
    version_count: 0,
    in_project: true,
  };

  it("says a missing project file is no longer at its path in the project", () => {
    expect(previewPlan({ ...written, missing: true })).toEqual({
      mode: "none",
      note: "This file is no longer at docs/notes.md in the project. Its row is kept, and the file is shown again once it is back.",
    });
  });

  it("shows the hidden characters an agent put in a missing file's path, with their count", () => {
    const path = "docs/‮notes​.md";
    expect(
      previewPlan({ ...written, name: path, rel_path: path, missing: true }),
    ).toEqual({
      mode: "none",
      note: "This file is no longer at docs/<U+202E>notes<U+200B>.md in the project. Its row is kept, and the file is shown again once it is back. Note: this request contains 2 hidden characters, shown as <U+…>.",
    });
  });

  it("sends a file too large to preview to Export alone when Open is not offered", () => {
    expect(
      previewPlan({
        ...written,
        name: "bin/tool.sh",
        rel_path: "bin/tool.sh",
        kind: "code",
        size_bytes: MAX_PREVIEW_BYTES + 1,
        openable: false,
      }),
    ).toEqual({
      mode: "none",
      note: "Too large to preview here (512.0 KB) — Export saves the whole file.",
    });
  });
});
