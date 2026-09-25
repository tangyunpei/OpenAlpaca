import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

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
});
