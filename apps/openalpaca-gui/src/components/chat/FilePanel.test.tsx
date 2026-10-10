/**
 * The file panel's file actions — the Work pane's artifact preview: `Open`
 * when the caller gives it a handler, `Reveal in Finder` only inside the app's
 * window as well.
 */

import { render, screen, within } from "@testing-library/react";
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

describe("FilePanel — the picker's names", () => {
  it("shows the hidden characters an agent put in a listed path, with their count", () => {
    // A result file's name is its path in the project, which the agent chose.
    renderPanel({
      pickerOpen: true,
      pickerItems: [
        { id: "art-2", name: "scripts/‮release​.sh", kind: "code" },
      ],
    });

    const option = screen.getByRole("option", { name: /release/ });
    const escapes = Array.from(
      option.querySelectorAll("[data-hidden-char]"),
      (element) => element.getAttribute("data-hidden-char"),
    );
    expect(escapes).toEqual(["U+202E", "U+200B"]);
    const shown = within(option).getByText("release", { exact: false });
    expect(shown).toHaveTextContent("scripts/<U+202E>release<U+200B>.sh");
    expect(shown).toHaveClass("wrap-anywhere");
    expect(shown).not.toHaveClass("truncate");
    expect(
      within(option).getByText(
        "Note: this request contains 2 hidden characters, shown as <U+…>.",
      ),
    ).toBeInTheDocument();
  });

  it("draws no hidden-character note for a plain listed path", () => {
    renderPanel({
      pickerOpen: true,
      pickerItems: [{ id: "art-2", name: "scripts/release.sh", kind: "code" }],
    });

    const option = screen.getByRole("option", { name: /release/ });
    expect(option.querySelector("[data-hidden-char]")).toBeNull();
    expect(within(option).queryByText(/hidden character/)).toBeNull();
    expect(within(option).getByText("scripts/release.sh")).toHaveClass(
      "truncate",
    );
  });
});
