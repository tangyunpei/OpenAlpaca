import { fireEvent, render } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useUiStore } from "@/stores/ui";

import { useGlobalKeys, type GlobalKeyOptions } from "./useGlobalKeys";

function Harness(props: GlobalKeyOptions) {
  useGlobalKeys(props);
  return null;
}

const initial = useUiStore.getState();

beforeEach(() => {
  useUiStore.setState({
    ...initial,
    view: "chat",
    paletteOpen: false,
    pickerOpen: false,
    panelArtifactId: null,
    workOpen: true,
  });
});

describe("useGlobalKeys", () => {
  it("toggles the palette on ⌘K and again on ⌘K (§4.5)", () => {
    render(<Harness />);

    fireEvent.keyDown(window, { key: "k", metaKey: true });
    expect(useUiStore.getState().paletteOpen).toBe(true);

    fireEvent.keyDown(window, { key: "k", metaKey: true });
    expect(useUiStore.getState().paletteOpen).toBe(false);
  });

  it("accepts Ctrl+K and an uppercase K", () => {
    render(<Harness />);
    fireEvent.keyDown(window, { key: "K", ctrlKey: true });
    expect(useUiStore.getState().paletteOpen).toBe(true);
  });

  it("walks the Escape ladder in order: palette → picker → panel → deny", () => {
    const onDeny = vi.fn();
    render(<Harness blocked onDeny={onDeny} />);

    useUiStore.setState({
      paletteOpen: true,
      pickerOpen: true,
      panelArtifactId: "findings",
      workOpen: false,
    });

    fireEvent.keyDown(window, { key: "Escape" });
    let state = useUiStore.getState();
    expect(state.paletteOpen).toBe(false);
    expect(state.pickerOpen).toBe(true);
    expect(onDeny).not.toHaveBeenCalled();

    fireEvent.keyDown(window, { key: "Escape" });
    state = useUiStore.getState();
    expect(state.pickerOpen).toBe(false);
    expect(state.panelArtifactId).toBe("findings");

    fireEvent.keyDown(window, { key: "Escape" });
    state = useUiStore.getState();
    // The panel rung hands the aside back to the Work pane.
    expect(state.panelArtifactId).toBeNull();
    expect(state.workOpen).toBe(true);
    expect(onDeny).not.toHaveBeenCalled();

    fireEvent.keyDown(window, { key: "Escape" });
    expect(onDeny).toHaveBeenCalledTimes(1);
  });

  it("does nothing on Escape when nothing is open and nothing is blocked", () => {
    const onDeny = vi.fn();
    render(<Harness onDeny={onDeny} />);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onDeny).not.toHaveBeenCalled();
  });

  it("approves on Enter only while blocked, in chat, with the palette shut", () => {
    const onApprove = vi.fn();
    const { rerender } = render(<Harness onApprove={onApprove} />);

    fireEvent.keyDown(window, { key: "Enter" });
    expect(onApprove).not.toHaveBeenCalled();

    rerender(<Harness blocked onApprove={onApprove} />);
    fireEvent.keyDown(window, { key: "Enter" });
    expect(onApprove).toHaveBeenCalledTimes(1);

    // Shift+Enter is a newline, never an approval.
    fireEvent.keyDown(window, { key: "Enter", shiftKey: true });
    expect(onApprove).toHaveBeenCalledTimes(1);

    // Not while the palette owns the keyboard.
    useUiStore.setState({ paletteOpen: true });
    fireEvent.keyDown(window, { key: "Enter" });
    expect(onApprove).toHaveBeenCalledTimes(1);

    // Not from another view.
    useUiStore.setState({ paletteOpen: false, view: "work" });
    fireEvent.keyDown(window, { key: "Enter" });
    expect(onApprove).toHaveBeenCalledTimes(1);
  });

  it("leaves Enter alone inside a text field", () => {
    const onApprove = vi.fn();
    render(
      <>
        <Harness blocked onApprove={onApprove} />
        <input aria-label="query" />
      </>,
    );
    const input = document.querySelector("input");
    fireEvent.keyDown(input as Element, { key: "Enter" });
    expect(onApprove).not.toHaveBeenCalled();
  });

  it("removes its listener on unmount", () => {
    const { unmount } = render(<Harness />);
    unmount();
    fireEvent.keyDown(window, { key: "k", metaKey: true });
    expect(useUiStore.getState().paletteOpen).toBe(false);
  });
});
