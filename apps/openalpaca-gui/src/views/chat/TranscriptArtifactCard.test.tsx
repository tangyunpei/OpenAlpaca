/**
 * The inline card an `artifact_written` frame produces, through the transcript
 * that orders it.
 */

import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";

import { useUiStore } from "@/stores/ui";

import { Transcript } from "./Transcript";
import { buildTranscript, type WrittenArtifact } from "./transcript-model";
import { initialChatStreamState } from "@/lib/chat-stream";

const written: WrittenArtifact = {
  artifactId: "art-1",
  name: "connector-audit-findings.md",
  kind: "markdown",
  version: 2,
  taskId: "task-1",
  at: "2026-09-05T13:41:00Z",
};

function items(...artifacts: WrittenArtifact[]) {
  return buildTranscript({
    history: [],
    reports: [],
    artifacts,
    confirmations: [],
    resolutions: [],
    steers: [],
    stream: initialChatStreamState,
    pending: null,
  });
}

beforeEach(() => {
  useUiStore.setState({ view: "chat", openArtifactId: null });
});

describe("the written-artifact card", () => {
  it("names the file, its kind badge and the version the frame carried", () => {
    render(<Transcript items={items(written)} dense={false} />);

    expect(screen.getByText("connector-audit-findings.md")).toBeInTheDocument();
    expect(screen.getByText("v2")).toBeInTheDocument();
    expect(screen.getByText(/File written/)).toBeInTheDocument();
    // `markdown` is the daemon's spelling; the badge is the design's `MD`.
    expect(screen.getByText("MD")).toBeInTheDocument();
  });

  it("opens the file in the Library on the Preview tab", async () => {
    const user = userEvent.setup();
    render(<Transcript items={items(written)} dense={false} />);

    await user.click(screen.getByRole("button", { name: "Open in Library" }));

    const state = useUiStore.getState();
    expect(state.view).toBe("library");
    expect(state.openArtifactId).toBe("art-1");
    expect(state.libraryTab).toBe("preview");
  });

  it("shows a supersede as its own card rather than mutating the first", () => {
    render(
      <Transcript
        items={items(written, {
          ...written,
          version: 3,
          at: "2026-09-05T13:44:00Z",
        })}
        dense={false}
      />,
    );
    expect(screen.getAllByText("connector-audit-findings.md")).toHaveLength(2);
    expect(screen.getByText("v2")).toBeInTheDocument();
    expect(screen.getByText("v3")).toBeInTheDocument();
  });
});
