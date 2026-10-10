/**
 * `Reveal in Finder`'s three helpers: whether the window can, what the button
 * says, and what the shell is asked.
 */

import { invoke } from "@tauri-apps/api/core";
import { afterEach, describe, expect, it, vi } from "vitest";

import { canReveal, revealInFolder, revealLabel } from "./reveal";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const MAC_WEBVIEW =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)";
const WINDOWS_WEBVIEW =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36 Edg/140.0.0.0";
const LINUX_WEBVIEW =
  "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko)";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  vi.mocked(invoke).mockReset();
});

describe("canReveal", () => {
  it("is true inside the Tauri window, which marks itself `isTauri`", () => {
    vi.stubGlobal("isTauri", true);
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
  it("asks the shell's `reveal_path` command for the path as given", async () => {
    vi.mocked(invoke).mockResolvedValue(null);

    await revealInFolder("/Users/me/.openalpaca/artifacts/01-notes.md");

    expect(invoke).toHaveBeenCalledWith("reveal_path", {
      path: "/Users/me/.openalpaca/artifacts/01-notes.md",
    });
  });

  it("rejects with the shell's own sentence", async () => {
    vi.mocked(invoke).mockRejectedValue(
      "/tmp/notes.md is not in an OpenAlpaca store or one of your projects, so it is not shown in its folder.",
    );

    await expect(revealInFolder("/tmp/notes.md")).rejects.toThrow(
      "/tmp/notes.md is not in an OpenAlpaca store or one of your projects, so it is not shown in its folder.",
    );
  });
});
