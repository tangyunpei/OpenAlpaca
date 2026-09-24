/**
 * A unified patch between two versions of one note, built on this side.
 *
 * The daemon keeps a note's last five service-written revisions
 * (`.history/<id>/<rev>.md`, design §2) and serves each one's text through
 * `GET /v1/memories/{id}?revision=`, but no patch. A note is at most 32 KiB,
 * so a longest-common-subsequence over its lines is cheap, and the output is a
 * real unified patch — hunk headers, three lines of context — because the
 * renderer it feeds is the Library's own `DiffView`, which parses exactly that.
 * One diff view in the app, not two.
 */

type Kind = " " | "-" | "+";

interface Row {
  kind: Kind;
  text: string;
  /** The line's number in the old text (the next one, for an addition). */
  oldLine: number;
  newLine: number;
}

/** Past this many table cells the comparison is "all of it changed". */
const MAX_CELLS = 4_000_000;

const CONTEXT = 3;

function lines(text: string): string[] {
  if (text === "") return [];
  const body = text.endsWith("\n") ? text.slice(0, -1) : text;
  return body.split("\n");
}

/** Line edits turning `a` into `b`, removals before additions in a run. */
function edits(a: readonly string[], b: readonly string[]): [Kind, string][] {
  const n = a.length;
  const m = b.length;
  if (n * m > MAX_CELLS) {
    return [
      ...a.map((text): [Kind, string] => ["-", text]),
      ...b.map((text): [Kind, string] => ["+", text]),
    ];
  }
  const width = m + 1;
  // `lcs[i * width + j]` — the longest common run of `a[i..]` and `b[j..]`.
  const lcs = new Uint32Array((n + 1) * width);
  for (let i = n - 1; i >= 0; i -= 1) {
    for (let j = m - 1; j >= 0; j -= 1) {
      lcs[i * width + j] =
        a[i] === b[j]
          ? (lcs[(i + 1) * width + j + 1] ?? 0) + 1
          : Math.max(
              lcs[(i + 1) * width + j] ?? 0,
              lcs[i * width + j + 1] ?? 0,
            );
    }
  }
  const out: [Kind, string][] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    const left = a[i] ?? "";
    const right = b[j] ?? "";
    if (left === right) {
      out.push([" ", left]);
      i += 1;
      j += 1;
    } else if (
      (lcs[(i + 1) * width + j] ?? 0) >= (lcs[i * width + j + 1] ?? 0)
    ) {
      out.push(["-", left]);
      i += 1;
    } else {
      out.push(["+", right]);
      j += 1;
    }
  }
  for (; i < n; i += 1) out.push(["-", a[i] ?? ""]);
  for (; j < m; j += 1) out.push(["+", b[j] ?? ""]);
  return out;
}

/**
 * `before` → `after` as a unified patch, or `""` when the two are the same
 * text. Lines are compared whole; a trailing newline does not count as a line.
 */
export function unifiedPatch(before: string, after: string): string {
  let oldLine = 1;
  let newLine = 1;
  const rows: Row[] = edits(lines(before), lines(after)).map(([kind, text]) => {
    const row = { kind, text, oldLine, newLine };
    if (kind !== "+") oldLine += 1;
    if (kind !== "-") newLine += 1;
    return row;
  });

  const changed = rows.flatMap((row, index) =>
    row.kind === " " ? [] : [index],
  );
  if (changed.length === 0) return "";

  // Merge changes whose context would overlap or touch into one hunk.
  const hunks: [number, number][] = [];
  const last = rows.length - 1;
  let start = -1;
  let end = -1;
  for (const index of changed) {
    if (start >= 0 && index - CONTEXT <= end + 1) {
      end = Math.min(last, index + CONTEXT);
      continue;
    }
    if (start >= 0) hunks.push([start, end]);
    start = Math.max(0, index - CONTEXT);
    end = Math.min(last, index + CONTEXT);
  }
  hunks.push([start, end]);

  const out: string[] = [];
  for (const [from, to] of hunks) {
    const slice = rows.slice(from, to + 1);
    const first = slice[0];
    if (first === undefined) continue;
    const oldCount = slice.filter((row) => row.kind !== "+").length;
    const newCount = slice.filter((row) => row.kind !== "-").length;
    // A hunk with no lines on one side names the line *before* it, as `diff -u`.
    const oldStart = oldCount === 0 ? first.oldLine - 1 : first.oldLine;
    const newStart = newCount === 0 ? first.newLine - 1 : first.newLine;
    out.push(`@@ -${oldStart},${oldCount} +${newStart},${newCount} @@`);
    for (const row of slice) out.push(`${row.kind}${row.text}`);
  }
  return `${out.join("\n")}\n`;
}
