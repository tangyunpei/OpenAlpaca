/**
 * Which host this window talks to (Q07): chosen once, on first use, by
 * `lib/host/index.ts`. The shell's marker means TauriHost; else, in a
 * development build only, DevServerHost when `VITE_OPENALPACA_HOST` asks
 * for it outside the test run, else FixtureHost; a production build with
 * no shell has NoHost, which refuses every command as a missing bridge
 * does.
 *
 * Each case re-evaluates the modules (`vi.resetModules()` and a dynamic
 * import) after stubbing what the choice reads, so no setter is needed.
 */

import { afterEach, describe, expect, it, vi } from "vitest";

import type { HostBridge } from "./types";

/** The host a fresh evaluation chooses, beside that evaluation's fixture. */
async function fresh() {
  vi.resetModules();
  const index = await import("./index");
  const fixture = await import("./fixture");
  return { host: index.host(), fixtureHost: fixture.fixtureHost };
}

/** The rejection `promise` ends in, with its sentence and its kind. */
async function refusal(promise: Promise<unknown>) {
  const cause = await promise.then(
    () => {
      throw new Error("resolved; a refusal was expected");
    },
    (error: unknown) => error,
  );
  const { ConnectionError } = await import("./answer");
  expect(cause).toBeInstanceOf(ConnectionError);
  const error = cause as InstanceType<typeof ConnectionError>;
  return { message: error.message, kind: error.kind };
}

/** Every command a host serves, each refused with today's no-bridge sentence. */
async function expectNoAnswer(host: HostBridge) {
  expect(await refusal(host.ensureDaemonRunning())).toEqual({
    message: "Tauri command `ensure_daemon_running` failed",
    kind: null,
  });
  expect(await refusal(host.getConnectionInfo())).toEqual({
    message: "Tauri command `get_connection_info` failed",
    kind: null,
  });
  expect(await refusal(host.readDaemonLogTail(5))).toEqual({
    message: "Tauri command `read_daemon_log_tail` failed",
    kind: null,
  });
  expect(await refusal(host.awaitDaemonStopped())).toEqual({
    message: "Tauri command `await_daemon_stopped` failed",
    kind: null,
  });
}

afterEach(() => {
  vi.unstubAllEnvs();
  vi.unstubAllGlobals();
  vi.resetModules();
});

describe("the host this window talks to", () => {
  it("is FixtureHost in jsdom under vitest: a development build, MODE test, no shell marker", async () => {
    expect(import.meta.env.DEV).toBe(true);
    expect(import.meta.env.MODE).toBe("test");
    expect((globalThis as { isTauri?: unknown }).isTauri).toBeUndefined();

    const { host, fixtureHost } = await fresh();

    expect(host.kind).toBe("fixture");
    expect(host).toBe(fixtureHost);
  });

  it("is TauriHost when the shell's marker is set", async () => {
    vi.stubGlobal("isTauri", true);

    const { host } = await fresh();

    expect(host.kind).toBe("tauri");
    expect(host.reveal).toBe(true);
    expect(host.dialog).not.toBeNull();
    expect(host.window).not.toBeNull();
  });

  it("stays FixtureHost under vitest when VITE_OPENALPACA_HOST asks for the dev server", async () => {
    vi.stubEnv("VITE_OPENALPACA_HOST", "dev-server");

    const { host, fixtureHost } = await fresh();

    expect(host.kind).toBe("fixture");
    expect(host).toBe(fixtureHost);
  });

  it("is DevServerHost in a development build that asks for it, outside the test run", async () => {
    vi.stubEnv("MODE", "development");
    vi.stubEnv("VITE_OPENALPACA_HOST", "dev-server");
    const fetches: Array<{ url: string; init: RequestInit | undefined }> = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init?: RequestInit) => {
        fetches.push({ url, init });
        return Response.json({
          baseUrl: "http://127.0.0.1:51823",
          token: "t",
          instanceId: "i-1",
        });
      }),
    );

    const { host } = await fresh();

    expect(host.kind).toBe("dev-server");
    expect(host.reveal).toBe(false);
    expect(host.dialog).toBeNull();
    expect(host.window).toBeNull();
    expect((await host.getConnectionInfo()).instanceId).toBe("i-1");
    expect(fetches.map(({ url, init }) => [url, init?.method])).toEqual([
      ["/__openalpaca/connection", "POST"],
    ]);
  });

  it("starts FixtureHost with the known connection when VITE_OPENALPACA_HOST=fixture, outside the test run", async () => {
    vi.stubEnv("MODE", "development");
    vi.stubEnv("VITE_OPENALPACA_HOST", "fixture");

    const { host } = await fresh();

    expect(host.kind).toBe("fixture");
    const known = {
      baseUrl: "http://127.0.0.1:9999",
      token: "test-token",
      instanceId: "7f3a1122",
      daemonPath: null,
      version: null,
      protocol: null,
      note: null,
    };
    expect(await host.ensureDaemonRunning()).toEqual(known);
    expect(await host.getConnectionInfo()).toEqual(known);
    expect(await host.readDaemonLogTail(5)).toBe("");
    expect(await host.awaitDaemonStopped()).toEqual({
      outcome: "stopped",
      pid: null,
      waitedMs: 0,
    });
  });

  it("starts FixtureHost unanswered under vitest, whatever VITE_OPENALPACA_HOST says, and in a plain dev session", async () => {
    vi.stubEnv("VITE_OPENALPACA_HOST", "fixture");
    await expectNoAnswer((await fresh()).host);

    // A plain session: no variable, whatever the developer's shell exports.
    vi.unstubAllEnvs();
    vi.stubEnv("MODE", "development");
    vi.stubEnv("VITE_OPENALPACA_HOST", undefined);
    const plain = (await fresh()).host;
    expect(plain.kind).toBe("fixture");
    await expectNoAnswer(plain);
  });

  it("is NoHost in a production build with no shell, refusing as a missing bridge does", async () => {
    vi.stubEnv("DEV", false);
    vi.stubEnv("PROD", true);
    vi.stubEnv("MODE", "production");

    const { host, fixtureHost } = await fresh();

    expect(fixtureHost).toBeNull();
    expect(host.kind).toBe("none");
    expect(host.reveal).toBe(false);
    expect(host.dialog).toBeNull();
    expect(host.window).toBeNull();
    await expectNoAnswer(host);
  });

  it("is chosen once: a later change of what the choice reads changes nothing", async () => {
    vi.resetModules();
    const index = await import("./index");
    const first = index.host();

    vi.stubGlobal("isTauri", true);

    expect(index.host()).toBe(first);
    expect(index.host().kind).toBe("fixture");
  });
});
