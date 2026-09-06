import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { Session } from "@/lib/api/types";

import { SessionSidebar } from "./SessionSidebar";

function session(overrides: Partial<Session> = {}): Session {
  return {
    id: "sess-1",
    lane_key: "user:gui",
    source: "gui",
    title: "Connector audit",
    workspace_id: "/Users/dev/openalpaca",
    status: "active",
    message_count: 12,
    last_message_at: "2026-09-06 10:00:00",
    created_at: "2026-09-06 09:00:00",
    updated_at: "2026-09-06 10:00:00",
    ended_at: null,
    active_task_count: 0,
    interrupted_task_count: 0,
    ...overrides,
  };
}

function renderSidebar(props: Partial<Parameters<typeof SessionSidebar>[0]>) {
  return render(
    <SessionSidebar
      sessions={[]}
      selectedId={null}
      loading={false}
      error={null}
      busyId={null}
      creating={false}
      actionError={null}
      windowProject={null}
      width={236}
      loaded={0}
      total={0}
      loadingMore={false}
      onLoadMore={vi.fn()}
      onCollapse={vi.fn()}
      onNewChat={vi.fn()}
      onSelect={vi.fn()}
      onRename={vi.fn()}
      onArchive={vi.fn()}
      onDelete={vi.fn()}
      {...props}
    />,
  );
}

describe("SessionSidebar — honest states", () => {
  it("says it is loading rather than showing an empty list", () => {
    renderSidebar({ loading: true });
    expect(screen.getByText("Loading conversations…")).toBeInTheDocument();
    expect(screen.queryByRole("listitem")).not.toBeInTheDocument();
  });

  /**
   * A list that could not be read is not an empty list. Saying "no
   * conversations" over a failed request would be a fabrication — the rows
   * may well exist.
   */
  it("names the failure instead of reading as empty", () => {
    renderSidebar({ error: new Error("daemon unreachable") });
    expect(
      screen.getByText(/Could not read this lane's conversations/),
    ).toHaveTextContent("daemon unreachable");
    expect(screen.queryByText("No conversations yet.")).not.toBeInTheDocument();
  });

  it("shows the design's own empty copy when the lane really has none", () => {
    renderSidebar({});
    expect(screen.getByText("No conversations yet.")).toBeInTheDocument();
  });
});

describe("SessionSidebar — the rows", () => {
  it("renders each conversation with its project, count and stamp", () => {
    renderSidebar({ sessions: [session()] });

    expect(
      screen.getByRole("button", { name: /Connector audit/ }),
    ).toBeInTheDocument();
    expect(screen.getByText(/openalpaca/)).toBeInTheDocument();
    expect(screen.getByText(/12 messages/)).toBeInTheDocument();
    expect(screen.getByText(/6 Sep/)).toBeInTheDocument();
  });

  /** `title` is `""` until the conversation is renamed — never render a blank row. */
  it("falls back to a name for an untitled conversation", () => {
    renderSidebar({ sessions: [session({ title: "" })] });
    expect(
      screen.getByRole("button", { name: /Untitled conversation/ }),
    ).toBeInTheDocument();
  });

  /** A session with no project is a real state, not missing data. */
  it("says so when a conversation is bound to no project", () => {
    renderSidebar({ sessions: [session({ workspace_id: null })] });
    expect(screen.getByText(/no project/)).toBeInTheDocument();
  });

  it("badges an archived conversation", () => {
    renderSidebar({ sessions: [session({ status: "archived" })] });
    expect(screen.getByText("archived")).toBeInTheDocument();
  });

  /**
   * `interrupted_task_count` is structurally 0 until Phase 7b's boot sweep
   * writes the status; the badge is here so it starts telling the truth the
   * moment it does, with no client change.
   */
  it("badges a conversation whose runs were interrupted", () => {
    renderSidebar({ sessions: [session({ interrupted_task_count: 2 })] });
    expect(screen.getByText("2 interrupted")).toBeInTheDocument();
  });

  it("marks the selected row for assistive tech, not just visually", () => {
    renderSidebar({ sessions: [session()], selectedId: "sess-1" });
    expect(
      screen.getByRole("button", { name: /Connector audit/ }),
    ).toHaveAttribute("aria-current", "true");
  });

  it("selects the row that was clicked, by id", () => {
    const onSelect = vi.fn();
    renderSidebar({
      sessions: [session(), session({ id: "sess-2", title: "Docs pass" })],
      onSelect,
    });

    fireEvent.click(screen.getByRole("button", { name: /Docs pass/ }));
    expect(onSelect).toHaveBeenCalledWith("sess-2");
  });
});

describe("SessionSidebar — the write verbs", () => {
  it("starts a new chat", () => {
    const onNewChat = vi.fn();
    renderSidebar({ onNewChat });
    fireEvent.click(screen.getByRole("button", { name: "New chat" }));
    expect(onNewChat).toHaveBeenCalledTimes(1);
  });

  it("renames through an inline field, committing on submit", () => {
    const onRename = vi.fn();
    renderSidebar({ sessions: [session()], onRename });

    fireEvent.click(screen.getByRole("button", { name: "Rename" }));
    const field = screen.getByLabelText("Conversation title");
    fireEvent.change(field, { target: { value: "Connector audit v2" } });
    fireEvent.submit(field);

    expect(onRename).toHaveBeenCalledWith("sess-1", "Connector audit v2");
    expect(
      screen.queryByLabelText("Conversation title"),
    ).not.toBeInTheDocument();
  });

  it("abandons a rename on Escape without calling the daemon", () => {
    const onRename = vi.fn();
    renderSidebar({ sessions: [session()], onRename });

    fireEvent.click(screen.getByRole("button", { name: "Rename" }));
    fireEvent.keyDown(screen.getByLabelText("Conversation title"), {
      key: "Escape",
    });

    expect(onRename).not.toHaveBeenCalled();
    expect(
      screen.queryByLabelText("Conversation title"),
    ).not.toBeInTheDocument();
  });

  it("archives only what is still live", () => {
    const onArchive = vi.fn();
    renderSidebar({
      sessions: [session({ status: "archived" })],
      onArchive,
    });
    expect(
      screen.queryByRole("button", { name: "Archive" }),
    ).not.toBeInTheDocument();
  });

  /**
   * Deleting takes a transcript with it and the daemon offers no undo, so the
   * first click only arms it.
   */
  it("asks once before deleting", () => {
    const onDelete = vi.fn();
    renderSidebar({ sessions: [session()], onDelete });

    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(onDelete).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Confirm delete" }));
    expect(onDelete).toHaveBeenCalledWith("sess-1");
  });

  it("renders a refusal in the daemon's own terms", () => {
    renderSidebar({
      sessions: [session()],
      actionError:
        "This conversation has a run in flight — cancel it before deleting.",
    });
    expect(screen.getByRole("alert")).toHaveTextContent(/run in flight/);
  });

  it("disables a row's verbs while one of its writes is in flight", () => {
    renderSidebar({ sessions: [session()], busyId: "sess-1" });
    expect(screen.getByRole("button", { name: "Rename" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Archive" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Delete" })).toBeDisabled();
  });
});

describe("SessionSidebar — R49, the resumed project", () => {
  /**
   * A turn that names a session takes that session's project, overriding the
   * window's. Without this line the project indicator in Settings would look
   * authoritative while the turn ran somewhere else entirely.
   */
  it("says which project the next turn will run in when they differ", () => {
    renderSidebar({
      sessions: [session()],
      selectedId: "sess-1",
      windowProject: "/Users/dev/other",
    });
    expect(
      screen.getByText(/runs in \/Users\/dev\/openalpaca/),
    ).toHaveTextContent("/Users/dev/other");
  });

  it("stays quiet when the window and the conversation agree", () => {
    renderSidebar({
      sessions: [session()],
      selectedId: "sess-1",
      windowProject: "/Users/dev/openalpaca",
    });
    expect(screen.queryByText(/runs in/)).not.toBeInTheDocument();
  });

  /** Nothing to override: an unbound conversation takes the window's project. */
  it("stays quiet for a conversation with no project of its own", () => {
    renderSidebar({
      sessions: [session({ workspace_id: null })],
      selectedId: "sess-1",
      windowProject: "/Users/dev/other",
    });
    expect(screen.queryByText(/runs in/)).not.toBeInTheDocument();
  });
});

describe("SessionSidebar — the column itself", () => {
  /**
   * A fourth fixed column in a shell whose minimum window is budgeted pane by
   * pane has to be closable, and its width belongs in the same store as the
   * other three rather than being a literal in the class list.
   */
  it("takes its width from the caller, not from a hardcoded class", () => {
    renderSidebar({ width: 300 });
    expect(
      screen.getByRole("complementary", { name: "Conversations" }),
    ).toHaveStyle({
      width: "300px",
    });
  });

  it("offers a way to collapse itself", () => {
    const onCollapse = vi.fn();
    renderSidebar({ onCollapse });

    fireEvent.click(
      screen.getByRole("button", { name: "Collapse conversations" }),
    );
    expect(onCollapse).toHaveBeenCalledTimes(1);
  });
});

describe("SessionSidebar — the list is longer than the page", () => {
  /**
   * The envelope's `total` was fetched and thrown away, so a lane with more
   * conversations than one page showed the newest hundred and looked complete.
   * The number that says otherwise was already in hand.
   */
  it("says how much of the list it has, and offers the rest", () => {
    const onLoadMore = vi.fn();
    renderSidebar({
      sessions: [session()],
      loaded: 100,
      total: 143,
      onLoadMore,
    });

    expect(screen.getByText("100 of 143 loaded")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Show more" }));
    expect(onLoadMore).toHaveBeenCalledTimes(1);
  });

  it("says nothing once the whole list is in hand", () => {
    renderSidebar({ sessions: [session()], loaded: 12, total: 12 });

    expect(screen.queryByRole("button", { name: "Show more" })).toBeNull();
    expect(screen.queryByText(/loaded/)).not.toBeInTheDocument();
  });

  it("holds the control while the next page is on the wire", () => {
    renderSidebar({
      sessions: [session()],
      loaded: 100,
      total: 143,
      loadingMore: true,
    });

    expect(screen.getByRole("button", { name: "loading…" })).toBeDisabled();
  });
});
