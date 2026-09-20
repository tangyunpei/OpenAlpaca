/**
 * G2 — Settings → Models was one step stale.
 *
 * After enabling a provider, and again after `Refresh models`, the banner kept
 * saying "not available, and no model is" while `GET /v1/status` already
 * reported an `effective_default_model`; after picking the chat model it said
 * "configured: X — not available, using Y" with the daemon's default already
 * being Y. Nothing invalidated the status query, so the banner waited for the
 * 30 s poll.
 *
 * These drive the real hooks against a mutation that resolves *after* the
 * status query has already answered — the live ordering — and assert the
 * status entry is invalidated once the write settles.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { useUpdateOrchestratorConfig } from "@/hooks/useOrchestrator";
import { useRefreshModels, useSetProviderEnabled } from "@/hooks/useSettings";
import { qk } from "@/lib/query-keys";

vi.mock("@/lib/api/settings", () => ({
  setProviderEnabled: vi.fn(async () => {
    await Promise.resolve();
    return {
      id: "ollama",
      enabled: true,
      loaded: true,
      removed_models: [],
      restored_models: [],
      discovered_models: 3,
      discovery_error: null,
      warning: null,
    };
  }),
  refreshModels: vi.fn(async () => {
    await Promise.resolve();
    return [];
  }),
}));

vi.mock("@/lib/api/orchestrator", () => ({
  updateOrchestratorConfig: vi.fn(async () => {
    await Promise.resolve();
  }),
  getOrchestratorConfig: vi.fn(async () => ({})),
  getLatencyRecords: vi.fn(async () => []),
  getLatencyAggregates: vi.fn(async () => []),
  getDispatchDecisions: vi.fn(async () => []),
}));

/** A client holding one answered `GET /v1/status` entry, as the window does. */
function seeded(): QueryClient {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false, gcTime: Infinity },
      mutations: { retry: false },
    },
  });
  client.setQueryData(qk.status(null), { home_root: "/tmp/store" });
  return client;
}

function wrapper(client: QueryClient) {
  return function Wrapper({ children }: { children: React.ReactNode }) {
    return (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
  };
}

function statusIsStale(client: QueryClient): boolean {
  return client.getQueryState(qk.status(null))?.isInvalidated === true;
}

describe("a write that changes what the daemon would answer with (G2)", () => {
  it("refetches the status after a provider toggle settles", async () => {
    const client = seeded();
    const { result } = renderHook(() => useSetProviderEnabled(), {
      wrapper: wrapper(client),
    });

    expect(statusIsStale(client)).toBe(false);
    result.current.mutate({ provider: "ollama", enabled: true });

    await waitFor(() => expect(statusIsStale(client)).toBe(true));
  });

  it("refetches the status after a model refresh settles", async () => {
    const client = seeded();
    const { result } = renderHook(() => useRefreshModels(), {
      wrapper: wrapper(client),
    });

    result.current.mutate();
    await waitFor(() => expect(statusIsStale(client)).toBe(true));
  });

  it("refetches the status after the default model is set", async () => {
    const client = seeded();
    const { result } = renderHook(() => useUpdateOrchestratorConfig(), {
      wrapper: wrapper(client),
    });

    result.current.mutate({ model: "qwen3:8b", fallback_models: [] });
    await waitFor(() => expect(statusIsStale(client)).toBe(true));
  });
});
