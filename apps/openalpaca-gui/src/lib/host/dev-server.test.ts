/**
 * DevServerHost (Q07), the client half of `VITE_OPENALPACA_HOST=dev-server`:
 * it asks the Vite dev server's own endpoints (`scripts/dev-server-host.ts`)
 * on the page's origin, and every answer and refusal passes through the
 * shared answer check (`lib/host/answer.ts`), so a refusal carrying one of
 * the shell's kinds reads exactly as the shell's does.
 */

import { afterEach, describe, expect, it, vi } from "vitest";

import { ConnectionError } from "./answer";
import { createDevServerHost } from "./dev-server";

/** What each `fetch` was asked: its URL, method, headers and body. */
let asked: Array<{ url: string; method?: string; body: unknown }> = [];

/** The dev server answering every request with `answer`. */
function devServer(answer: () => Response | Promise<Response>): void {
  asked = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init?: RequestInit) => {
      asked.push({
        url,
        method: init?.method,
        body: typeof init?.body === "string" ? JSON.parse(init.body) : null,
      });
      expect(new Headers(init?.headers).get("Content-Type")).toBe(
        "application/json",
      );
      return answer();
    }),
  );
}

/** The rejection `promise` ends in, as a `ConnectionError`. */
async function rejection(promise: Promise<unknown>): Promise<ConnectionError> {
  const cause = await promise.then(
    () => {
      throw new Error("resolved; a refusal was expected");
    },
    (error: unknown) => error,
  );
  expect(cause).toBeInstanceOf(ConnectionError);
  return cause as ConnectionError;
}

const ANSWER = {
  baseUrl: "http://127.0.0.1:51823",
  token: "t",
  instanceId: "i-1",
  daemonPath: null,
  version: "0.1.0",
  protocol: 1,
  note: null,
};

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("DevServerHost", () => {
  it("is the dev server's: no Reveal, no dialog, no window", () => {
    const host = createDevServerHost();
    expect(host.kind).toBe("dev-server");
    expect(host.reveal).toBe(false);
    expect(host.dialog).toBeNull();
    expect(host.window).toBeNull();
  });

  it("asks the connection endpoint by POST on the page's origin, at boot and on reconnect alike", async () => {
    devServer(() => Response.json(ANSWER));
    const host = createDevServerHost();

    expect(await host.ensureDaemonRunning()).toEqual(ANSWER);
    expect(await host.getConnectionInfo()).toEqual(ANSWER);
    expect(asked).toEqual([
      { url: "/__openalpaca/connection", method: "POST", body: {} },
      { url: "/__openalpaca/connection", method: "POST", body: {} },
    ]);
  });

  it("asks the log-tail endpoint by POST with the lines wanted", async () => {
    devServer(() => Response.json("four\nfive"));

    expect(await createDevServerHost().readDaemonLogTail(2)).toBe("four\nfive");
    expect(asked).toEqual([
      { url: "/__openalpaca/log-tail", method: "POST", body: { lines: 2 } },
    ]);
  });

  it("reads a well-formed answer as the shell's is read: absent fields null", async () => {
    devServer(() =>
      Response.json({
        baseUrl: "http://127.0.0.1:51823",
        token: "t",
        instanceId: "i-1",
      }),
    );
    expect(await createDevServerHost().getConnectionInfo()).toEqual({
      baseUrl: "http://127.0.0.1:51823",
      token: "t",
      instanceId: "i-1",
      daemonPath: null,
      version: null,
      protocol: null,
      note: null,
    });
  });

  it("refuses an unexpected payload, naming the command", async () => {
    devServer(() => Response.json({ token: "t" }));
    expect(
      (await rejection(createDevServerHost().ensureDaemonRunning())).message,
    ).toBe("`ensure_daemon_running` returned an unexpected payload");
  });

  it("keeps the kind and the sentence of a refusal that carries one of the shell's kinds", async () => {
    for (const [kind, message] of [
      [
        "not_running",
        "No discovery.json under /s/state: the daemon may not be running.",
      ],
      ["expired", "The token in /s/state/discovery.json has expired."],
      [
        "discovery_unreadable",
        "Could not read /s/state/discovery.json: not JSON.",
      ],
    ] as const) {
      devServer(() => Response.json({ kind, message }, { status: 503 }));
      const error = await rejection(createDevServerHost().getConnectionInfo());
      expect(error.kind).toBe(kind);
      expect(error.message).toBe(message);
    }
    devServer(() =>
      Response.json(
        {
          kind: "log_unreadable",
          message: "Could not read /s/state/logs/daemon.log: EISDIR",
        },
        { status: 503 },
      ),
    );
    const error = await rejection(createDevServerHost().readDaemonLogTail(5));
    expect(error.kind).toBe("log_unreadable");
    expect(error.message).toBe(
      "Could not read /s/state/logs/daemon.log: EISDIR",
    );
  });

  it("keeps the sentence of a refusal with no kind, verbatim and kindless", async () => {
    for (const status of [403, 500]) {
      devServer(() =>
        Response.json({ message: "the dev server's own sentence" }, { status }),
      );
      const error = await rejection(createDevServerHost().getConnectionInfo());
      expect(error.kind).toBeNull();
      expect(error.message).toBe("the dev server's own sentence");
    }
  });

  it("says so in its own sentence when the dev server does not answer", async () => {
    devServer(() => {
      throw new TypeError("Failed to fetch");
    });
    const error = await rejection(createDevServerHost().ensureDaemonRunning());
    expect(error.kind).toBeNull();
    expect(error.message).toBe("The dev server did not answer.");
  });

  it("says so in its own sentence when a refusal carries no sentence", async () => {
    devServer(() => new Response("<html>", { status: 502 }));
    const error = await rejection(createDevServerHost().getConnectionInfo());
    expect(error.kind).toBeNull();
    expect(error.message).toBe("The dev server answered HTTP 502.");
  });

  it("cannot wait for a daemon to stop, and says so with no kind", async () => {
    devServer(() => Response.json({}));
    const error = await rejection(createDevServerHost().awaitDaemonStopped());
    expect(error.kind).toBeNull();
    expect(error.message).toBe(
      "The dev server cannot wait for the daemon to stop.",
    );
    expect(asked).toEqual([]);
  });
});
