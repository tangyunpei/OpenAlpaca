/**
 * The Preview tab's sentences when it draws nothing. The button that opens a
 * file in its own app is `Open` (Round G, G4), so the notes name it — never
 * `Reveal`, which now means showing the file in its folder.
 */

import { describe, expect, it } from "vitest";

import type { Artifact } from "@/lib/api/artifacts";
import { ApiError } from "@/lib/http";

import { MAX_PREVIEW_BYTES, previewNote, previewPlan } from "./preview";

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

  it("reads a project file marked missing again, so a file put back is shown (d7-app-1)", () => {
    expect(previewPlan({ ...written, missing: true })).toEqual({
      mode: "text",
      kind: "md",
      note: null,
    });
  });

  it("says a marked project file it cannot read could not be read last time, never that it is gone nor that Export or Open opens it (Y.4)", () => {
    expect(
      previewPlan({
        ...written,
        kind: "binary",
        mime_type: "application/octet-stream",
        size_bytes: 2048,
        missing: true,
      }),
    ).toEqual({
      mode: "none",
      note: "This file could not be read the last time it was asked for: it was not at its path in the project, or the project's folder is gone or has moved. Binary file, 2.0 KB.",
    });
  });

  it("says a marked project file too large to preview could not be read last time, sending it to neither Export nor Open (Y.4)", () => {
    for (const openable of [true, false]) {
      expect(
        previewPlan({
          ...written,
          size_bytes: MAX_PREVIEW_BYTES + 1,
          missing: true,
          openable,
        }),
      ).toEqual({
        mode: "none",
        note: "This file could not be read the last time it was asked for: it was not at its path in the project, or the project's folder is gone or has moved. Too large to preview here (512.0 KB).",
      });
    }
  });

  it("loads a marked project image again, with a note for a failed load that names neither Export nor Open (X14)", () => {
    const diagram: Artifact = {
      ...written,
      name: "docs/diagram.png",
      rel_path: "docs/diagram.png",
      kind: "image",
      mime_type: "image/png",
    };
    expect(previewPlan({ ...diagram, missing: true })).toEqual({
      mode: "image",
      failedNote:
        "This file could not be read the last time it was asked for: it was not at its path in the project, or the project's folder is gone or has moved. Its row is kept, and the image is loaded again the next time this preview is opened.",
    });
    // An image nobody marked keeps the placeholder's own sentence.
    expect(previewPlan(diagram)).toEqual({ mode: "image", failedNote: null });
  });

  it("still says a missing by-product is no longer on disk", () => {
    expect(previewPlan({ ...notes, missing: true })).toEqual({
      mode: "none",
      note: "This file is no longer on disk. The daemon kept its record, so the history below is still real.",
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

describe("previewNote — what a failed read of a project file says", () => {
  const written: Artifact = {
    ...notes,
    name: "docs/notes.md",
    path: "/Users/dev/app/docs/notes.md",
    project_root: "/Users/dev/app",
    rel_path: "docs/notes.md",
    version_count: 0,
    in_project: true,
    missing: true,
  };
  const textPlan = { mode: "text", kind: "md", note: null } as const;

  it("says the file is no longer at its path when the read answers 410 ARTIFACT_GONE", () => {
    const error = new ApiError(
      "artifact art-1 is gone: /Users/dev/app/docs/notes.md no longer exists",
      410,
      "ARTIFACT_GONE",
    );
    expect(previewNote(written, textPlan, { error, isLoading: false })).toBe(
      "This file is no longer at docs/notes.md in the project. Its row is kept, and the file is shown again once it is back.",
    );
  });

  it("shows the hidden characters an agent put in a missing file's path, with their count", () => {
    const path = "docs/\u{202E}notes\u{200B}.md";
    const error = new ApiError("gone", 410, "ARTIFACT_GONE");
    expect(
      previewNote({ ...written, name: path, rel_path: path }, textPlan, {
        error,
        isLoading: false,
      }),
    ).toBe(
      "This file is no longer at docs/<U+202E>notes<U+200B>.md in the project. Its row is kept, and the file is shown again once it is back. Note: this request contains 2 hidden characters, shown as <U+…>.",
    );
  });

  it("shows the daemon's sentence for a project whose folder is gone or has moved (R-T26)", () => {
    const sentence =
      "/Users/dev/app: this project's folder is gone or has moved; re-point the project to read its files.";
    const error = new ApiError(sentence, 410, "ARTIFACT_PROJECT_MISSING");
    expect(previewNote(written, textPlan, { error, isLoading: false })).toBe(
      sentence,
    );
  });

  it("draws any other refusal escaped, never raw", () => {
    const error = new ApiError(
      "artifact art-1 is gone: /Users/dev/app/\u{202E}x\u{200B}.md no longer exists",
      410,
      "ARTIFACT_GONE",
    );
    expect(
      previewNote({ ...notes, missing: true }, textPlan, {
        error,
        isLoading: false,
      }),
    ).toBe(
      "artifact art-1 is gone: /Users/dev/app/<U+202E>x<U+200B>.md no longer exists",
    );
  });

  it("says it is reading while the read is in flight, then the plan's own note", () => {
    expect(
      previewNote(written, textPlan, { error: null, isLoading: true }),
    ).toBe("Reading the file…");
    expect(
      previewNote(
        written,
        { mode: "text", kind: "code", note: "Shown as source." },
        { error: null, isLoading: false },
      ),
    ).toBe("Shown as source.");
    expect(
      previewNote(
        written,
        { mode: "none", note: "Export saves it." },
        { error: null, isLoading: false },
      ),
    ).toBe("Export saves it.");
  });
});
