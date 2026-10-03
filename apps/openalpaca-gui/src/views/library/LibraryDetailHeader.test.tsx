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
