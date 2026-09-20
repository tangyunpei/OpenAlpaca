/**
 * `TrafficLights` (DESIGN_SPEC §3.1).
 *
 * The design draws macOS window buttons in the rail because the artboard *is*
 * the whole window. The Tauri shell keeps native decorations (`tauri.conf.json`
 * declares no `decorations: false`), so painting a second set would duplicate
 * the OS chrome. §3.1 offers exactly two resolutions; this takes both:
 *
 *   * ask the window whether it is decorated, and render nothing when it is —
 *     the native buttons are the design's buttons;
 *   * when it is *not* decorated (a custom-chrome build, or the browser dev
 *     server where there is no chrome at all), render them and make them work.
 *
 * The check starts as "unknown" and renders nothing, so the real app never
 * flashes a duplicate set on boot.
 */

import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";

type WindowAction = "close" | "minimize" | "toggleMaximize";

async function runWindowAction(action: WindowAction): Promise<void> {
  const appWindow = getCurrentWindow();
  if (action === "close") return appWindow.close();
  if (action === "minimize") return appWindow.minimize();
  return appWindow.toggleMaximize();
}

/** `null` until the window answers; `true` when the OS draws the chrome. */
export function useNativeDecorations(): boolean | null {
  const [decorated, setDecorated] = useState<boolean | null>(null);

  useEffect(() => {
    let alive = true;
    const settle = (value: boolean) => {
      if (alive) setDecorated(value);
    };
    try {
      void getCurrentWindow()
        .isDecorated()
        .then(settle, () => settle(false));
    } catch {
      // No Tauri bridge (browser dev server / jsdom): no native chrome.
      settle(false);
    }
    return () => {
      alive = false;
    };
  }, []);

  return decorated;
}

const LIGHTS: ReadonlyArray<{
  action: WindowAction;
  label: string;
  color: string;
}> = [
  { action: "close", label: "Close window", color: "bg-tl-close" },
  { action: "minimize", label: "Minimize window", color: "bg-tl-min" },
  { action: "toggleMaximize", label: "Zoom window", color: "bg-tl-max" },
];

export function TrafficLights() {
  const decorated = useNativeDecorations();
  if (decorated !== false) return null;

  return (
    <div className="mb-[18px] flex gap-[8px] px-[6px] pt-[2px]">
      {LIGHTS.map(({ action, label, color }) => (
        <button
          key={action}
          type="button"
          aria-label={label}
          title={label}
          onClick={() => void runWindowAction(action).catch(() => undefined)}
          className={`h-[12px] w-[12px] cursor-pointer rounded-full border border-[rgba(0,0,0,.06)] p-0 ${color} focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue`}
        />
      ))}
    </div>
  );
}
