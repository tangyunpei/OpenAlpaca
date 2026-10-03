/**
 * The file panel's file actions — the Work pane's artifact preview: `Open`
 * when the caller gives it a handler, `Reveal in Finder` only inside the app's
 * window as well.
 */

import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { revealLabel } from "@/lib/reveal";

import { FilePanel, type FilePanelProps } from "./FilePanel";

function renderPanel(overrides: Partial<FilePanelProps> = {}) {
  const props: FilePanelProps = {
    artifact: {
      id: "art-1",
      name: "connector-audit-findings.md",
      kind: "md",
      version: 1,
    },
    artifactNote: null,
    tab: "preview",
    onTabChange: vi.fn(),
    pickerOpen: false,
    onTogglePicker: vi.fn(),
    onClosePicker: vi.fn(),
    pickerItems: [],
    pickerNote: null,
    onPickArtifact: vi.fn(),
    onBackToWork: vi.fn(),
    onClose: vi.fn(),
    onOpenInLibrary: vi.fn(),
    pinned: false,
    onTogglePin: vi.fn(),
    ...overrides,
  };
  render(<FilePanel {...props} />);
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("FilePanel — Open and Reveal", () => {
  it("draws neither before the slot has an artifact to hand them", () => {
    vi.stubGlobal("isTauri", true);
    renderPanel({ artifact: null, artifactNote: "Reading the file…" });

    expect(screen.queryByRole("button", { name: "Open" })).toBeNull();
    expect(screen.queryByRole("button", { name: revealLabel() })).toBeNull();
  });

  it("draws Open, and no Reveal outside the app's window", async () => {
    const onOpen = vi.fn();
    renderPanel({ onOpen, onReveal: vi.fn() });

    await userEvent.setup().click(screen.getByRole("button", { name: "Open" }));

    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("button", { name: revealLabel() })).toBeNull();
  });

  it("draws Reveal inside the app's window", async () => {
    vi.stubGlobal("isTauri", true);
    const onReveal = vi.fn();
    renderPanel({ onOpen: vi.fn(), onReveal });

    await userEvent
      .setup()
      .click(screen.getByRole("button", { name: revealLabel() }));

    expect(onReveal).toHaveBeenCalledTimes(1);
  });
});
