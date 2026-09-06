/**
 * The two terminal treatments (§3.19's card footer, §3.26's banner).
 *
 * `RunActionBar` has always disabled the action that is in flight; these two
 * did not, because before GAP-06 closed their only control was permanently
 * disabled anyway. Now `Re-run` is live, and every extra click is a real
 * `POST /v1/tasks/{id}/rerun` → a real `201` → a real lead agent and real
 * spend. There is no server-side idempotency for `rerun`, and there should not
 * be — two re-runs of one goal is a legitimate request — so the button is the
 * guard.
 */

import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { TerminalBanner, TerminalRunRow } from "./RunActionBar";
import { terminalRunActions } from "./run-actions";

/** The `Re-run` descriptor the card's footer is handed (§3.19). */
const rerun = () => {
  const found = terminalRunActions().find((action) => action.id === "rerun");
  if (found === undefined) throw new Error("terminalRunActions lost Re-run");
  return found;
};

describe("TerminalRunRow", () => {
  it("disables Re-run while that re-run is in flight", async () => {
    const onAction = vi.fn();
    render(
      <TerminalRunRow
        note="wrote 2 files"
        status="done"
        rerun={rerun()}
        busy="rerun"
        onAction={onAction}
      />,
    );

    const button = screen.getByRole("button", { name: "Re-run" });
    expect(button).toBeDisabled();
    await userEvent.click(button);
    expect(onAction).not.toHaveBeenCalled();
  });

  it("leaves Re-run live when a different action is in flight", () => {
    render(
      <TerminalRunRow
        note={null}
        status="failed"
        rerun={rerun()}
        busy="cancel"
        onAction={vi.fn()}
      />,
    );
    expect(screen.getByRole("button", { name: "Re-run" })).toBeEnabled();
  });
});

describe("TerminalBanner", () => {
  it("disables the action that is in flight", async () => {
    const onAction = vi.fn();
    render(
      <TerminalBanner
        status="done"
        note="wrote 2 files"
        actions={terminalRunActions()}
        busy="rerun"
        onAction={onAction}
      />,
    );

    const button = screen.getByRole("button", { name: "Re-run" });
    expect(button).toBeDisabled();
    await userEvent.click(button);
    expect(onAction).not.toHaveBeenCalled();
  });

  it("leaves it live when nothing is in flight", () => {
    render(
      <TerminalBanner
        status="done"
        note={null}
        actions={terminalRunActions()}
        onAction={vi.fn()}
      />,
    );
    expect(screen.getByRole("button", { name: "Re-run" })).toBeEnabled();
  });
});
