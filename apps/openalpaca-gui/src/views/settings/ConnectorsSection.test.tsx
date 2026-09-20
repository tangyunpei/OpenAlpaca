import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ConnectorsSection } from "./ConnectorsSection";

const state = vi.hoisted(() => ({
  connectors: [] as unknown[],
  unwired: [] as Array<{ connectorId: string; declaredBy: string }>,
}));

vi.mock("@/hooks/useConnectors", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/useConnectors")>()),
  useConnectors: () => ({
    data: state.connectors,
    isPending: false,
    error: null,
  }),
  useUnwiredConnectors: () => state.unwired,
  useConnectorAction: () => ({ mutate: vi.fn(), isPending: false }),
}));

function connector(overrides: Record<string, unknown> = {}) {
  return {
    id: "telegram",
    name: "Telegram",
    status: "active",
    configured: true,
    source: "telegram",
    registered: true,
    messages_7d: 0,
    ...overrides,
  };
}

beforeEach(() => {
  state.connectors = [];
  state.unwired = [];
});

describe("ConnectorsSection detail (GAP-17, detail half — T49)", () => {
  /** The design's `184 calls 7d`, served as the messages the daemon actually attributed. */
  it("renders the message count over the window the daemon counted", () => {
    state.connectors = [connector({ messages_7d: 184 })];

    render(<ConnectorsSection />);

    expect(screen.getByText(/184 messages · 7d/)).toBeInTheDocument();
  });

  /** One message is one message — a count is not a plural. */
  it("says message, not messages, for a single one", () => {
    state.connectors = [connector({ messages_7d: 1 })];

    render(<ConnectorsSection />);

    expect(screen.getByText(/1 message · 7d/)).toBeInTheDocument();
  });

  /** A quiet week says so in words: `0 messages` reads like a failed load. */
  it("says No messages for a connector with no traffic this week", () => {
    state.connectors = [connector({ messages_7d: 0 })];

    render(<ConnectorsSection />);

    expect(screen.getByText(/No messages · 7d/)).toBeInTheDocument();
  });

  /**
   * The name is the daemon's, and the daemon's is now the connector's own —
   * before T49 a route-side `match` on two ids left Discord as `discord`.
   */
  it("names a connector the display name the daemon sent", () => {
    state.connectors = [
      connector({ id: "discord", name: "Discord", source: "discord" }),
    ];

    render(<ConnectorsSection />);

    expect(screen.getByText("Discord")).toBeInTheDocument();
  });

  /**
   * `registered` answers the one question `status` cannot: an `error` row is
   * either a connector that started and exited, or one that never started.
   */
  it("distinguishes a crashed connector from one that never started", () => {
    state.connectors = [
      connector({
        id: "discord",
        name: "Discord",
        source: "discord",
        status: "error",
        registered: true,
      }),
    ];
    const { unmount } = render(<ConnectorsSection />);
    expect(screen.getByText(/started, then exited/)).toBeInTheDocument();
    unmount();

    state.connectors = [
      connector({
        id: "discord",
        name: "Discord",
        source: "discord",
        status: "error",
        registered: false,
      }),
    ];
    render(<ConnectorsSection />);
    expect(screen.getByText(/never started/)).toBeInTheDocument();
  });

  /** A healthy row says nothing about the handle — status already covers it. */
  it("stays quiet about the registry for a connector that is not in error", () => {
    state.connectors = [connector({ status: "active", registered: true })];

    render(<ConnectorsSection />);

    expect(screen.queryByText(/started, then exited/)).toBeNull();
    expect(screen.queryByText(/never started/)).toBeNull();
  });
});
