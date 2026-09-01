/**
 * Resizable pane widths (DESIGN_SPEC §4.6).
 *
 * The storage key and value shape are deliberately unchanged from the retired
 * SvelteKit build so an existing user's layout survives the rework.
 */

export type PaneKey = "workW" | "workListW" | "libListW";

export interface PaneWidths {
  /** Chat aside. */
  workW: number;
  /** Work view's left column. */
  workListW: number;
  /** Library view's left column. */
  libListW: number;
}

export const PANE_WIDTHS_STORAGE_KEY = "oa-pane-widths";

export const PANE_DEFAULTS: PaneWidths = {
  workW: 396,
  workListW: 340,
  libListW: 326,
};

export const PANE_BOUNDS: Record<PaneKey, { min: number; max: number }> = {
  workW: { min: 300, max: 600 },
  workListW: { min: 260, max: 480 },
  libListW: { min: 260, max: 480 },
};

export const PANE_KEYS: readonly PaneKey[] = ["workW", "workListW", "libListW"];

/** Clamp to the pane's range. `NaN` has no position on the axis, so it resets. */
export function clampPaneWidth(key: PaneKey, value: number): number {
  if (Number.isNaN(value)) return PANE_DEFAULTS[key];
  const { min, max } = PANE_BOUNDS[key];
  return Math.round(Math.min(max, Math.max(min, value)));
}

/**
 * Parse a persisted payload with **per-key** fallbacks: one corrupt width must
 * not reset the other two.
 */
export function parsePaneWidths(raw: string | null): PaneWidths {
  if (raw === null) return { ...PANE_DEFAULTS };

  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return { ...PANE_DEFAULTS };
  }
  if (typeof parsed !== "object" || parsed === null)
    return { ...PANE_DEFAULTS };

  const source = parsed as Record<string, unknown>;
  const widths = { ...PANE_DEFAULTS };
  for (const key of PANE_KEYS) {
    const value = source[key];
    widths[key] =
      typeof value === "number"
        ? clampPaneWidth(key, value)
        : PANE_DEFAULTS[key];
  }
  return widths;
}

/** A `localStorage`-shaped dependency, so tests need no jsdom globals. */
export interface WidthStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

function browserStorage(): WidthStorage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    // Private-mode / disabled storage throws on access.
    return null;
  }
}

export function loadPaneWidths(
  storage: WidthStorage | null = browserStorage(),
): PaneWidths {
  if (storage === null) return { ...PANE_DEFAULTS };
  try {
    return parsePaneWidths(storage.getItem(PANE_WIDTHS_STORAGE_KEY));
  } catch {
    return { ...PANE_DEFAULTS };
  }
}

export function savePaneWidths(
  widths: PaneWidths,
  storage: WidthStorage | null = browserStorage(),
): void {
  if (storage === null) return;
  try {
    storage.setItem(PANE_WIDTHS_STORAGE_KEY, JSON.stringify(widths));
  } catch {
    // A full or blocked quota must never break a drag.
  }
}
