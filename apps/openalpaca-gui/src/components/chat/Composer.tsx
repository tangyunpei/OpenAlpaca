/**
 * `Composer` (DESIGN_SPEC §3.16) — two mutually exclusive states.
 *
 * The blocked state is the important one: while a tool confirmation is
 * pending the textarea is **not rendered at all** (§3.16a). The action bar
 * replaces it, which is also why the global `Enter → approve` binding in §4.5
 * can never collide with `Enter → send`.
 *
 * `Always allow` sends `approval_scope: "entire_tool"`, which the daemon
 * caches for the rest of the session (GAP-01, closed). The caller
 * (`useChatSession`) is responsible for the toast that confirms it.
 *
 * **Attaching files (U5).** §5.6's row said attachments were absent from the
 * design and named the shape to build: "a ghost xs button left of the model
 * button". That is exactly what this is — `Attach` opens a hidden
 * `<input type="file" multiple>`, which works in the Tauri WKWebView and in
 * the browser preview alike and needs no Tauri plugin and no capability (G15
 * removed the fs plugin on purpose). Picked files become chips above the
 * textarea; the upload itself is the caller's, so this component stays a
 * rendering of what it is handed.
 *
 * Two more ways in, both the same path: pasting a file (an image off the
 * clipboard) and dropping one on the composer. The drop needs no config —
 * `tauri.conf.json` already sets `dragDropEnabled: false`, so the webview sees
 * ordinary HTML drag events — and it draws no drop zone, because §5.6 rules
 * one out and the chips are the feedback.
 */

import { useLayoutEffect, useRef } from "react";

import { Button } from "@/components/ui";
import { cn } from "@/lib/cn";
import { dragCarriesFiles } from "@/lib/drag";
import { formatFileSize, type ModelEntry } from "@/lib/api/types";
import type { ComposerMode } from "@/stores/ui";

import { hasUploadInFlight, type DraftAttachment } from "./attachments";
import { ModelPicker } from "./ModelPicker";

/** Auto-grow ceiling: past this the textarea scrolls instead of pushing the transcript out. */
const MAX_TEXTAREA_HEIGHT = 220;

/**
 * The narrowest the message box may get before the controls beside it give way
 * (T2).
 *
 * On a 1080-point window the row held a textarea 8px wide — "A / s / k" stacked
 * one letter per line — because `flex-1` will happily shrink to nothing while
 * Attach, a 24-character local model id and Send all refuse to. This is the
 * floor that makes the row wrap instead: below it the controls take a line of
 * their own and the textarea keeps the full width.
 */
const MIN_TEXTAREA_WIDTH = 180;

/**
 * How much of a model id the button shows before eliding it.
 *
 * A local id (`qwen3.8:27b-mtp-q8_0`) is two to three times the length of a
 * hosted one and was the single widest thing in the row. The full id stays on
 * the label's own `title`, and the picker shows it in full.
 */
const MAX_MODEL_LABEL_WIDTH = 150;

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

  // ── attachments (U5) ──────────────────────────────────────────────────────
  /** The draft chips, in pick order. Empty — the default — draws no strip. */
  attachments?: readonly DraftAttachment[];
  /** Files picked, pasted or dropped. The caller uploads them. */
  onAttachFiles?: (files: readonly File[]) => void;
  onRemoveAttachment?: (key: string) => void;
  /** The refusal for a pick that never became a chip (the 11th file). */
  attachmentError?: string | null;

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

/**
 * One draft attachment.
 *
 * The three states read differently on purpose: `uploading…` is a wait, `✓` is
 * a file the daemon holds, and a failure prints the **daemon's** sentence
 * ("MIME type 'application/zip' is not allowed", the size cap) rather than a
 * house word for it — the reader has to know which rule they hit.
 */
function AttachmentChip({
  item,
  onRemove,
}: {
  item: DraftAttachment;
  onRemove?: () => void;
}) {
  const failed = item.state === "failed";
  return (
    <li
      className={cn(
        "flex max-w-full items-center gap-[7px] rounded-md border px-[8px] py-[4px]",
        failed ? "border-red-line bg-red-tint" : "border-line bg-muted",
      )}
    >
      <span className="truncate font-mono text-xs text-secondary">
        {item.name}
      </span>
      <span className="shrink-0 font-mono text-2xs text-faint">
        {formatFileSize(item.size)}
      </span>
      {item.state === "uploading" && (
        <span className="shrink-0 font-mono text-2xs text-faint">
          uploading…
        </span>
      )}
      {item.state === "ready" && (
        <span
          role="img"
          aria-label="ready"
          className="shrink-0 font-mono text-2xs text-tertiary"
        >
          ✓
        </span>
      )}
      {failed && (
        <span className="font-mono text-2xs text-red-ink">
          {item.error ?? "Upload failed"}
        </span>
      )}
      {onRemove !== undefined && (
        <button
          type="button"
          aria-label={`Remove ${item.name}`}
          onClick={onRemove}
          className="shrink-0 cursor-pointer border-none bg-transparent p-0 font-mono text-2xs text-muted-fg hover:text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue"
        >
          ✗
        </button>
      )}
    </li>
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
    attachments = [],
    onAttachFiles,
    onRemoveAttachment,
    attachmentError = null,
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
  const fileInput = useRef<HTMLInputElement>(null);

  /** Anything the browser handed us as files, on any of the three paths. */
  const attach = (files: FileList | null | undefined): boolean => {
    const picked = Array.from(files ?? []);
    if (picked.length === 0 || onAttachFiles === undefined) return false;
    onAttachFiles(picked);
    return true;
  };

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

          {/* Wraps for the same reason the composer's own row does (T2): three
              answers side by side do not fit a half-screen window. */}
          <div className="flex flex-wrap items-center gap-[8px] rounded-3xl border border-red bg-raised p-[8px] shadow-alert">
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

  // An upload still in flight has no `file_id` yet, so a send would silently
  // drop it. Wait for it rather than send a turn missing a file the user
  // attached.
  const uploading = hasUploadInFlight(attachments);
  const canSend = value.trim() !== "" && !sending && !uploading;

  return (
    <div
      className="shrink-0 border-t border-line-subtle bg-main px-[26px] pt-[14px] pb-[20px]"
      onDragOver={(event) => {
        // Only a *file* drag is ours: cancelling a text drag's default here
        // would break dropping a selection into the textarea.
        if (onAttachFiles !== undefined && dragCarriesFiles(event.dataTransfer))
          event.preventDefault();
      }}
      onDrop={(event) => {
        if (attach(event.dataTransfer.files)) event.preventDefault();
      }}
    >
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

        {attachmentError !== null && (
          <p
            role="alert"
            className="mb-[8px] font-mono text-2xs-plus text-red-ink"
          >
            {attachmentError}
          </p>
        )}

        {attachments.length > 0 && (
          <ul
            aria-label="Attachments"
            className="mb-[8px] flex list-none flex-wrap gap-[6px] p-0"
          >
            {attachments.map((item) => (
              <AttachmentChip
                key={item.key}
                item={item}
                onRemove={
                  onRemoveAttachment === undefined
                    ? undefined
                    : () => onRemoveAttachment(item.key)
                }
              />
            ))}
          </ul>
        )}

        {/* `flex-wrap`, and the three controls as one item, is the whole of
            T2's composer rule: while the row can hold the textarea's floor and
            the cluster it looks exactly as the design draws it, and when it
            cannot the cluster takes a line of its own rather than squeezing
            the message box to nothing. */}
        <div className="relative flex flex-wrap items-end gap-[9px] rounded-3xl border border-line bg-raised py-[7px] pr-[7px] pl-[11px] focus-within:border-line-hover">
          <textarea
            ref={textarea}
            rows={1}
            value={value}
            aria-label="Message"
            placeholder={composerPlaceholder(steer)}
            onChange={(event) => onChange(event.target.value)}
            onPaste={(event) => {
              // A pasted image is an attachment, not text: take it, and only
              // then stop the default so an ordinary text paste is untouched.
              if (attach(event.clipboardData.files)) event.preventDefault();
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.shiftKey) {
                event.preventDefault();
                if (canSend) onSend();
              }
            }}
            style={{ minWidth: MIN_TEXTAREA_WIDTH }}
            className="min-h-[24px] flex-1 resize-none border-none bg-transparent py-[8px] font-sans text-lg leading-[1.5] text-ink outline-none placeholder:text-faint"
          />

          <div className="ml-auto flex shrink-0 items-end gap-[9px]">
            {onAttachFiles !== undefined && (
              <>
                {/* Hidden, not absent: the button is the control, and this is
                    the only thing that can open a file picker. `value` is
                    cleared after every pick so the same file can be attached
                    twice in a row. */}
                <input
                  ref={fileInput}
                  type="file"
                  multiple
                  hidden
                  onChange={(event) => {
                    attach(event.target.files);
                    event.target.value = "";
                  }}
                />
                {/* Deliberately never disabled at the cap: the 11th pick has to
                    be *refused with a sentence*, and a dead button says
                    nothing. */}
                <Button
                  variant="ghostXs"
                  className="shrink-0 self-end py-[7px]"
                  title="Attach files"
                  onClick={() => fileInput.current?.click()}
                >
                  Attach
                </Button>
              </>
            )}

            <button
              type="button"
              title="Chat model"
              aria-haspopup="dialog"
              aria-expanded={modelPickerOpen}
              onClick={onToggleModelPicker}
              className="flex min-w-0 shrink cursor-pointer items-center gap-[5px] rounded-md border border-line bg-main px-[9px] py-[8px] font-mono text-xs text-secondary hover:border-line-hover focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue"
            >
              {/* Elided, never wrapped: the full id is on the label's own
                  `title` and in the picker. */}
              <span
                className="truncate"
                style={{ maxWidth: MAX_MODEL_LABEL_WIDTH }}
                {...(model === null ? {} : { title: model })}
              >
                {model ?? "model"}
              </span>
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
          </div>

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
