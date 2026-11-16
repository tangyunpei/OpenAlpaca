/**
 * The two polling reads stop asking while the daemon is deliberately stopped
 * (§6.2 item 4): a stopped daemon is a state, and a query that keeps hammering
 * a dead port every 30 s is noise. Cached data stays readable.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { invoke } from "@tauri-apps/api/core";

import {
  bootstrapConnection,
  resetConnection,
  setStopIntent,
} from "@/lib/connection";

import {
  useConnectionStatus,
  useDaemonStatus,
  useHealth,
  useStopIntent,
} from "./useConnection";

const reads = vi.hoisted(() => ({
  health: vi.fn(() =>
    Promise.resolve({
      status: "ok",
      version: "0.1.0",
      pid: 1,
      instance_id: "i",
    }),
  ),
  status: vi.fn(() => Promise.resolve({ home_root: "/h" })),
}));

vi.mock("@/lib/api/telemetry", () => ({ getHealth: reads.health }));
vi.mock("@/lib/api/status", () => ({
  getDaemonStatus: reads.status,
  requestDaemonShutdown: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

/** One client per test, held outside the wrapper so a rerender keeps it. */
function makeWrapper() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
  };
}

afterEach(() => {
  setStopIntent(null);
  reads.health.mockClear();
  reads.status.mockClear();
});

describe("while the daemon is stopped", () => {
  it("asks neither /v1/health nor /v1/status", async () => {
    setStopIntent("stopped_here");
    const wrapper = makeWrapper();
    const health = renderHook(() => useHealth(), { wrapper });
    const status = renderHook(() => useDaemonStatus(null), { wrapper });

    await act(async () => {
      await Promise.resolve();
    });
    expect(health.result.current.fetchStatus).toBe("idle");
    expect(status.result.current.fetchStatus).toBe("idle");
    expect(reads.health).not.toHaveBeenCalled();
    expect(reads.status).not.toHaveBeenCalled();
  });

  it("asks again once the intent is cleared", async () => {
    setStopIntent("stopped_elsewhere");
    const { result } = renderHook(
      () => ({ health: useHealth(), intent: useStopIntent() }),
      { wrapper: makeWrapper() },
    );
    expect(result.current.intent).toBe("stopped_elsewhere");

    act(() => setStopIntent(null));

    expect(result.current.intent).toBeNull();
    await waitFor(() => expect(reads.health).toHaveBeenCalled());
  });
});

/**
 * Q04's seventh row (A-20): a daemon of this app's protocol at another
 * version is used, and the window says so — both versions, and Stop then
 * Start, never a restart of its own. The shell writes the sentence; the
 * window carries it from the connection to the panel.
 */
describe("the note on a daemon of another version", () => {
  const info = {
    baseUrl: "http://127.0.0.1:51823",
    token: "t",
    instanceId: "i",
    daemonPath: null,
    version: "0.1.0",
    protocol: 1,
  };
  const note =
    "This daemon is version 0.1.0 and this app is 0.2.0; they speak the same protocol, so the app uses it. To run this app's own version, use Stop daemon…, then Start daemon.";

  afterEach(() => {
    resetConnection();
    vi.mocked(invoke).mockReset();
  });

  it("is the shell's sentence, from the connection", async () => {
    vi.mocked(invoke).mockResolvedValue({ ...info, note });
    await bootstrapConnection();
    const { result } = renderHook(() => useConnectionStatus(), {
      wrapper: makeWrapper(),
    });
    expect(result.current.note).toBe(note);
  });

  it("is null for a daemon of this app's version", async () => {
    vi.mocked(invoke).mockResolvedValue({ ...info, note: null });
    await bootstrapConnection();
    const { result } = renderHook(() => useConnectionStatus(), {
      wrapper: makeWrapper(),
    });
    expect(result.current.note).toBeNull();
  });
});
