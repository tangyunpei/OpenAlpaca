/**
 * `x-workspace-path` — the one place this window encodes a project path for the
 * wire (ruling R81).
 *
 * An HTTP header value is bytes, not a JS string: `fetch` throws a `TypeError`
 * on a value holding anything above `\xff`, and the daemon's own read
 * (`HeaderValue::to_str`) refuses anything above `\x7f`. A project at
 * `~/项目/openalpaca` is an ordinary path, so the value is percent-encoded UTF-8
 * and the daemon decodes it.
 *
 * Only what must be escaped is escaped — non-ASCII, the control range, and `%`
 * itself (otherwise a literal `%20` in a directory name would decode to a
 * space). `/Users/me/repo` is therefore its own encoding, byte for byte, which
 * is what keeps a plain ASCII value readable in a request log.
 */
export function encodeWorkspacePath(path: string): string {
  let encoded = "";
  for (const char of path) {
    const code = char.codePointAt(0) ?? 0;
    if (char === "%" || code < 0x20 || code > 0x7e) {
      for (const byte of new TextEncoder().encode(char)) {
        encoded += `%${byte.toString(16).toUpperCase().padStart(2, "0")}`;
      }
    } else {
      encoded += char;
    }
  }
  return encoded;
}

/**
 * The header pair a request carries for `path`, ready to spread into `headers`.
 *
 * `null`, `undefined` and `""` are all "this window has no project", which sends
 * no header at all rather than an empty one — the daemon reads a missing header
 * and an empty one the same way, and the senders that took a truthy check before
 * this existed behaved exactly so.
 */
export function workspaceHeader(
  path: string | null | undefined,
): Record<string, string> | undefined {
  if (!path) return undefined;
  return { "x-workspace-path": encodeWorkspacePath(path) };
}
