/**
 * `Reveal in Finder`'s three helpers: whether the window can, what the button
 * says, and what the daemon is asked (Q06: its `POST /v1/files/{id}/reveal`).
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { resetConnection } from "./connection";
import { fixtureHost } from "./host/fixture";
import { canReveal, revealInFolder, revealLabel } from "./reveal";

const MAC_WEBVIEW =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)";
const WINDOWS_WEBVIEW =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36 Edg/140.0.0.0";
const LINUX_WEBVIEW =
  "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko)";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  fixtureHost!.reset();
});

describe("canReveal", () => {
  it("is true when the host draws Reveal, as the Tauri window's does", () => {
    fixtureHost!.setReveal(true);
    expect(canReveal()).toBe(true);
  });

  it("is false with no shell — the Vite preview, or jsdom", () => {
    expect(canReveal()).toBe(false);
  });
});

describe("revealLabel", () => {
  it("names Finder on macOS", () => {
    expect(revealLabel(MAC_WEBVIEW)).toBe("Reveal in Finder");
  });

  it("names the folder everywhere else", () => {
    expect(revealLabel(WINDOWS_WEBVIEW)).toBe("Show in folder");
    expect(revealLabel(LINUX_WEBVIEW)).toBe("Show in folder");
  });

  it("reads this window's user agent when given none", () => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue(MAC_WEBVIEW);
    expect(revealLabel()).toBe("Reveal in Finder");
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue(WINDOWS_WEBVIEW);
    expect(revealLabel()).toBe("Show in folder");
  });
});

describe("revealInFolder", () => {
  interface Recorded {
    url: string;
    method: string;
    body: unknown;
  }
  let requests: Recorded[] = [];
  let reply: () => Response = () => new Response(null);

  function json(payload: unknown, status = 200): Response {
    return new Response(JSON.stringify(payload), { status });
  }

  beforeEach(() => {
    requests = [];
    resetConnection();
    fixtureHost!.answerConnection({
      baseUrl: "http://127.0.0.1:9999",
      token: "tok",
      instanceId: "7f3a1122",
    });
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: unknown, init?: RequestInit) => {
        requests.push({
          url: String(input),
          method: init?.method ?? "GET",
          body: init?.body ?? null,
        });
        return reply();
      }),
    );
  });

  it("asks the daemon's `POST /v1/files/{id}/reveal` for the file id, never a path", async () => {
    reply = () => json({ id: "art 1", status: "revealed" });
    const logTail = vi.spyOn(fixtureHost!, "readDaemonLogTail");
    const stopWait = vi.spyOn(fixtureHost!, "awaitDaemonStopped");

    await revealInFolder("art 1");

    expect(requests).toEqual([
      {
        url: "http://127.0.0.1:9999/v1/files/art%201/reveal",
        method: "POST",
        body: null,
      },
    ]);
    // The shell is asked for the connection only — no command of its own:
    // the host has no reveal command, and no other command, dialog or
    // window of it was used.
    expect(logTail).not.toHaveBeenCalled();
    expect(stopWait).not.toHaveBeenCalled();
    expect(fixtureHost!.dialog).toBeNull();
    expect(fixtureHost!.window).toBeNull();
  });

  it("rejects with the daemon's own sentence for a file it does not show", async () => {
    reply = () =>
      json(
        {
          error: {
            code: "REVEAL_REFUSED",
            message:
              "/tmp/notes.md is not in an OpenAlpaca store or one of your projects, so it is not shown in its folder.",
          },
        },
        409,
      );

    await expect(revealInFolder("art-1")).rejects.toThrow(
      "/tmp/notes.md is not in an OpenAlpaca store or one of your projects, so it is not shown in its folder.",
    );
  });

  it("rejects with the sentence of a reveal the platform could not do", async () => {
    reply = () =>
      json(
        {
          error: {
            code: "REVEAL_FAILED",
            message:
              "open could not show /Users/me/.openalpaca/artifacts/01-notes.md in its folder (exit status: 1).",
          },
        },
        500,
      );

    await expect(revealInFolder("art-1")).rejects.toThrow(
      "open could not show /Users/me/.openalpaca/artifacts/01-notes.md in its folder (exit status: 1).",
    );
  });

  it("rejects with a missing project's sentence, 410", async () => {
    reply = () =>
      json(
        {
          error: {
            code: "ARTIFACT_PROJECT_MISSING",
            message:
              "/Users/me/proj: this project's folder is gone or has moved; re-point the project to read its files.",
          },
        },
        410,
      );

    await expect(revealInFolder("art-1")).rejects.toThrow(
      "/Users/me/proj: this project's folder is gone or has moved; re-point the project to read its files.",
    );
  });
});
