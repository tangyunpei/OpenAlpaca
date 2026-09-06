import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { PANE_DEFAULTS } from "./pane-widths";
import {
  selectShowAside,
  selectWorkClosed,
  TOAST_DURATION_MS,
  useUiStore,
  type UiState,
} from "./ui";

const initial: Partial<UiState> = {
  view: "chat",
  workOpen: true,
  sessionsOpen: true,
  panelArtifactId: null,
  pickerOpen: false,
  paletteOpen: false,
  toast: null,
  dense: false,
};

beforeEach(() => {
  localStorage.clear();
  useUiStore.setState({
    ...initial,
    paneWidths: { ...PANE_DEFAULTS },
    pins: {},
  });
});

afterEach(() => {
  vi.useRealTimers();
});

describe("pane widths", () => {
  it("clamps on set and persists on demand", () => {
    const { setPaneWidth, persistPaneWidths } = useUiStore.getState();
    setPaneWidth("workW", 5000);
    expect(useUiStore.getState().paneWidths.workW).toBe(600);

    persistPaneWidths();
    expect(localStorage.getItem("oa-pane-widths")).toContain('"workW":600');
  });

  it("resets one pane to its default and persists immediately", () => {
    useUiStore.getState().setPaneWidth("libListW", 470);
    useUiStore.getState().resetPaneWidth("libListW");

    expect(useUiStore.getState().paneWidths.libListW).toBe(
      PANE_DEFAULTS.libListW,
    );
    expect(localStorage.getItem("oa-pane-widths")).toContain('"libListW":326');
  });
});

describe("aside modes", () => {
  it("makes the work pane and the file panel mutually exclusive", () => {
    useUiStore.getState().openSidePanel("findings");
    expect(useUiStore.getState().panelArtifactId).toBe("findings");

    useUiStore.getState().openWorkPane();
    expect(useUiStore.getState().panelArtifactId).toBeNull();
    expect(useUiStore.getState().workOpen).toBe(true);
  });

  it("shows the header pill only when the aside is fully collapsed", () => {
    useUiStore.getState().openSidePanel("findings");
    useUiStore.getState().closePanel();

    const state = useUiStore.getState();
    expect(selectShowAside(state)).toBe(false);
    expect(selectWorkClosed(state)).toBe(true);
  });

  it("carries the panel tab across into the Library view", () => {
    useUiStore.getState().openSidePanel("findings");
    useUiStore.getState().setPanelTab("history");
    useUiStore.getState().openInLibrary();

    const state = useUiStore.getState();
    expect(state.view).toBe("library");
    expect(state.openArtifactId).toBe("findings");
    expect(state.libraryTab).toBe("history");
    expect(state.panelArtifactId).toBeNull();
  });
});

describe("the conversation column", () => {
  /**
   * It is a fourth fixed column in a shell whose minimum window is budgeted
   * pane by pane, so it has to be closable — and the state is its own, not the
   * aside's: collapsing the conversations must not take the Work pane with it.
   */
  it("collapses and reopens without touching the aside", () => {
    expect(useUiStore.getState().sessionsOpen).toBe(true);

    useUiStore.getState().closeSessions();
    expect(useUiStore.getState().sessionsOpen).toBe(false);
    expect(useUiStore.getState().workOpen).toBe(true);

    useUiStore.getState().openSessions();
    expect(useUiStore.getState().sessionsOpen).toBe(true);
  });

  it("has a width in the same store as the other three columns", () => {
    const { setPaneWidth } = useUiStore.getState();
    setPaneWidth("chatSessionsW", 9000);
    expect(useUiStore.getState().paneWidths.chatSessionsW).toBe(360);

    setPaneWidth("chatSessionsW", 10);
    expect(useUiStore.getState().paneWidths.chatSessionsW).toBe(200);
  });
});

describe("escape ladder", () => {
  it("consumes the palette, then the picker, then the panel, then defers", () => {
    const store = useUiStore.getState();
    store.setPaletteOpen(true);
    store.openSidePanel("findings");
    useUiStore.setState({ pickerOpen: true });

    expect(useUiStore.getState().handleEscape()).toBe("palette");
    expect(useUiStore.getState().handleEscape()).toBe("picker");
    expect(useUiStore.getState().handleEscape()).toBe("panel");
    // Nothing left for the UI to close — the caller denies the confirmation.
    expect(useUiStore.getState().handleEscape()).toBe("none");
    expect(useUiStore.getState().workOpen).toBe(true);
  });
});

describe("toast", () => {
  it("holds one slot and clears the previous timer", () => {
    vi.useFakeTimers();
    const { showToast } = useUiStore.getState();

    showToast("first");
    vi.advanceTimersByTime(TOAST_DURATION_MS - 100);
    showToast("second");

    // The first toast's timer must not fire and blank the second.
    vi.advanceTimersByTime(200);
    expect(useUiStore.getState().toast).toBe("second");

    vi.advanceTimersByTime(TOAST_DURATION_MS);
    expect(useUiStore.getState().toast).toBeNull();
  });
});

/**
 * `PUT /v1/artifacts/{id}/pin` made the server the authority. `oa-pins` stays
 * only as the optimistic cache: it shows a star before the round trip finishes
 * and survives a reload, and the server's own answer overwrites it.
 */
describe("pins", () => {
  beforeEach(() => {
    useUiStore.setState({ pins: {} });
    localStorage.removeItem("oa-pins");
  });

  it("writes optimistically and persists to localStorage", () => {
    useUiStore.getState().setPin("findings", true);
    expect(useUiStore.getState().isPinned("findings")).toBe(true);
    expect(localStorage.getItem("oa-pins")).toBe('{"findings":true}');

    useUiStore.getState().setPin("findings", false);
    expect(useUiStore.getState().isPinned("findings")).toBe(false);
  });

  it("lets a server page overwrite the cache, disagreement and all", () => {
    useUiStore.getState().setPin("findings", true);
    useUiStore.getState().setPin("notes", true);

    // The rows came back saying `findings` is not pinned — the server wins.
    useUiStore.getState().syncPins({ findings: false });

    expect(useUiStore.getState().isPinned("findings")).toBe(false);
    // A row the page did not carry keeps whatever it had.
    expect(useUiStore.getState().isPinned("notes")).toBe(true);
    expect(localStorage.getItem("oa-pins")).toBe(
      '{"findings":false,"notes":true}',
    );
  });

  it("does not touch the store when a page agrees with the cache", () => {
    useUiStore.getState().setPin("findings", true);
    const before = useUiStore.getState().pins;

    useUiStore.getState().syncPins({ findings: true });

    // Identity matters: a new object every fetch would re-render every row.
    expect(useUiStore.getState().pins).toBe(before);
  });
});
