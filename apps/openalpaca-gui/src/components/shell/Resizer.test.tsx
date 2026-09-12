import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";

import { PANE_DEFAULTS, PANE_WIDTHS_STORAGE_KEY } from "@/stores/pane-widths";
import { useUiStore } from "@/stores/ui";

import { Resizer } from "./Resizer";

beforeEach(() => {
  localStorage.clear();
  useUiStore.setState({ paneWidths: { ...PANE_DEFAULTS } });
});

function widths() {
  return useUiStore.getState().paneWidths;
}

describe("Resizer", () => {
  it("grows the chat aside when dragged left (direction -1, §2.7)", () => {
    render(<Resizer paneKey="workW" direction={-1} label="chat side pane" />);
    const grip = screen.getByRole("separator");

    fireEvent.mouseDown(grip, { clientX: 900 });
    fireEvent.mouseMove(window, { clientX: 860 });
    expect(widths().workW).toBe(PANE_DEFAULTS.workW + 40);

    fireEvent.mouseUp(window);
    expect(
      JSON.parse(localStorage.getItem(PANE_WIDTHS_STORAGE_KEY) ?? "{}"),
    ).toMatchObject({ workW: PANE_DEFAULTS.workW + 40 });
  });

  it("grows a left-hand list when dragged right (direction +1)", () => {
    render(<Resizer paneKey="workListW" direction={1} label="run list" />);
    fireEvent.mouseDown(screen.getByRole("separator"), { clientX: 340 });
    fireEvent.mouseMove(window, { clientX: 380 });
    expect(widths().workListW).toBe(PANE_DEFAULTS.workListW + 40);
  });

  it("clamps at the pane's bounds", () => {
    render(<Resizer paneKey="workW" direction={-1} label="chat side pane" />);
    const grip = screen.getByRole("separator");

    fireEvent.mouseDown(grip, { clientX: 900 });
    fireEvent.mouseMove(window, { clientX: 100 });
    expect(widths().workW).toBe(600);

    fireEvent.mouseMove(window, { clientX: 1800 });
    expect(widths().workW).toBe(300);
    fireEvent.mouseUp(window);
  });

  it("stops tracking the pointer after mouseup", () => {
    render(<Resizer paneKey="workW" direction={-1} label="chat side pane" />);
    const grip = screen.getByRole("separator");

    fireEvent.mouseDown(grip, { clientX: 900 });
    fireEvent.mouseMove(window, { clientX: 880 });
    fireEvent.mouseUp(window);
    const settled = widths().workW;

    fireEvent.mouseMove(window, { clientX: 500 });
    expect(widths().workW).toBe(settled);
  });

  it("restores the body's cursor and selection after a drag", () => {
    render(<Resizer paneKey="workW" direction={-1} label="chat side pane" />);
    const grip = screen.getByRole("separator");

    fireEvent.mouseDown(grip, { clientX: 900 });
    expect(document.body.style.cursor).toBe("col-resize");
    expect(document.body.style.userSelect).toBe("none");

    fireEvent.mouseUp(window);
    expect(document.body.style.cursor).toBe("");
    expect(document.body.style.userSelect).toBe("");
  });

  it("resets to the default on double-click", () => {
    useUiStore.setState({
      paneWidths: { ...PANE_DEFAULTS, libListW: 470 },
    });
    render(<Resizer paneKey="libListW" direction={1} label="library list" />);
    fireEvent.doubleClick(screen.getByRole("separator"));
    expect(widths().libListW).toBe(PANE_DEFAULTS.libListW);
  });

  it("nudges with the arrow keys and reports its value (accessibility)", () => {
    render(<Resizer paneKey="workW" direction={-1} label="chat side pane" />);
    const grip = screen.getByRole("separator");
    expect(grip).toHaveAttribute("aria-valuenow", String(PANE_DEFAULTS.workW));
    expect(grip).toHaveAttribute("aria-valuemin", "300");
    expect(grip).toHaveAttribute("aria-valuemax", "600");

    fireEvent.keyDown(grip, { key: "ArrowLeft" });
    expect(widths().workW).toBe(PANE_DEFAULTS.workW + 16);
    fireEvent.keyDown(grip, { key: "Home" });
    expect(widths().workW).toBe(PANE_DEFAULTS.workW);
  });
});
