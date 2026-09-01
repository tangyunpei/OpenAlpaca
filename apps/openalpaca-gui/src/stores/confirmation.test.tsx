import { render } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  useConfirmationStore,
  usePublishConfirmation,
  type PendingToolConfirmation,
} from "./confirmation";

function Publisher({ pending }: { pending: PendingToolConfirmation | null }) {
  usePublishConfirmation(pending);
  return null;
}

const approve = vi.fn();
const deny = vi.fn();

const confirmation: PendingToolConfirmation = {
  toolName: "shell_execute",
  runId: "b41c8e02",
  approve,
  deny,
};

beforeEach(() => {
  useConfirmationStore.setState({ pending: null });
});

describe("usePublishConfirmation", () => {
  it("publishes while blocked and clears when the block resolves", () => {
    const { rerender } = render(<Publisher pending={confirmation} />);
    expect(useConfirmationStore.getState().pending).toMatchObject({
      toolName: "shell_execute",
      runId: "b41c8e02",
    });

    rerender(<Publisher pending={null} />);
    expect(useConfirmationStore.getState().pending).toBeNull();
  });

  it("clears the slot when the publisher unmounts", () => {
    const { unmount } = render(<Publisher pending={confirmation} />);
    expect(useConfirmationStore.getState().pending).not.toBeNull();

    unmount();
    expect(useConfirmationStore.getState().pending).toBeNull();
  });

  it("keeps a null run id rather than guessing one (GAP-07)", () => {
    render(<Publisher pending={{ ...confirmation, runId: null }} />);
    expect(useConfirmationStore.getState().pending?.runId).toBeNull();
  });

  it("does not republish an unchanged confirmation on re-render", () => {
    const setPending = vi.spyOn(useConfirmationStore.getState(), "setPending");
    const { rerender } = render(<Publisher pending={{ ...confirmation }} />);
    const calls = setPending.mock.calls.length;

    rerender(<Publisher pending={{ ...confirmation }} />);
    expect(setPending.mock.calls.length).toBe(calls);
    setPending.mockRestore();
  });
});
