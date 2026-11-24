/**
 * The dev server's half of `VITE_OPENALPACA_HOST=dev-server` (Q07): a Vite
 * plugin that lets a plain browser on the dev server's page reach an
 * isolated development daemon. The page's half is
 * `src/lib/host/dev-server.ts` (DevServerHost).
 *
 * Registered by `vite.config.ts` only when `VITE_OPENALPACA_HOST` is
 * `dev-server` and not under vitest; it runs on the dev server alone
 * (`apply: "serve"`), never in a build, and turns Vite's CORS off.
 *
 * Two endpoints, each answering a POST only:
 *
 *   `/__openalpaca/connection` — the store's `state/discovery.json`, read at
 *                                each request (so a token the daemon rotates
 *                                is followed), answered in the shell's form:
 *                                `baseUrl`, `token`, `instanceId`,
 *                                `version`, `protocol`, `daemonPath: null`,
 *                                `note: null`;
 *   `/__openalpaca/log-tail`   — the last `{ lines }` lines of the store's
 *                                `state/logs/daemon.log`, at most 64 KB from
 *                                its end, no line cut mid-way, colour
 *                                escapes removed.
 *
 * Every request is refused unless its `Origin` is exactly the dev server's
 * own (`http://localhost:1420` or `http://127.0.0.1:1420`: a browser sends
 * one on every POST), 403, and unless `OPENALPACA_HOME_STORE` is set and
 * absolute — there is no fallback to the home store — 500; both with a
 * sentence and no kind. No answer grants CORS. A store's refusals carry the
 * shell's kinds, 503: `not_running` (no file), `discovery_unreadable` (one
 * that cannot be read or is not a daemon's), `expired` (its token), and
 * `log_unreadable`.
 *
 * A development aid, not the shell: it judges the token's expiry only — no
 * protocol mismatch, no process check, no health probe — and spawns no
 * daemon. Its sentences are its own, not the shell's. Of the shell's log
 * read it does not mirror the run offset (`from`), and it decodes the bytes
 * as UTF-8 with Node's replacement rather than Rust's lossy one.
 */

import { open, readFile } from "node:fs/promises";
import type { IncomingMessage, ServerResponse } from "node:http";
import { isAbsolute, join } from "node:path";

import type { Connect, Plugin } from "vite";

const CONNECTION_ENDPOINT = "/__openalpaca/connection";
const LOG_TAIL_ENDPOINT = "/__openalpaca/log-tail";
const ALLOWED_ORIGINS: ReadonlySet<string> = new Set([
  "http://localhost:1420",
  "http://127.0.0.1:1420",
]);
/** What the shell's log read takes from the end (`LOG_TAIL_READ_BYTES`). */
const LOG_TAIL_READ_BYTES = 64 * 1024;
/** The most of a request body read: `{ "lines": n }` is a few bytes. */
const BODY_LIMIT_BYTES = 4 * 1024;

export function devServerHost(): Plugin {
  return {
    name: "openalpaca-dev-server-host",
    apply: "serve",
    config: () => ({ server: { cors: false } }),
    configureServer(server) {
      server.middlewares.use(handle);
    },
  };
}

/** A JSON answer, with no CORS header. */
type Answer = { status: number; body: unknown };

const handle: Connect.NextHandleFunction = (request, response, next) => {
  const path = (request.url ?? "").split("?")[0];
  if (
    request.method !== "POST" ||
    (path !== CONNECTION_ENDPOINT && path !== LOG_TAIL_ENDPOINT)
  ) {
    next();
    return;
  }
  void answer(path, request).then((result) => send(response, result));
};

async function answer(path: string, request: IncomingMessage): Promise<Answer> {
  const origin = request.headers.origin;
  if (typeof origin !== "string" || !ALLOWED_ORIGINS.has(origin)) {
    return {
      status: 403,
      body: {
        message:
          "The dev server answers its own page only: this request's Origin is not http://localhost:1420 or http://127.0.0.1:1420.",
      },
    };
  }
  const store = process.env.OPENALPACA_HOME_STORE;
  if (store === undefined || store === "" || !isAbsolute(store)) {
    return {
      status: 500,
      body: {
        message:
          "The dev server has no development store: start it with OPENALPACA_HOME_STORE set to an absolute path.",
      },
    };
  }
  if (path === CONNECTION_ENDPOINT) return connection(store);
  return logTail(store, await bodyOf(request));
}

function send(response: ServerResponse, { status, body }: Answer): void {
  response.statusCode = status;
  response.setHeader("Content-Type", "application/json");
  response.end(JSON.stringify(body));
}

function refused(kind: string, message: string): Answer {
  return { status: 503, body: { kind, message } };
}

/**
 * The request's body as JSON; `undefined` when it is none, too long, or a
 * stream that failed.
 */
async function bodyOf(request: IncomingMessage): Promise<unknown> {
  let text = "";
  try {
    for await (const chunk of request) {
      text += String(chunk);
      if (text.length > BODY_LIMIT_BYTES) return undefined;
    }
  } catch {
    return undefined;
  }
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return undefined;
  }
}

async function connection(store: string): Promise<Answer> {
  const file = join(store, "state", "discovery.json");
  let text: string;
  try {
    text = await readFile(file, "utf8");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") {
      return refused(
        "not_running",
        `No discovery file at ${file}: the development daemon may not be running.`,
      );
    }
    return refused(
      "discovery_unreadable",
      `Could not read ${file}: ${(error as Error).message}`,
    );
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return refused("discovery_unreadable", `${file} is not JSON.`);
  }
  const d = (parsed ?? {}) as Record<string, unknown>;
  const listen = (d.listen ?? {}) as Record<string, unknown>;
  const auth = (d.auth ?? {}) as Record<string, unknown>;
  const build = (d.build ?? {}) as Record<string, unknown>;
  const expiresAt =
    typeof auth.expires_at === "string" ? Date.parse(auth.expires_at) : NaN;
  if (
    typeof listen.host !== "string" ||
    typeof listen.port !== "number" ||
    typeof auth.token !== "string" ||
    typeof d.instance_id !== "string" ||
    Number.isNaN(expiresAt)
  ) {
    return refused("discovery_unreadable", `${file} does not name a daemon.`);
  }
  if (Date.now() >= expiresAt) {
    return refused("expired", `The token in ${file} has expired.`);
  }
  return {
    status: 200,
    body: {
      baseUrl: `http://${listen.host}:${listen.port}`,
      token: auth.token,
      instanceId: d.instance_id,
      version: typeof build.version === "string" ? build.version : null,
      protocol: typeof build.protocol === "number" ? build.protocol : null,
      daemonPath: null,
      note: null,
    },
  };
}

async function logTail(store: string, body: unknown): Promise<Answer> {
  const file = join(store, "state", "logs", "daemon.log");
  const lines = (body as { lines?: unknown } | undefined)?.lines;
  if (typeof lines !== "number" || !Number.isInteger(lines) || lines < 0) {
    return refused(
      "log_unreadable",
      `Could not read ${file}: the request did not say how many lines.`,
    );
  }
  let opened;
  try {
    opened = await open(file, "r");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") {
      return { status: 200, body: "" };
    }
    return refused(
      "log_unreadable",
      `Could not read ${file}: ${(error as Error).message}`,
    );
  }
  try {
    const { size } = await opened.stat();
    const start = Math.max(0, size - LOG_TAIL_READ_BYTES);
    const buffer = Buffer.alloc(size - start);
    await opened.read(buffer, 0, buffer.length, start);
    let text = buffer.toString("utf8");
    if (start > 0) {
      const newline = text.indexOf("\n");
      text = newline === -1 ? "" : text.slice(newline + 1);
    }
    return { status: 200, body: tailOf(text, lines) };
  } catch (error) {
    return refused(
      "log_unreadable",
      `Could not read ${file}: ${(error as Error).message}`,
    );
  } finally {
    // The answer is already made; a failed close changes nothing in it.
    await opened.close().catch(() => undefined);
  }
}

/**
 * The last `lines` lines of `text`, newest last, as the shell's `log_tail`
 * gives them: colour escapes removed, trailing blank lines and the window's
 * leading ones dropped, nothing else changed.
 */
function tailOf(text: string, lines: number): string {
  const cleaned = text
    .split("\n")
    .map((line) => stripAnsi(line.endsWith("\r") ? line.slice(0, -1) : line));
  if (text.endsWith("\n")) cleaned.pop();
  let end = cleaned.length;
  while (end > 0 && cleaned[end - 1].trim() === "") end -= 1;
  const window = cleaned.slice(Math.max(0, end - lines), end);
  const first = window.findIndex((line) => line.trim() !== "");
  return (first === -1 ? [] : window.slice(first)).join("\n");
}

/** `line` without its ANSI CSI sequences (`ESC [` … a final byte `@`..`~`). */
function stripAnsi(line: string): string {
  // Everything up to and with the final byte goes; a sequence the line
  // ends inside is dropped whole, as the shell does.
  return line.replace(/\x1b\[[^\x40-\x7e]*[\x40-\x7e]?/g, "");
}
