/**
 * The Library header's file actions — `Open` always, `Reveal in Finder` only
 * where a Tauri shell can answer it — and the summary line under the name.
 */

import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Artifact } from "@/lib/api/artifacts";
import { revealLabel } from "@/lib/reveal";

import { LibraryDetailHeader } from "./LibraryDetailHeader";

const artifact: Artifact = {
  id: "art-1",
  name: "connector-audit-findings.md",
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
  created_at: "2026-09-05T10:00:00Z",
  updated_at: "2026-09-05T10:05:00Z",
  origin: "produced",
  pinned: false,
  missing: false,
  path: "/home/.openalpaca/artifacts/loose/2026-09-05/01-connector-audit-findings.md",
  project_root: null,
  rel_path: "loose/2026-09-05/01-connector-audit-findings.md",
  in_project: false,
  openable: true,
};

function renderHeader(summary: string | null = null) {
  const handlers = {
    onTabChange: vi.fn(),
    onTogglePin: vi.fn(),
    onExport: vi.fn(),
    onOpen: vi.fn(),
    onReveal: vi.fn(),
  };
  render(
    <LibraryDetailHeader
      artifact={{ ...artifact, summary }}
      pinned={false}
      tab="preview"
      {...handlers}
    />,
  );
  return handlers;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("LibraryDetailHeader — Open and Reveal", () => {
  it("draws Open, and no Reveal, outside the app's window", async () => {
    const handlers = renderHeader();

    await userEvent.setup().click(screen.getByRole("button", { name: "Open" }));

    expect(handlers.onOpen).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("button", { name: "Reveal" })).toBeNull();
    expect(
      screen.queryByRole("button", { name: revealLabel() }),
    ).not.toBeInTheDocument();
  });

  it("draws Reveal beside Open inside the app's window", async () => {
    vi.stubGlobal("isTauri", true);
    const handlers = renderHeader();

    await userEvent
      .setup()
      .click(screen.getByRole("button", { name: revealLabel() }));

    expect(handlers.onReveal).toHaveBeenCalledTimes(1);
    expect(handlers.onOpen).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Open" })).toBeInTheDocument();
  });
});

describe("LibraryDetailHeader — the summary line", () => {
  it("draws the summary on one line under the name, whole on its tooltip", () => {
    renderHeader("notes/tea.md");

    const line = screen.getByText("notes/tea.md");
    expect(line).toHaveAttribute("title", "notes/tea.md");
    expect(line).toHaveClass("truncate");
    expect(screen.getByRole("heading", { level: 2 }).nextElementSibling).toBe(
      line,
    );
  });

  it.each([
    ["null", null],
    ["blank", "  "],
  ])("draws no line for a %s summary", (_label, summary) => {
    renderHeader(summary);

    const next = screen.getByRole("heading", { level: 2 }).nextElementSibling;
    expect(next).toHaveTextContent("v1 of 1");
  });
});

describe("LibraryDetailHeader — a file written into a project", () => {
  const written: Artifact = {
    ...artifact,
    name: "scripts/release.sh",
    kind: "code",
    mime_type: "text/plain",
    path: "/Users/dev/app/scripts/release.sh",
    project_root: "/Users/dev/app",
    rel_path: "scripts/release.sh",
    version: 1,
    version_count: 0,
    in_project: true,
    openable: false,
  };

  function renderWritten(patch: Partial<Artifact> = {}) {
    render(
      <LibraryDetailHeader
        artifact={{ ...written, ...patch }}
        pinned={false}
        tab="preview"
        onTabChange={vi.fn()}
        onTogglePin={vi.fn()}
        onExport={vi.fn()}
        onOpen={vi.fn()}
        onReveal={vi.fn()}
      />,
    );
  }

  it("offers Preview only, with no Diff or History", () => {
    renderWritten();
    expect(screen.getByRole("tab", { name: "Preview" })).toBeInTheDocument();
    expect(screen.queryByRole("tab", { name: "Diff" })).toBeNull();
    expect(screen.queryByRole("tab", { name: "History" })).toBeNull();
  });

  it("names its project where a version would be", () => {
    renderWritten();
    expect(screen.queryByText(/v1 of 0/)).toBeNull();
    expect(screen.getByText("project")).toBeInTheDocument();
    expect(screen.getByText("app")).toHaveAttribute("title", "/Users/dev/app");
  });

  it("draws Reveal and no Open for a kind the daemon does not open", () => {
    vi.stubGlobal("isTauri", true);
    renderWritten();
    expect(screen.queryByRole("button", { name: "Open" })).toBeNull();
    expect(
      screen.getByRole("button", { name: revealLabel() }),
    ).toBeInTheDocument();
  });

  it("draws Open for a project file of a kind the store keeps", () => {
    renderWritten({ name: "docs/notes.md", kind: "markdown", openable: true });
    expect(screen.getByRole("button", { name: "Open" })).toBeInTheDocument();
  });

  it("shows the hidden characters an agent put in the path, with their count", () => {
    const name = "scripts/\u{202E}release\u{200B}.sh";
    renderWritten({ name, rel_path: name });

    const heading = screen.getByRole("heading", { level: 2 });
    expect(heading).toHaveTextContent("scripts/<U+202E>release<U+200B>.sh");
    expect(
      Array.from(heading.querySelectorAll("[data-hidden-char]"), (element) =>
        element.getAttribute("data-hidden-char"),
      ),
    ).toEqual(["U+202E", "U+200B"]);
    // The count sits under the name, before the meta row.
    expect(heading.nextElementSibling).toHaveTextContent(
      "Note: this request contains 2 hidden characters, shown as <U+…>.",
    );
  });
});

describe("LibraryDetailHeader — hidden characters in the summary", () => {
  it("escapes the summary, on its tooltip too, and counts it with the name", () => {
    renderHeader("notes/\u{202E}tea.md");

    const line = screen.getByText("tea.md", { exact: false });
    expect(line).toHaveTextContent("notes/<U+202E>tea.md");
    expect(line).toHaveAttribute("title", "notes/<U+202E>tea.md");
    // An ellipsis could cut off an escape the count says is there.
    expect(line).toHaveClass("wrap-anywhere");
    expect(line).not.toHaveClass("truncate");
    expect(line.nextElementSibling).toHaveTextContent(
      "Note: this request contains 1 hidden character, shown as <U+…>.",
    );
  });

  it("draws no hidden-character note for a plain name and summary", () => {
    renderHeader("notes/tea.md");

    expect(document.querySelector("[data-hidden-char]")).toBeNull();
    expect(screen.queryByText(/hidden character/)).toBeNull();
  });
});
