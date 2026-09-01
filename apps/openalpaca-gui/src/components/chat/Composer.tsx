/**
 * `Composer` (DESIGN_SPEC §3.16) — two mutually exclusive states.
 *
 * The blocked state is the important one: while a tool confirmation is
 * pending the textarea is **not rendered at all** (§3.16a). The action bar
 * replaces it, which is also why the global `Enter → approve` binding in §4.5
 * can never collide with `Enter → send`.
 *
 * `Always allow` sends `approval_scope: "entire_tool"` even though the daemon
 * drops it today (GAP-01, serde ignores unknown fields). The caller is
 * responsible for telling the user the truth about what that did.
 */

import { useLayoutEffect, useRef } from "react";

import { Button } from "@/components/ui";
import { cn } from "@/lib/cn";
import type { ModelEntry } from "@/lib/api/types";
import type { ComposerMode } from "@/stores/ui";

import { ModelPicker } from "./ModelPicker";

/** Auto-grow ceiling: past this the textarea scrolls instead of pushing the transcript out. */
const MAX_TEXTAREA_HEIGHT = 220;

export interface ComposerSteerTarget {
  mode: ComposerMode;
  /** Two or three words — the run's short label. */
  label: string;
}

export function composerPlaceholder(steer: ComposerSteerTarget | null): string {
  if (steer === null) return "Ask, or describe a job to run in the background…";
  return steer.mode === "steer"
    ? `Steer ${steer.label} mid-run…`
    : `Queue a follow-up after ${steer.label}…`;
}

export interface ComposerProps {
  blocked: boolean;

  // ── blocked state ─────────────────────────────────────────────────────────
  /** The tool the daemon is waiting on. */
  pendingToolName?: string;
  onApprove?: () => void;
  onDeny?: () => void;
  onAlwaysAllow?: () => void;
  /** Disables the three answers while the POST is in flight. */
  answering?: boolean;

  // ── normal state ──────────────────────────────────────────────────────────
  value: string;
  onChange: (value: string) => void;
  onSend: () => void;
  sending?: boolean;
  steer?: ComposerSteerTarget | null;
  onClearSteer?: () => void;

  // ── model picker ──────────────────────────────────────────────────────────
  models: readonly ModelEntry[];
  model: string | null;
  modelStatus?: "ready" | "loading" | "error";
  modelPickerOpen: boolean;
  onToggleModelPicker: () => void;
  onCloseModelPicker: () => void;
  onPickModel: (modelId: string) => void;
  onManageProviders: () => void;
  modelNote?: string | null;

  /** `$0.0184`, or `null` while the daily rollup is unknown. */
  spend: string | null;
}

function HintRow({ left, spend }: { left: string; spend: string | null }) {
  return (
    <div className="mt-[9px] flex justify-between font-mono text-xs text-faint">
      <span>{left}</span>
      <span>{spend === null ? "" : `${spend} today`}</span>
    </div>
  );
}

export function Composer(props: ComposerProps) {
  const {
    blocked,
    pendingToolName = "This tool",
    onApprove,
    onDeny,
    onAlwaysAllow,
    answering = false,
    value,
    onChange,
    onSend,
    sending = false,
    steer = null,
    onClearSteer,
    models,
    model,
    modelStatus = "ready",
    modelPickerOpen,
    onToggleModelPicker,
    onCloseModelPicker,
    onPickModel,
    onManageProviders,
    modelNote = null,
    spend,
  } = props;

  const textarea = useRef<HTMLTextAreaElement>(null);

  // Auto-grow: reset first so the box can also shrink when text is deleted.
  useLayoutEffect(() => {
    const node = textarea.current;
    if (node === null) return;
    node.style.height = "auto";
    node.style.height = `${Math.min(node.scrollHeight, MAX_TEXTAREA_HEIGHT)}px`;
  }, [value, blocked]);

  if (blocked) {
    return (
      <div className="shrink-0 border-t border-line-subtle bg-main px-[26px] pt-[14px] pb-[20px]">
        <div className="mx-auto max-w-transcript">
          <div className="mb-[10px] flex items-center gap-[8px]">
            <span
              aria-hidden
              className="animate-pulse-oa-fast block h-[6px] w-[6px] shrink-0 rounded-full bg-red"
            />
            <span className="font-mono text-xs-plus tracking-label text-amber-ink">
              {pendingToolName} is waiting on you
            </span>
          </div>

          <div className="flex items-center gap-[8px] rounded-3xl border border-red bg-raised p-[8px] shadow-alert">
            <Button
              variant="primaryBlock"
              className="flex-1"
              disabled={answering}
              onClick={onApprove}
            >
              Approve
              <span aria-hidden className="font-mono text-sm opacity-55">
                ↵
              </span>
            </Button>
            <button
              type="button"
              disabled={answering}
              onClick={onDeny}
              className={cn(
                "inline-flex flex-1 cursor-pointer items-center justify-center gap-[6px] rounded-lg border border-line bg-transparent p-[11px] text-md leading-[normal] font-semibold text-ink",
                "transition-[background-color,border-color,color] duration-[120ms] hover:bg-muted",
                "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue",
                "disabled:pointer-events-none disabled:opacity-55",
              )}
            >
              Deny
              <span aria-hidden className="font-mono text-sm opacity-50">
                esc
              </span>
            </button>
            <button
              type="button"
              disabled={answering}
              onClick={onAlwaysAllow}
              className={cn(
                "cursor-pointer rounded-lg border-none bg-transparent px-[13px] py-[11px] text-base leading-[normal] text-tertiary",
                "transition-colors duration-[120ms] hover:text-ink",
                "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue",
                "disabled:pointer-events-none disabled:opacity-55",
              )}
            >
              Always allow
            </button>
          </div>

          <HintRow left="composer paused until answered" spend={spend} />
        </div>
      </div>
    );
  }

  const canSend = value.trim() !== "" && !sending;

  return (
    <div className="shrink-0 border-t border-line-subtle bg-main px-[26px] pt-[14px] pb-[20px]">
      <div className="mx-auto max-w-transcript">
        {steer !== null && (
          <div className="mb-[9px] flex items-center gap-[8px]">
            <span className="rounded-sm bg-amber-tint px-[7px] py-[2px] font-mono text-2xs tracking-label text-amber-ink uppercase">
              {steer.mode === "steer" ? "steering" : "follow-up"} →{" "}
              {steer.label}
            </span>
            <button
              type="button"
              onClick={onClearSteer}
              className="cursor-pointer border-none bg-transparent p-0 text-sm text-muted-fg hover:text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue"
            >
              send to assistant instead
            </button>
          </div>
        )}

        <div className="relative flex items-end gap-[9px] rounded-3xl border border-line bg-raised py-[7px] pr-[7px] pl-[11px] focus-within:border-line-hover">
          <textarea
            ref={textarea}
            rows={1}
            value={value}
            aria-label="Message"
            placeholder={composerPlaceholder(steer)}
            onChange={(event) => onChange(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.shiftKey) {
                event.preventDefault();
                if (canSend) onSend();
              }
            }}
            className="min-h-[24px] flex-1 resize-none border-none bg-transparent py-[8px] font-sans text-lg leading-[1.5] text-ink outline-none placeholder:text-faint"
          />

          <button
            type="button"
            title="Chat model"
            aria-haspopup="dialog"
            aria-expanded={modelPickerOpen}
            onClick={onToggleModelPicker}
            className="flex shrink-0 cursor-pointer items-center gap-[5px] rounded-md border border-line bg-main px-[9px] py-[8px] font-mono text-xs text-secondary hover:border-line-hover focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue"
          >
            {model ?? "model"}
            <span aria-hidden className="text-[8px] text-muted-fg">
              {modelPickerOpen ? "▴" : "▾"}
            </span>
          </button>

          <Button
            variant="primaryMd"
            className="shrink-0"
            disabled={!canSend}
            onClick={onSend}
          >
            Send
          </Button>

          {modelPickerOpen && (
            <ModelPicker
              models={models}
              selected={model}
              status={modelStatus}
              note={modelNote}
              onPick={onPickModel}
              onClose={onCloseModelPicker}
              onManageProviders={onManageProviders}
            />
          )}
        </div>

        <HintRow left="⏎ send · ⇧⏎ newline · ⌘K commands" spend={spend} />
      </div>
    </div>
  );
}
