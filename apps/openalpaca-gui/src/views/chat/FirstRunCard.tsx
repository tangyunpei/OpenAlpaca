/**
 * The empty transcript's first-run card (D-G, DESIGN_SPEC §5.4a).
 *
 * Drawn by `ChatView` only when the transcript is empty **and**
 * `firstRunState(...)` is `"no-model"` — the daemon's own answer that it can
 * route nothing. It names the problem in the daemon's words and has one way
 * out: Settings → Models & keys. It is never a modal and never blocks the
 * composer (see `first-run.ts` for why).
 *
 * `role="status"`, not `alert`: this is a state of the app, not an error that
 * just happened, and `alert` would interrupt a screen reader on every poll.
 * The amber treatment matches the Models banner, so the two surfaces read as
 * one system.
 */

import { Button } from "@/components/ui";
import type { DaemonLlmStatus } from "@/lib/api/types";

import {
  FIRST_RUN_ACTION,
  FIRST_RUN_HINT,
  FIRST_RUN_TITLE,
  firstRunBody,
} from "./first-run";

export interface FirstRunCardProps {
  llm: DaemonLlmStatus | null | undefined;
  onOpenSettings: () => void;
}

export function FirstRunCard({ llm, onOpenSettings }: FirstRunCardProps) {
  return (
    <section
      role="status"
      aria-label="Set up a model"
      className="mb-[26px] rounded-2xl border border-amber-line bg-amber-surface px-[16px] py-[14px]"
    >
      <h2 className="m-0 mb-[6px] text-md font-medium text-amber-ink">
        {FIRST_RUN_TITLE}
      </h2>
      <p className="m-0 mb-[4px] text-base leading-[1.5] text-amber-ink">
        {firstRunBody(llm)}
      </p>
      <p className="m-0 mb-[10px] text-base leading-[1.5] text-amber-ink">
        {FIRST_RUN_HINT}
      </p>
      <Button variant="primarySm" onClick={onOpenSettings}>
        {FIRST_RUN_ACTION}
      </Button>
    </section>
  );
}
