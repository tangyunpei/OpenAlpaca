import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { Artifact } from "@/lib/api/artifacts";

import { LibraryRow, artifactSubtitle } from "./LibraryRow";

/**
 * One row of `GET /v1/artifacts`, spelled out here so the subtitle rules can be
 * asserted without a daemon. `LibraryView.test.tsx` proves the same component
 * against real response bodies.
 */
const artifact: Artifact = {
  id: "findings",
  name: "connector-audit-findings.md",
  kind: "markdown",
  mime_type: "text/markdown",
  size_bytes: 4096,
  task_id: "b41c8e02",
  task_title: "connector audit",
  agent_id: "agent-1",
  agent_template_id: "review_agent",
  version: 2,
  version_count: 2,
  summary: null,
  metadata: null,
  created_at: "2026-08-31T12:00:00Z",
  updated_at: "2026-08-31T12:00:00Z",
  origin: "produced",
  pinned: false,
  missing: false,
  path: "/home/.openalpaca/artifacts/2026-08-31-run/01-connector-audit-findings.md",
  project_root: null,
  rel_path: "2026-08-31-run/01-connector-audit-findings.md",
  in_project: false,
  openable: true,
};

describe("LibraryRow (§3.30)", () => {
  it("builds the design's `agent · run · when` subtitle", () => {
    const now = new Date("2026-08-31T12:02:00Z");
    expect(artifactSubtitle(artifact, now)).toBe(
      "review_agent · connector audit · 2m ago",
    );
  });

  it("drops missing attribution rather than filling it in", () => {
    const now = new Date("2026-08-31T12:00:30Z");
    const orphan: Artifact = {
      ...artifact,
      agent_id: null,
      agent_template_id: null,
      task_title: null,
    };
    expect(artifactSubtitle(orphan, now)).toBe("just now");
  });

  it("selects by id and marks only a pinned row", () => {
    const onSelect = vi.fn();
    const { rerender } = render(
      <LibraryRow
        artifact={artifact}
        active={false}
        pinned={false}
        onSelect={onSelect}
      />,
    );
    expect(screen.queryByLabelText("Pinned")).toBeNull();

    screen.getByRole("button").click();
    expect(onSelect).toHaveBeenCalledWith("findings");

    rerender(
      <LibraryRow artifact={artifact} active pinned onSelect={onSelect} />,
    );
    expect(screen.getByLabelText("Pinned")).toBeInTheDocument();
  });
});

describe("LibraryRow — a file written into a project (storage design §13)", () => {
  const written: Artifact = {
    ...artifact,
    id: "written",
    name: "src/main.rs",
    kind: "code",
    mime_type: "text/plain",
    path: "/Users/dev/app/src/main.rs",
    project_root: "/Users/dev/app",
    rel_path: "src/main.rs",
    version: 1,
    version_count: 0,
    in_project: true,
    openable: true,
  };

  it("names the file by its path in the project, with a project tag", () => {
    render(
      <LibraryRow
        artifact={written}
        active={false}
        pinned={false}
        onSelect={vi.fn()}
      />,
    );
    expect(screen.getByText("src/main.rs")).toBeInTheDocument();
    expect(screen.getByText("project")).toBeInTheDocument();
  });

  it("names the project first in the subtitle", () => {
    const now = new Date("2026-08-31T12:02:00Z");
    expect(artifactSubtitle(written, now)).toBe(
      "app · review_agent · connector audit · 2m ago",
    );
  });

  it("draws no project tag on a by-product", () => {
    render(
      <LibraryRow
        artifact={artifact}
        active={false}
        pinned={false}
        onSelect={vi.fn()}
      />,
    );
    expect(screen.queryByText("project")).not.toBeInTheDocument();
  });

  it("shows the hidden characters an agent put in the path, with their count", () => {
    // The path is the agent's: a bidi override and a zero-width space in it
    // would make the row read as a different file.
    const name = "src/‮evil​.rs";
    render(
      <LibraryRow
        artifact={{ ...written, name, rel_path: name }}
        active={false}
        pinned={false}
        onSelect={vi.fn()}
      />,
    );

    const escapes = Array.from(
      document.querySelectorAll("[data-hidden-char]"),
      (element) => element.getAttribute("data-hidden-char"),
    );
    expect(escapes).toEqual(["U+202E", "U+200B"]);
    const shown = screen.getByText("evil", { exact: false });
    expect(shown).toHaveTextContent("src/<U+202E>evil<U+200B>.rs");
    // An ellipsis could cut off an escape the count says is there.
    expect(shown).toHaveClass("wrap-anywhere");
    expect(shown).not.toHaveClass("truncate");
    expect(
      screen.getByText(
        "Note: this request contains 2 hidden characters, shown as <U+…>.",
      ),
    ).toBeInTheDocument();
  });

  it("draws no hidden-character note for a plain path", () => {
    render(
      <LibraryRow
        artifact={written}
        active={false}
        pinned={false}
        onSelect={vi.fn()}
      />,
    );
    expect(document.querySelector("[data-hidden-char]")).toBeNull();
    expect(screen.queryByText(/hidden character/)).toBeNull();
    expect(screen.getByText("src/main.rs")).toHaveClass("truncate");
  });
});
