/**
 * `NavRail` (DESIGN_SPEC §2.1) — 196px, always mounted, never scrolls.
 *
 * Everything it shows is real:
 *   * "Running now" and the Work badge come from `GET /v1/tasks?status=active`,
 *     whose SQL is `status IN ('queued','running','paused')` — exactly §4.2's
 *     `railRuns` / `activeCount` sets;
 *   * the connection row comes from the live socket and `/v1/health`.
 *
 * The Library count is **omitted**, not zeroed: there is no artifact listing
 * route (API_MAP §3, GAP-04), and a `0` beside Library would be a claim about
 * the user's library that this app cannot make. The count returns with the API.
 */

import { ChatIcon, LibraryIcon, SettingsIcon, WorkIcon } from "@/lib/icons";
import { toUiStatus } from "@/components/ui";
import { useTasks } from "@/hooks/useTasks";
import { useUiStore } from "@/stores/ui";

import { Brand } from "./Brand";
import { CommandButton } from "./CommandButton";
import { ConnectionRow } from "./ConnectionRow";
import { NavItem } from "./NavItem";
import { RunningNowSection, type RailRun } from "./RunningNowSection";
import { TrafficLights } from "./TrafficLights";

export interface NavRailProps {
  /** The run holding a pending tool confirmation, once chat knows of one. */
  blockedRunId?: string | null;
}

export function NavRail({ blockedRunId = null }: NavRailProps) {
  const view = useUiStore((s) => s.view);
  const setView = useUiStore((s) => s.setView);
  const focusRun = useUiStore((s) => s.focusRun);
  const setPaletteOpen = useUiStore((s) => s.setPaletteOpen);

  const activeTasks = useTasks({ status: "active" });
  const runs: RailRun[] = (activeTasks.data ?? []).map((task) => ({
    id: task.id,
    title: task.title,
    status: toUiStatus(task.status),
  }));

  return (
    <nav
      aria-label="Primary"
      className="flex w-rail shrink-0 flex-col border-r border-line-strong bg-rail px-[12px] py-[16px]"
    >
      <TrafficLights />
      <Brand />

      <div className="flex flex-col gap-[2px]">
        <NavItem
          icon={<ChatIcon />}
          label="Chat"
          active={view === "chat"}
          onSelect={() => setView("chat")}
        />
        <NavItem
          icon={<WorkIcon />}
          label="Work"
          active={view === "work"}
          onSelect={() => setView("work")}
          count={runs.length > 0 ? runs.length : undefined}
          countStyle="badge"
        />
        <NavItem
          icon={<LibraryIcon />}
          label="Library"
          active={view === "library"}
          onSelect={() => setView("library")}
        />
      </div>

      <RunningNowSection
        runs={runs}
        onFocusRun={focusRun}
        blockedRunId={blockedRunId}
      />

      <div className="mt-auto flex flex-col gap-[10px]">
        <CommandButton onOpen={() => setPaletteOpen(true)} />
        <ConnectionRow />
        <NavItem
          icon={<SettingsIcon />}
          label="Settings"
          active={view === "settings"}
          onSelect={() => setView("settings")}
        />
      </div>
    </nav>
  );
}
