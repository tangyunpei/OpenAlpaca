/**
 * The application root (DESIGN_SPEC §2, §4.5).
 *
 * Structure, top to bottom:
 *   `QueryProvider`  — the TanStack Query cache **and** the daemon event
 *                      socket; every view below reads server state through it,
 *                      and every live event invalidates through it.
 *   `AppShell`       — the fluid frame (§2), `position:relative` so the
 *                      overlays can be absolute siblings of the panes.
 *   `NavRail`        — always mounted, never swapped (§2.1).
 *   one of four views— lazily loaded, keyed on the UI store's `view`.
 *   overlay slots    — the command palette (z-50) and the toast (z-60).
 *
 * This root owns **all** of the global keyboard surface (§4.5) and owns it
 * exactly once:
 *
 *   `useGlobalKeys`      ⌘K and the ordered Escape ladder, with the ladder's
 *                        last rung (deny) and the Enter approval supplied by
 *                        the chat lane's published confirmation.
 *   `useCommandShortcuts` the palette's own shortcuts — ⌘N, ⌘⇧S, ⌘1–3, ⌘⇧D,
 *                        ⌘, — bound off the same catalogue the palette draws.
 *
 * The confirmation itself lives in the chat session, behind a lazy boundary, so
 * it reaches the rail, the palette and the key ladder through the one-slot
 * registry in `stores/confirmation` rather than through four levels of props.
 */

import { lazy, Suspense } from "react";

import {
  CommandPalette,
  ToastHost,
  useCommandCatalog,
  useCommandShortcuts,
} from "@/components/overlays";
import { AppShell, NavRail, useGlobalKeys } from "@/components/shell";
import { QueryProvider } from "@/lib/query-provider";
import { useConfirmationStore } from "@/stores/confirmation";
import { useUiStore, type View } from "@/stores/ui";

const ChatView = lazy(() => import("@/views/ChatView"));
const WorkView = lazy(() => import("@/views/WorkView"));
const LibraryView = lazy(() => import("@/views/LibraryView"));
const SettingsView = lazy(() => import("@/views/SettingsView"));

function renderView(view: View, blockedRunId: string | null) {
  switch (view) {
    case "chat":
      return <ChatView />;
    case "work":
      return <WorkView blockedRunId={blockedRunId} />;
    case "library":
      return <LibraryView />;
    case "settings":
      return <SettingsView />;
  }
}

/** The frame below the providers — rendered directly by tests. */
export function AppFrame() {
  const view = useUiStore((s) => s.view);
  const pending = useConfirmationStore((s) => s.pending);

  useGlobalKeys({
    blocked: pending !== null,
    onApprove: pending?.approve,
    onDeny: pending?.deny,
  });
  useCommandShortcuts(useCommandCatalog());

  return (
    <AppShell>
      <NavRail blockedRunId={pending?.runId ?? null} />

      <Suspense fallback={<div className="min-w-0 flex-1 bg-main" />}>
        {renderView(view, pending?.runId ?? null)}
      </Suspense>

      <CommandPalette />
      <ToastHost />
    </AppShell>
  );
}

export default function App() {
  return (
    <QueryProvider>
      <AppFrame />
    </QueryProvider>
  );
}
