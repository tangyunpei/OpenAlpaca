/**
 * The two polling reads stop asking while the daemon is deliberately stopped
 * (§6.2 item 4): a stopped daemon is a state, and a query that keeps hammering
 * a dead port every 30 s is noise. Cached data stays readable.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { setStopIntent } from "@/lib/connection";

import { useDaemonStatus, useHealth, useStopIntent } from "./useConnection";

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
