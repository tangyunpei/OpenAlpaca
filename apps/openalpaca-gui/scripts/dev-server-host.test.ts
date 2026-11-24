/**
 * The dev server's half of `VITE_OPENALPACA_HOST=dev-server` (Q07,
 * `scripts/dev-server-host.ts`): two POST endpoints that read an isolated
 * development store's `discovery.json` and `daemon.log` for a plain browser.
 *
 * The middleware is driven directly, with a fake request and response: no
 * server and no port. Every store here is a scratch directory this file
 * makes and removes; nothing reads the real home.
 */

import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import type { IncomingMessage, ServerResponse } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Readable } from "node:stream";

import type { Connect, UserConfig, ViteDevServer } from "vite";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { devServerHost } from "./dev-server-host";

/** What the middleware did with one request. */
type Outcome =
  | { passed: true }
  | {
      passed: false;
      status: number;
      headers: Record<string, string>;
      body: unknown;
    };

/** The middleware the plugin adds to the dev server. */
function middleware(): Connect.NextHandleFunction {
  const added: Connect.NextHandleFunction[] = [];
  const server = {
    middlewares: {
      use: (handler: Connect.NextHandleFunction) => added.push(handler),
    },
  } as unknown as ViteDevServer;
  const hook = devServerHost().configureServer;
  const configure = (typeof hook === "function" ? hook : hook?.handler) as (
    server: ViteDevServer,
  ) => void;
  configure(server);
  expect(added).toHaveLength(1);
  return added[0];
}

/** Send one request through the middleware and say what came of it. */
function send(
  url: string,
  {
    method = "POST",
    origin = "http://localhost:1420",
    body = "{}",
  }: { method?: string; origin?: string | null; body?: string } = {},
): Promise<Outcome> {
  const request = Readable.from([
    Buffer.from(body),
  ]) as unknown as IncomingMessage;
  Object.assign(request, {
    method,
    url,
    headers: origin === null ? {} : { origin },
  });
  return new Promise((resolve) => {
    const headers: Record<string, string> = {};
    const response = {
      statusCode: 200,
      setHeader(name: string, value: string) {
        headers[name.toLowerCase()] = value;
        return response;
      },
      end(chunk: string) {
        resolve({
          passed: false,
          status: response.statusCode,
          headers,
          body: JSON.parse(chunk),
        });
      },
    };
    void middleware()(request, response as unknown as ServerResponse, () =>
      resolve({ passed: true }),
    );
  });
}

/** The answer of a request the middleware answered, with no CORS grant. */
async function answered(outcome: Promise<Outcome>) {
  const result = await outcome;
  if (result.passed) throw new Error("passed on; an answer was expected");
  expect(result.headers).not.toHaveProperty("access-control-allow-origin");
  expect(result.headers["content-type"]).toBe("application/json");
  return { status: result.status, body: result.body };
}

let root = "";

/** A daemon's `discovery.json` in the scratch store. */
async function discovery(
  overrides: { token?: string; expiresAt?: string; instanceId?: string } = {},
): Promise<void> {
  await mkdir(join(root, "state"), { recursive: true });
  await writeFile(
    join(root, "state", "discovery.json"),
    JSON.stringify({
      listen: { host: "127.0.0.1", port: 51823 },
      auth: {
        token: overrides.token ?? "tok-1",
        expires_at: overrides.expiresAt ?? "2999-01-01T00:00:00Z",
      },
      instance_id: overrides.instanceId ?? "i-1",
      pid: 4242,
      build: { version: "0.1.0", protocol: 1 },
    }),
  );
}

/** `text` as the scratch store's `daemon.log`. */
async function log(text: string | Buffer): Promise<void> {
  await mkdir(join(root, "state", "logs"), { recursive: true });
  await writeFile(join(root, "state", "logs", "daemon.log"), text);
}

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), "openalpaca-dev-server-host-"));
  vi.stubEnv("OPENALPACA_HOME_STORE", root);
});

afterEach(async () => {
  vi.unstubAllEnvs();
  await rm(root, { recursive: true, force: true });
});

describe("the dev server's host plugin", () => {
  it("runs on the dev server only, with CORS off", () => {
    const plugin = devServerHost();
    expect(plugin.apply).toBe("serve");
    const config = (
      typeof plugin.config === "function"
        ? plugin.config
        : plugin.config?.handler
    ) as () => UserConfig;
    expect(config().server?.cors).toBe(false);
  });

  it("answers the connection to a POST from the dev server's own origin, either spelling", async () => {
    await discovery();
    for (const origin of ["http://localhost:1420", "http://127.0.0.1:1420"]) {
      expect(
        await answered(send("/__openalpaca/connection", { origin })),
      ).toEqual({
        status: 200,
        body: {
          baseUrl: "http://127.0.0.1:51823",
          token: "tok-1",
          instanceId: "i-1",
          version: "0.1.0",
          protocol: 1,
          daemonPath: null,
          note: null,
        },
      });
    }
  });

  it("refuses a request with no Origin, or any other, with 403 and no kind", async () => {
    await discovery();
    for (const origin of [
      null,
      "http://localhost:1421",
      "http://127.0.0.1:1420.evil",
    ]) {
      for (const path of [
        "/__openalpaca/connection",
        "/__openalpaca/log-tail",
      ]) {
        const { status, body } = await answered(send(path, { origin }));
        expect(status).toBe(403);
        expect(body).toEqual({ message: expect.any(String) });
      }
    }
  });

  it("refuses with 500 and no kind while OPENALPACA_HOME_STORE is unset, empty or relative", async () => {
    await discovery();
    for (const value of [undefined, "", "relative/store"]) {
      vi.stubEnv("OPENALPACA_HOME_STORE", value);
      for (const path of [
        "/__openalpaca/connection",
        "/__openalpaca/log-tail",
      ]) {
        const { status, body } = await answered(send(path));
        expect(status).toBe(500);
        expect(body).toEqual({ message: expect.any(String) });
      }
    }
  });

  it("reads discovery.json at each request, so a rotated token is followed", async () => {
    await discovery({ token: "tok-1" });
    const first = await answered(send("/__openalpaca/connection"));
    await discovery({ token: "tok-2" });
    const second = await answered(send("/__openalpaca/connection"));
    expect((first.body as { token: string }).token).toBe("tok-1");
    expect((second.body as { token: string }).token).toBe("tok-2");
  });

  it("refuses with the shell's kinds: no file, an unreadable one, an expired token", async () => {
    const refused = async (kind: string) => {
      const { status, body } = await answered(send("/__openalpaca/connection"));
      expect(status).toBe(503);
      expect(body).toEqual({ kind, message: expect.any(String) });
    };
    await refused("not_running");

    await mkdir(join(root, "state"), { recursive: true });
    await writeFile(join(root, "state", "discovery.json"), "{ not json");
    await refused("discovery_unreadable");

    await writeFile(
      join(root, "state", "discovery.json"),
      JSON.stringify({ pid: 1 }),
    );
    await refused("discovery_unreadable");

    await discovery({ expiresAt: "2000-01-01T00:00:00Z" });
    await refused("expired");
  });

  it("answers the log's last lines, colour escapes removed, and nothing for no log", async () => {
    expect(
      await answered(send("/__openalpaca/log-tail", { body: '{"lines":5}' })),
    ).toEqual({ status: 200, body: "" });

    await log("one\n\x1b[2mtwo\x1b[0m\nthree\n\n\n");
    expect(
      await answered(send("/__openalpaca/log-tail", { body: '{"lines":2}' })),
    ).toEqual({ status: 200, body: "two\nthree" });
  });

  it("reads at most 64 KB from the log's end, never a line cut mid-way", async () => {
    const line = "x".repeat(99);
    const lines = Array.from({ length: 1000 }, (_, i) => `${i}:${line}`);
    await log(`${lines.join("\n")}\n`);
    const { body } = await answered(
      send("/__openalpaca/log-tail", { body: '{"lines":100000}' }),
    );
    const tail = (body as string).split("\n");
    expect(tail.at(-1)).toBe(`999:${line}`);
    expect(Buffer.byteLength(body as string)).toBeLessThanOrEqual(64 * 1024);
    expect(lines).toContain(tail[0]);
  });

  it("refuses a log it cannot read, or a request naming no line count, as log_unreadable", async () => {
    await mkdir(join(root, "state", "logs", "daemon.log"), { recursive: true });
    for (const body of ['{"lines":5}', '{"lines":"5"}', "{}", "not json"]) {
      const answer = await answered(send("/__openalpaca/log-tail", { body }));
      expect(answer.status).toBe(503);
      expect(answer.body).toEqual({
        kind: "log_unreadable",
        message: expect.any(String),
      });
    }
  });

  it("passes on every other request: another path, or another method", async () => {
    expect(await send("/src/main.tsx")).toEqual({ passed: true });
    expect(await send("/__openalpaca/elsewhere")).toEqual({ passed: true });
    expect(await send("/__openalpaca/connection", { method: "GET" })).toEqual({
      passed: true,
    });
  });

  it("says why it refuses an Origin or a missing store, in its own sentences", async () => {
    await discovery();
    expect(
      await answered(
        send("/__openalpaca/connection", { origin: "http://localhost:1421" }),
      ),
    ).toEqual({
      status: 403,
      body: {
        message:
          "The dev server answers its own page only: this request's Origin is not http://localhost:1420 or http://127.0.0.1:1420.",
      },
    });

    vi.stubEnv("OPENALPACA_HOME_STORE", "relative/store");
    expect(await answered(send("/__openalpaca/connection"))).toEqual({
      status: 500,
      body: {
        message:
          "The dev server has no development store: start it with OPENALPACA_HOME_STORE set to an absolute path.",
      },
    });
  });

  it("names the discovery file it found missing or expired", async () => {
    const file = join(root, "state", "discovery.json");
    expect(await answered(send("/__openalpaca/connection"))).toEqual({
      status: 503,
      body: {
        kind: "not_running",
        message: `No discovery file at ${file}: the development daemon may not be running.`,
      },
    });

    await discovery({ expiresAt: "2000-01-01T00:00:00Z" });
    expect(await answered(send("/__openalpaca/connection"))).toEqual({
      status: 503,
      body: {
        kind: "expired",
        message: `The token in ${file} has expired.`,
      },
    });
  });

  it("refuses a negative line count as log_unreadable", async () => {
    await log("one\ntwo\n");
    const file = join(root, "state", "logs", "daemon.log");
    expect(
      await answered(send("/__openalpaca/log-tail", { body: '{"lines":-1}' })),
    ).toEqual({
      status: 503,
      body: {
        kind: "log_unreadable",
        message: `Could not read ${file}: the request did not say how many lines.`,
      },
    });
  });

  it("answers a version or protocol of the wrong type as null", async () => {
    await mkdir(join(root, "state"), { recursive: true });
    await writeFile(
      join(root, "state", "discovery.json"),
      JSON.stringify({
        listen: { host: "127.0.0.1", port: 51823 },
        auth: { token: "tok-1", expires_at: "2999-01-01T00:00:00Z" },
        instance_id: "i-1",
        pid: 4242,
        build: { version: 1, protocol: "1" },
      }),
    );
    const { status, body } = await answered(send("/__openalpaca/connection"));
    expect(status).toBe(200);
    expect(body).toMatchObject({ version: null, protocol: null });
  });
});
