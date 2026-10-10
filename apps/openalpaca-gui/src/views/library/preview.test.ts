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
