/**
 * Client-only UI state (DESIGN_SPEC §4.2, API_MAP §2.6).
 *
 * Nothing here belongs on the daemon. Server state lives in TanStack Query;
 * this store holds view selection, density, pane geometry, the overlays, and
 * the single-slot toast.
 *
 * Two things are persisted, both per-machine by design:
 *   `oa-pane-widths` — the three resizable columns (§4.6)
 *   `oa-pins`        — artifact pins (GAP-12: correct as a local preference)
 */

import { create } from "zustand";

import {
  clampPaneWidth,
  loadPaneWidths,
  PANE_DEFAULTS,
  savePaneWidths,
  type PaneKey,
  type PaneWidths,
} from "./pane-widths";

export type View = "chat" | "work" | "library" | "settings";
export type ArtifactTab = "preview" | "diff" | "history";
export type ComposerMode = "steer" | "queue";

/** Toast auto-dismiss, straight from the design. */
export const TOAST_DURATION_MS = 2600;

export const PINS_STORAGE_KEY = "oa-pins";

function loadPins(): Record<string, boolean> {
  try {
    if (typeof localStorage === "undefined") return {};
    const raw = localStorage.getItem(PINS_STORAGE_KEY);
    if (raw === null) return {};
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return {};
    const pins: Record<string, boolean> = {};
    for (const [key, value] of Object.entries(
      parsed as Record<string, unknown>,
    )) {
      if (typeof value === "boolean") pins[key] = value;
    }
    return pins;
  } catch {
    return {};
  }
}

function savePins(pins: Record<string, boolean>): void {
  try {
    if (typeof localStorage === "undefined") return;
    localStorage.setItem(PINS_STORAGE_KEY, JSON.stringify(pins));
  } catch {
    // Non-fatal: pins are a convenience.
  }
}

/** Whichever escape-ladder rung consumed the key, or `"none"` (§4.5). */
export type EscapeOutcome = "palette" | "picker" | "panel" | "none";

export interface UiState {
  // Navigation
  view: View;
  /** Selected run in the Work view. */
  selectedRunId: string | null;
  /** Selected artifact in the Library view. */
  openArtifactId: string | null;
  libraryTab: ArtifactTab;
  libraryKind: string;
  settingsSectionId: string;

  // Chat aside — `workOpen` and `panelArtifactId` are mutually exclusive modes
  workOpen: boolean;
  panelArtifactId: string | null;
  panelTab: ArtifactTab;
  pickerOpen: boolean;

  // Composer
  /** `null` until the daemon's default model is known — never a hardcoded id. */
  model: string | null;
  modelPickerOpen: boolean;
  steerTargetRunId: string | null;
  composerMode: ComposerMode;

  // Chrome
  dense: boolean;
  paletteOpen: boolean;
  toast: string | null;

  paneWidths: PaneWidths;
  pins: Record<string, boolean>;

  // ── actions ───────────────────────────────────────────────────────────────
  setView: (view: View) => void;
  focusRun: (runId: string) => void;
  openArtifact: (artifactId: string) => void;
  setLibraryTab: (tab: ArtifactTab) => void;
  setLibraryKind: (kind: string) => void;
  setSettingsSection: (sectionId: string) => void;

  openWorkPane: () => void;
  closeWorkPane: () => void;
  /** File row in a run card → open the aside's file panel. */
  openSidePanel: (artifactId: string) => void;
  /** `‹ Work` — back to the Work pane. */
  backToWork: () => void;
  /** `›` — collapse the aside entirely. */
  closePanel: () => void;
  setPanelTab: (tab: ArtifactTab) => void;
  togglePicker: () => void;
  closePicker: () => void;
  pickPanelArtifact: (artifactId: string) => void;
  /** Carries the current tab across into the Library view. */
  openInLibrary: () => void;

  setModel: (model: string) => void;
  toggleModelPicker: () => void;
  closeModelPicker: () => void;
  setSteerTarget: (runId: string | null, mode?: ComposerMode) => void;
  clearSteerTarget: () => void;

  toggleDense: () => void;
  setPaletteOpen: (open: boolean) => void;
  togglePalette: () => void;
  showToast: (text: string) => void;
  clearToast: () => void;

  setPaneWidth: (key: PaneKey, width: number) => void;
  resetPaneWidth: (key: PaneKey) => void;
  persistPaneWidths: () => void;

  togglePin: (artifactId: string) => boolean;
  isPinned: (artifactId: string) => boolean;

  /** The strictly-ordered Escape ladder. `"none"` ⇒ the caller should deny. */
  handleEscape: () => EscapeOutcome;
}

/**
 * Module-scoped so a new toast cancels the previous timer rather than letting
 * two timers race to clear one slot.
 */
let toastTimer: ReturnType<typeof setTimeout> | null = null;

export const useUiStore = create<UiState>((set, get) => ({
  view: "chat",
  selectedRunId: null,
  openArtifactId: null,
  libraryTab: "preview",
  libraryKind: "All",
  settingsSectionId: "connection",

  workOpen: true,
  panelArtifactId: null,
  panelTab: "preview",
  pickerOpen: false,

  model: null,
  modelPickerOpen: false,
  steerTargetRunId: null,
  composerMode: "steer",

  dense: false,
  paletteOpen: false,
  toast: null,

  paneWidths: loadPaneWidths(),
  pins: loadPins(),

  setView: (view) => set({ view }),
  focusRun: (runId) => set({ view: "work", selectedRunId: runId }),
  openArtifact: (artifactId) =>
    set({ view: "library", openArtifactId: artifactId, libraryTab: "preview" }),
  setLibraryTab: (libraryTab) => set({ libraryTab }),
  setLibraryKind: (libraryKind) => set({ libraryKind }),
  setSettingsSection: (settingsSectionId) => set({ settingsSectionId }),

  openWorkPane: () => set({ workOpen: true, panelArtifactId: null }),
  closeWorkPane: () => set({ workOpen: false }),
  openSidePanel: (artifactId) =>
    set({
      view: "chat",
      panelArtifactId: artifactId,
      panelTab: "preview",
      pickerOpen: false,
    }),
  backToWork: () =>
    set({ panelArtifactId: null, workOpen: true, pickerOpen: false }),
  closePanel: () =>
    set({ panelArtifactId: null, workOpen: false, pickerOpen: false }),
  setPanelTab: (panelTab) => set({ panelTab }),
  togglePicker: () => set((s) => ({ pickerOpen: !s.pickerOpen })),
  closePicker: () => set({ pickerOpen: false }),
  pickPanelArtifact: (artifactId) =>
    set({
      panelArtifactId: artifactId,
      panelTab: "preview",
      pickerOpen: false,
    }),
  openInLibrary: () =>
    set((s) => ({
      view: "library",
      openArtifactId: s.panelArtifactId ?? s.openArtifactId,
      libraryTab: s.panelTab,
      panelArtifactId: null,
      pickerOpen: false,
    })),

  setModel: (model) => set({ model, modelPickerOpen: false }),
  toggleModelPicker: () =>
    set((s) => ({ modelPickerOpen: !s.modelPickerOpen })),
  closeModelPicker: () => set({ modelPickerOpen: false }),
  setSteerTarget: (runId, mode = "steer") =>
    set({ view: "chat", steerTargetRunId: runId, composerMode: mode }),
  clearSteerTarget: () => set({ steerTargetRunId: null }),

  toggleDense: () => set((s) => ({ dense: !s.dense })),
  setPaletteOpen: (paletteOpen) => set({ paletteOpen }),
  togglePalette: () => set((s) => ({ paletteOpen: !s.paletteOpen })),

  showToast: (text) => {
    if (toastTimer !== null) clearTimeout(toastTimer);
    set({ toast: text });
    toastTimer = setTimeout(() => {
      toastTimer = null;
      set({ toast: null });
    }, TOAST_DURATION_MS);
  },
  clearToast: () => {
    if (toastTimer !== null) {
      clearTimeout(toastTimer);
      toastTimer = null;
    }
    set({ toast: null });
  },

  setPaneWidth: (key, width) =>
    set((s) => ({
      paneWidths: { ...s.paneWidths, [key]: clampPaneWidth(key, width) },
    })),
  resetPaneWidth: (key) => {
    set((s) => ({
      paneWidths: { ...s.paneWidths, [key]: PANE_DEFAULTS[key] },
    }));
    savePaneWidths(get().paneWidths);
  },
  persistPaneWidths: () => savePaneWidths(get().paneWidths),

  togglePin: (artifactId) => {
    const next = !get().pins[artifactId];
    const pins = { ...get().pins, [artifactId]: next };
    set({ pins });
    savePins(pins);
    return next;
  },
  isPinned: (artifactId) => get().pins[artifactId] === true,

  handleEscape: () => {
    const state = get();
    if (state.paletteOpen) {
      set({ paletteOpen: false });
      return "palette";
    }
    if (state.pickerOpen) {
      set({ pickerOpen: false });
      return "picker";
    }
    if (state.panelArtifactId !== null) {
      set({ panelArtifactId: null, workOpen: true });
      return "panel";
    }
    return "none";
  },
}));

// ── Derived selectors (DESIGN_SPEC §4.2) ────────────────────────────────────

export const selectShowAside = (s: UiState): boolean =>
  s.workOpen || s.panelArtifactId !== null;
export const selectWorkMode = (s: UiState): boolean =>
  s.panelArtifactId === null;
export const selectPanelOn = (s: UiState): boolean =>
  s.panelArtifactId !== null;
/** The only condition that shows the header's "N running" pill. */
export const selectWorkClosed = (s: UiState): boolean =>
  !s.workOpen && s.panelArtifactId === null;
/** Transcript column width and message gap both key off density. */
export const selectTranscriptMaxWidth = (s: UiState): number =>
  s.dense ? 780 : 720;
export const selectMessageGap = (s: UiState): number => (s.dense ? 20 : 30);

export type { PaneKey, PaneWidths };
