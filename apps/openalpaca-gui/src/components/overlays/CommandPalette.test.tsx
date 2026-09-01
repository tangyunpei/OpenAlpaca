import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useUiStore } from "@/stores/ui";

import { CommandPalette } from "./CommandPalette";

const tasks = vi.hoisted(() => ({
  data: [{ id: "b41c8e02", title: "Connector audit" }],
}));

vi.mock("@/hooks/useTasks", () => ({
  useTasks: () => tasks,
}));

beforeEach(() => {
  useUiStore.setState({
    paletteOpen: true,
    view: "chat",
    dense: false,
    settingsSectionId: "connection",
    steerTargetRunId: null,
  });
});

describe("CommandPalette (§3.33)", () => {
  it("renders nothing while the palette is closed", () => {
    useUiStore.setState({ paletteOpen: false });
    const { container } = render(<CommandPalette />);
    expect(container).toBeEmptyDOMElement();
  });

  it("focuses the input on mount", () => {
    render(<CommandPalette />);
    expect(screen.getByRole("combobox")).toHaveFocus();
  });

  it("filters as you type over group + label", async () => {
    const user = userEvent.setup();
    render(<CommandPalette />);
    await user.type(screen.getByRole("combobox"), "librar");
    const options = screen.getAllByRole("option");
    expect(options).toHaveLength(1);
    expect(options[0]).toHaveTextContent("Library — artifacts");
  });

  it("says so when nothing matches", async () => {
    const user = userEvent.setup();
    render(<CommandPalette />);
    await user.type(screen.getByRole("combobox"), "zzzz");
    expect(screen.queryAllByRole("option")).toHaveLength(0);
    expect(screen.getByText("No commands match that.")).toBeInTheDocument();
  });

  it("moves the selection with the arrow keys and runs it with Enter", async () => {
    const user = userEvent.setup();
    render(<CommandPalette />);

    // Row 0 is `New background job`; row 1 is the Steer row for the active run.
    await user.keyboard("{ArrowDown}");
    expect(screen.getAllByRole("option")[1]).toHaveAttribute(
      "aria-selected",
      "true",
    );

    await user.keyboard("{Enter}");
    const state = useUiStore.getState();
    expect(state.paletteOpen).toBe(false);
    expect(state.steerTargetRunId).toBe("b41c8e02");
  });

  it("wraps the selection at both ends", async () => {
    const user = userEvent.setup();
    render(<CommandPalette />);
    const count = screen.getAllByRole("option").length;
    await user.keyboard("{ArrowUp}");
    expect(screen.getAllByRole("option")[count - 1]).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  it("dispatches a Go command into the ui store", async () => {
    const user = userEvent.setup();
    render(<CommandPalette />);
    await user.click(screen.getByRole("option", { name: /Work — all runs/ }));
    expect(useUiStore.getState().view).toBe("work");
    expect(useUiStore.getState().paletteOpen).toBe(false);
  });

  it("opens Settings on the section the command names", async () => {
    const user = userEvent.setup();
    render(<CommandPalette />);
    await user.click(
      screen.getByRole("option", { name: /Settings — skills & plugins/ }),
    );
    expect(useUiStore.getState().view).toBe("settings");
    expect(useUiStore.getState().settingsSectionId).toBe("skills");
  });

  it("only offers Approve when the chat lane has a pending confirmation", async () => {
    const onApprove = vi.fn();
    const { rerender } = render(<CommandPalette />);
    expect(screen.queryByRole("option", { name: /Approve/ })).toBeNull();

    rerender(
      <CommandPalette
        pendingConfirmation={{ toolName: "shell_execute", onApprove }}
      />,
    );
    await userEvent
      .setup()
      .click(
        screen.getByRole("option", { name: /Approve pending shell_execute/ }),
      );
    expect(onApprove).toHaveBeenCalledOnce();
  });

  it("names the missing artifact route instead of faking Find rows", () => {
    render(<CommandPalette />);
    expect(
      screen.getByText(/Artifact search is unavailable/),
    ).toHaveTextContent("GET /v1/artifacts");
  });
});
