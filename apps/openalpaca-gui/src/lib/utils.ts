/**
 * Shared utility helpers for the OpenAlpaca GUI.
 */

/** Format ISO timestamp to HH:MM:SS */
export function formatTime(ts: string): string {
  const date = new Date(ts);
  return date.toLocaleTimeString("en-US", {
    hour12: false,
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

/** Format ISO timestamp to date + time */
export function formatDateTime(ts: string): string {
  const date = new Date(ts);
  return date.toLocaleString("en-US", {
    hour12: false,
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

/** Format seconds into human-readable duration */
export function formatDuration(seconds: number): string {
  if (seconds < 60) return `${Math.round(seconds)}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${Math.round(seconds % 60)}s`;
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  return `${h}h ${m}m`;
}

/** Map a status string to a CSS class name */
export function getStatusColor(status: string): string {
  switch (status) {
    case "active":
    case "idle":
    case "completed":
    case "running":
      return "status-success";
    case "busy":
    case "queued":
    case "pending":
      return "status-warning";
    case "paused":
    case "waiting":
      return "status-paused";
    case "error":
    case "failed":
    case "cancelled":
      return "status-error";
    default:
      return "status-dim";
  }
}

/** Map icon shortcode names (from agent TOML) to emoji characters */
const iconMap: Record<string, string> = {
  magnifying_glass: "\u{1F50D}",
  pencil: "\u{270F}\uFE0F",
  pen: "\u{1F58A}\uFE0F",
  writing_hand: "\u{270D}\uFE0F",
  gear: "\u{2699}\uFE0F",
  wrench: "\u{1F527}",
  hammer: "\u{1F528}",
  robot: "\u{1F916}",
  brain: "\u{1F9E0}",
  bulb: "\u{1F4A1}",
  light_bulb: "\u{1F4A1}",
  chart: "\u{1F4CA}",
  bar_chart: "\u{1F4CA}",
  globe: "\u{1F310}",
  globe_with_meridians: "\u{1F310}",
  shield: "\u{1F6E1}\uFE0F",
  lock: "\u{1F512}",
  key: "\u{1F511}",
  file_folder: "\u{1F4C1}",
  computer: "\u{1F4BB}",
  desktop_computer: "\u{1F5A5}\uFE0F",
  terminal: "\u{1F4BB}",
  microscope: "\u{1F52C}",
  telescope: "\u{1F52D}",
  book: "\u{1F4D6}",
  books: "\u{1F4DA}",
  memo: "\u{1F4DD}",
  clipboard: "\u{1F4CB}",
  package: "\u{1F4E6}",
  rocket: "\u{1F680}",
  star: "\u{2B50}",
  zap: "\u{26A1}",
  fire: "\u{1F525}",
  eye: "\u{1F441}\uFE0F",
  speech_balloon: "\u{1F4AC}",
  thought_balloon: "\u{1F4AD}",
  art: "\u{1F3A8}",
  palette: "\u{1F3A8}",
  musical_note: "\u{1F3B5}",
  camera: "\u{1F4F7}",
  link: "\u{1F517}",
  mailbox: "\u{1F4EC}",
  bell: "\u{1F514}",
  hourglass: "\u{231B}",
  stopwatch: "\u{23F1}\uFE0F",
  calendar: "\u{1F4C5}",
  pushpin: "\u{1F4CC}",
  scissors: "\u{2702}\uFE0F",
  broom: "\u{1F9F9}",
  test_tube: "\u{1F9EA}",
  dna: "\u{1F9EC}",
  pill: "\u{1F48A}",
  stethoscope: "\u{1FA7A}",
  scales: "\u{2696}\uFE0F",
  detective: "\u{1F575}\uFE0F",
  astronaut: "\u{1F9D1}\u200D\u{1F680}",
  construction_worker: "\u{1F477}",
  teacher: "\u{1F9D1}\u200D\u{1F3EB}",
  scientist: "\u{1F9D1}\u200D\u{1F52C}",
  technologist: "\u{1F9D1}\u200D\u{1F4BB}",
  artist: "\u{1F9D1}\u200D\u{1F3A8}",
};

/** Resolve an icon field to a displayable emoji. Falls back to robot emoji. */
export function resolveIcon(icon: string | null | undefined): string {
  if (!icon) return "\u{1F916}";
  // If already an emoji (starts with non-ASCII), return as-is
  if (icon.codePointAt(0)! > 127) return icon;
  return iconMap[icon] ?? iconMap[icon.toLowerCase().replace(/ /g, "_")] ?? "\u{1F916}";
}

/** Safely parse JSON with a typed fallback */
export function safeParseJson<T>(json: string | null | undefined, fallback: T): T {
  if (!json) return fallback;
  try {
    return JSON.parse(json) as T;
  } catch {
    return fallback;
  }
}
