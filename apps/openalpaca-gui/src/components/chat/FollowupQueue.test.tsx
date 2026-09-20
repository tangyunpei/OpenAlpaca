import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { FollowupRecord } from "@/lib/api/followups";

import { FollowupQueue } from "./FollowupQueue";

function row(overrides: Partial<FollowupRecord> = {}): FollowupRecord {
  return {
    id: 42,
    lane_key: "user:gui",
    kind: "followup",
    content: "then write it up",
    source_task_id: "run-1",
    status: "queued",
    created_at: "2026-09-05 10:00:00",
    updated_at: "2026-09-05 10:00:00",
    ...overrides,
  };
}

describe("FollowupQueue", () => {
  it("renders nothing at all when the queue is empty", () => {
    const { container } = render(
      <FollowupQueue followups={[]} onCancel={vi.fn()} />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  /**
   * The two kinds are not the same thing and the pill has to say which: a
   * `follow-up` is work the daemon will *run*, an `unprocessed_steering` row is
   * a message a workflow never read and is shown on the lane's next turn.
   */
  it("names each row's kind, in the order the daemon serves them", () => {
    render(
      <FollowupQueue
        followups={[
          row({ id: 1, content: "then write it up" }),
          row({
            id: 2,
            kind: "unprocessed_steering",
            content: "check telegram first",
          }),
        ]}
        onCancel={vi.fn()}
      />,
    );

    expect(screen.getByText("then write it up")).toBeInTheDocument();
    expect(screen.getByText("check telegram first")).toBeInTheDocument();
    expect(screen.getByText("follow-up")).toBeInTheDocument();
    expect(screen.getByText("steering leftover")).toBeInTheDocument();
  });

  it("cancels the row that was clicked, by id", () => {
    const onCancel = vi.fn();
    render(
      <FollowupQueue
        followups={[row({ id: 1 }), row({ id: 7, content: "and ship it" })]}
        onCancel={onCancel}
      />,
    );

    const buttons = screen.getAllByRole("button", { name: "cancel" });
    fireEvent.click(buttons[1] as HTMLElement);
    expect(onCancel).toHaveBeenCalledExactlyOnceWith(7);
  });

  it("marks only the row a cancel is in flight for", () => {
    render(
      <FollowupQueue
        followups={[row({ id: 1 }), row({ id: 7, content: "and ship it" })]}
        cancellingId={7}
        onCancel={vi.fn()}
      />,
    );

    expect(screen.getByRole("button", { name: "cancel" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "cancelling…" })).toBeDisabled();
  });

  /**
   * A queue that cannot be read is worth saying out loud rather than rendering
   * as "nothing pending": items may still be queued, and the user would
   * otherwise believe the lane is clear.
   */
  it("says so when the queue could not be read", () => {
    render(
      <FollowupQueue
        followups={[]}
        error={new Error("connection refused")}
        onCancel={vi.fn()}
      />,
    );
    expect(screen.getByText(/connection refused/)).toBeInTheDocument();
    expect(screen.getByText(/follow-up queue/i)).toBeInTheDocument();
  });
});
