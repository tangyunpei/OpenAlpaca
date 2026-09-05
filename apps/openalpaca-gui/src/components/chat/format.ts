/**
 * Transcript formatting (DESIGN_SPEC §3.10, §3.12).
 *
 * Every string here is derived from something the daemon actually sent. The
 * assistant meta line maps 1:1 onto the SSE `done` payload —
 * `model · duration_ms → "3.8s" · tokens_in/tokens_out → "1284/612 tok"` — so
 * a missing field drops its segment rather than printing a zero.
 *
 * Clock times are formatted from the parts rather than `toLocaleTimeString`:
 * the design's `14:22` is 24-hour and zero-padded, and a test must not depend
 * on the machine's locale.
 */

/** Vendor prefixes the design strips: `claude-sonnet-4-6` reads as `sonnet-4-6`. */
const VENDOR_PREFIXES = ["claude-", "anthropic-", "openai-", "gpt-oss-"];

/** `claude-sonnet-4-6` → `sonnet-4-6`; anything unrecognised is left alone. */
export function shortModelId(model: string): string {
  const id = model.includes("/")
    ? (model.slice(model.lastIndexOf("/") + 1) ?? model)
    : model;
  for (const prefix of VENDOR_PREFIXES) {
    if (id.startsWith(prefix) && id.length > prefix.length) {
      return id.slice(prefix.length);
    }
  }
  return id;
}

function pad2(value: number): string {
  return value < 10 ? `0${value}` : String(value);
}

/** `2026-08-31T14:22:41Z` → `14:22`. Invalid input yields `null`, never a guess. */
export function formatClock(
  value: string | Date | null | undefined,
): string | null {
  if (value === null || value === undefined) return null;
  const date = value instanceof Date ? value : new Date(value);
  if (Number.isNaN(date.getTime())) return null;
  return `${pad2(date.getHours())}:${pad2(date.getMinutes())}`;
}

/** The header's date meta — `Mon 31 Aug`. */
export function formatHeaderDate(value: Date = new Date()): string {
  const days = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
  const months = [
    "Jan",
    "Feb",
    "Mar",
    "Apr",
    "May",
    "Jun",
    "Jul",
    "Aug",
    "Sep",
    "Oct",
    "Nov",
    "Dec",
  ];
  return `${days[value.getDay()] ?? ""} ${value.getDate()} ${months[value.getMonth()] ?? ""}`;
}

/**
 * `3800 → "3.8s"`, `372000 → "6m 12s"`. Seconds are zero-padded above a
 * minute, matching the design's `11m 04s`.
 */
export function formatDurationMs(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "0.0s";
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  const totalSeconds = Math.round(ms / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  if (minutes < 60) return `${minutes}m ${pad2(totalSeconds % 60)}s`;
  return `${Math.floor(minutes / 60)}h ${pad2(minutes % 60)}m`;
}

/** Wall clock between two ISO stamps, or `null` when either is unusable. */
export function formatElapsed(
  fromIso: string | null | undefined,
  toIso: string | null | undefined,
): string | null {
  if (!fromIso || !toIso) return null;
  const from = new Date(fromIso).getTime();
  const to = new Date(toIso).getTime();
  if (Number.isNaN(from) || Number.isNaN(to) || to < from) return null;
  return formatDurationMs(to - from);
}

export interface AssistantMeta {
  model?: string;
  durationMs?: number;
  tokensIn?: number;
  tokensOut?: number;
}

/**
 * The assistant header's meta line. Segments are omitted, never zeroed: the
 * daemon leaves fields off a `done` frame it has nothing to say about.
 */
export function assistantMetaLine(meta: AssistantMeta): string | null {
  const parts: string[] = [];
  if (meta.model !== undefined && meta.model !== "") {
    parts.push(shortModelId(meta.model));
  }
  if (meta.durationMs !== undefined) {
    parts.push(formatDurationMs(meta.durationMs));
  }
  if (meta.tokensIn !== undefined || meta.tokensOut !== undefined) {
    parts.push(`${meta.tokensIn ?? 0}/${meta.tokensOut ?? 0} tok`);
  }
  return parts.length === 0 ? null : parts.join(" · ");
}

/** `b41c8e02-…` → `b41c8e02`: the 8-hex id the design prints. */
export function shortRunId(taskId: string): string {
  const head = taskId.split("-")[0] ?? taskId;
  return head.slice(0, 8);
}

/**
 * The design's `Steer {run.short} mid-run…` placeholder wants two or three
 * words, and a `Task` has only one full-sentence `title` (there is no `short`
 * on the wire). Truncating the real title is honest; inventing a short one is
 * not.
 */
export function shortTitle(title: string, words = 3): string {
  const trimmed = title.trim();
  if (trimmed === "") return "the run";
  const parts = trimmed.split(/\s+/);
  if (parts.length <= words) return trimmed;
  return `${parts.slice(0, words).join(" ")}…`;
}

/**
 * The literal tool argument shown in the confirmation card's command box
 * (§3.14). A shell tool sends `{ command: "cargo tree" }`; anything else is
 * pretty-printed rather than flattened, because the user is being asked to
 * approve exactly these bytes.
 */
const COMMAND_KEYS = ["command", "cmd", "script", "query", "path", "url"];

export function formatToolArguments(args: unknown): string {
  if (args === null || args === undefined) return "";
  if (typeof args === "string") return args;
  if (typeof args !== "object") return String(args);

  const record = args as Record<string, unknown>;
  const keys = Object.keys(record);
  if (keys.length === 1) {
    const only = keys[0];
    if (only !== undefined && COMMAND_KEYS.includes(only)) {
      const value = record[only];
      if (typeof value === "string") return value;
    }
  }
  try {
    return JSON.stringify(args, null, 2);
  } catch {
    return String(args);
  }
}
