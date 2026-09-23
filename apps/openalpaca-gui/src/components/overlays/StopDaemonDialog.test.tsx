import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { DaemonBusyStatus } from "@/lib/api/types";

import {
  CONNECTOR_SILENCE_LINE,
  GENERIC_BUSY_LINE,
  StopDaemonDialog,
  busyLine,
  type StopDaemonDialogProps,
} from "./StopDaemonDialog";

function busy(overrides: Partial<DaemonBusyStatus> = {}): DaemonBusyStatus {
  return {
    running_tasks: 2,
    pending_confirmations: 1,
    connected_clients: 3,
    ...overrides,
  };
}

function renderDialog(overrides: Partial<StopDaemonDialogProps> = {}) {
  const props: StopDaemonDialogProps = {
    busy: busy(),
    connectorsRunning: false,
    reading: false,
    stopping: false,
    onCancel: vi.fn(),
    onConfirm: vi.fn(),
    ...overrides,
  };
  render(<StopDaemonDialog {...props} />);
  return props;
}

describe("busyLine", () => {
  it("names the non-zero counts, with singulars", () => {
    expect(busyLine(busy())).toBe(
      "Right now: 2 workflows running · 1 tool waiting for approval · 3 windows connected.",
    );
    expect(
      busyLine(
        busy({
          running_tasks: 1,
          pending_confirmations: 0,
          connected_clients: 1,
        }),
      ),
    ).toBe("Right now: 1 workflow running · 1 window connected.");
  });

  it("is omitted when every count is zero", () => {
    expect(
      busyLine(
        busy({
          running_tasks: 0,
          pending_confirmations: 0,
          connected_clients: 0,
        }),
      ),
    ).toBeNull();
  });
});

describe("StopDaemonDialog", () => {
  it("renders the count line when the daemon served a busy block", () => {
    renderDialog();

    expect(screen.getByTestId("stop-daemon-busy")).toHaveTextContent(
      "Right now: 2 workflows running · 1 tool waiting for approval · 3 windows connected.",
    );
    expect(screen.queryByText(GENERIC_BUSY_LINE)).toBeNull();
  });

  /** An older daemon, or T23 declined: say what this window cannot know. */
  it("renders the generic line, and no count, when there is no busy block", () => {
    for (const absent of [null, undefined]) {
      const { unmount } = render(
        <StopDaemonDialog
          busy={absent}
          connectorsRunning={false}
          reading={false}
          stopping={false}
          onCancel={vi.fn()}
          onConfirm={vi.fn()}
        />,
      );
      expect(screen.getByTestId("stop-daemon-busy")).toHaveTextContent(
        GENERIC_BUSY_LINE,
      );
      expect(screen.queryByText(/Right now:/)).toBeNull();
      unmount();
    }
  });

  it("says what a stop costs, and promises nothing a stop does not do", () => {
    renderDialog();
    const dialog = screen.getByRole("dialog");

    expect(dialog).toHaveTextContent("Nothing is finished first.");
    expect(dialog).toHaveTextContent(/comes back marked .interrupted./);
    expect(dialog).toHaveTextContent("choose Rerun");
    expect(dialog).toHaveTextContent(
      "Anything you typed at a running workflow while it worked is kept",
    );
    expect(dialog).toHaveTextContent(
      "A tool waiting for your approval is dropped without running.",
    );
    expect(dialog.textContent ?? "").not.toMatch(
      /resume where|finishing current|saving your place/i,
    );
  });

  it("warns remote users' silence only when a connector is running (T32)", () => {
    const { unmount } = render(
      <StopDaemonDialog
        busy={busy()}
        connectorsRunning={false}
        reading={false}
        stopping={false}
        onCancel={vi.fn()}
        onConfirm={vi.fn()}
      />,
    );
    expect(screen.queryByText(CONNECTOR_SILENCE_LINE)).toBeNull();
    unmount();

    renderDialog({ connectorsRunning: true });
    expect(screen.getByText(CONNECTOR_SILENCE_LINE)).toBeInTheDocument();
  });

  /**
   * The gate (T33): no typed word, but `Stop daemon` cannot be pressed until
   * the warning it confirms is the current one, and it is never the default
   * focus.
   */
  it("keeps Stop disabled until the fresh read has settled", () => {
    renderDialog({ reading: true });

    expect(screen.getByRole("button", { name: "Stop daemon" })).toBeDisabled();
    expect(screen.getByTestId("stop-daemon-busy")).toHaveTextContent(
      "Checking what the daemon is doing…",
    );
  });

  it("enables Stop once confirmed readable, and stops only on the press", async () => {
    const user = userEvent.setup();
    const props = renderDialog();

    const stop = screen.getByRole("button", { name: "Stop daemon" });
    expect(stop).toBeEnabled();
    expect(props.onConfirm).not.toHaveBeenCalled();

    await user.click(stop);
    expect(props.onConfirm).toHaveBeenCalledTimes(1);
  });

  it("disables Stop while the stop is in flight", () => {
    renderDialog({ stopping: true });
    expect(screen.getByRole("button", { name: "Stopping…" })).toBeDisabled();
  });

  it("focuses Cancel, not Stop, and Escape cancels", () => {
    const props = renderDialog();

    expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus();
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(props.onCancel).toHaveBeenCalledTimes(1);
    expect(props.onConfirm).not.toHaveBeenCalled();
  });

  it("uses the existing danger ghost variant, not a new filled one (T25)", () => {
    renderDialog();
    const stop = screen.getByRole("button", { name: "Stop daemon" });
    expect(stop.className).toContain("text-red-ink");
    expect(stop.className).toContain("bg-transparent");
  });
});
