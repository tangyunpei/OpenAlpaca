import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { MemoryUsedNote, usedReasonLabel } from "./MemoryUsedNote";

describe("MemoryUsedNote (design §5 step 8; design 09 §13.2)", () => {
  it("names each reason in the reader's words", () => {
    expect(usedReasonLabel("recall")).toBe("recalled");
    expect(usedReasonLabel("pinned")).toBe("pinned");
    expect(usedReasonLabel("tool")).toBe("looked up");
    // Index-first recall: the note was listed for the model, not delivered.
    expect(usedReasonLabel("index")).toBe("listed");
    // A reason this window does not know is printed as the daemon spelled it.
    expect(usedReasonLabel("something_new")).toBe("something_new");
  });

  it("marks a listed note as listed", () => {
    render(
      <MemoryUsedNote
        items={[
          {
            id: "00000000-0000-4000-8000-00000000a002",
            title: "Editor preference",
            reason: "recall",
          },
          {
            id: "00000000-0000-4000-8000-00000000a003",
            title: "Editor theme",
            reason: "index",
          },
        ]}
        onOpen={() => {}}
      />,
    );
    expect(
      screen.getByText("Editor preference").parentElement,
    ).toHaveTextContent("Editor preference — recalled");
    expect(screen.getByText("Editor theme").parentElement).toHaveTextContent(
      "Editor theme — listed",
    );
  });

  it("keeps a note listed and then looked up as two rows (L10b review 4)", () => {
    // Index mode's own flow: the recall lists a note, the model opens it with
    // memory_read, and `done.memory_used` names the same id twice.
    const id = "00000000-0000-4000-8000-00000000a003";
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      render(
        <MemoryUsedNote
          items={[
            { id, title: "Editor theme", reason: "index" },
            { id, title: "Editor theme", reason: "tool" },
          ]}
          onOpen={() => {}}
        />,
      );
      const rows = screen.getAllByRole("listitem");
      expect(rows).toHaveLength(2);
      expect(rows[0]).toHaveTextContent("Editor theme — listed");
      expect(rows[1]).toHaveTextContent("Editor theme — looked up");
      const keyWarnings = errors.mock.calls.filter((args) =>
        args.some((arg) => String(arg).includes("same key")),
      );
      expect(keyWarnings).toEqual([]);
    } finally {
      errors.mockRestore();
    }
  });
});
