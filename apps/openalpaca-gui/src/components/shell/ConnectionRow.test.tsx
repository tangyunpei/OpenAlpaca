import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import {
  ConnectionRowView,
  connectionLabel,
  connectionTone,
} from "./ConnectionRow";

describe("connection status mapping (§3.6)", () => {
  it("is green only when the socket is up and health agrees", () => {
    expect(connectionTone("connected", true)).toBe("up");
    // Socket up but `/v1/health` not yet answering: still settling.
    expect(connectionTone("connected", false)).toBe("pending");
  });

  it("is gold while it is still trying and red once it has failed", () => {
    expect(connectionTone("idle", false)).toBe("pending");
    expect(connectionTone("connecting", false)).toBe("pending");
    expect(connectionTone("disconnected", false)).toBe("down");
    expect(connectionTone("error", false)).toBe("down");
  });

  it("labels every socket state", () => {
    expect(connectionLabel("connected")).toBe("connected");
    expect(connectionLabel("connecting")).toBe("connecting");
    expect(connectionLabel("idle")).toBe("starting");
    expect(connectionLabel("disconnected")).toBe("disconnected");
    expect(connectionLabel("error")).toBe("connection error");
  });

  /**
   * A stopped daemon is neither an error nor a wait: the neutral tone, and a
   * word that says who stopped it — whatever the socket's own state says.
   */
  it("reads a stop intent as stopped, never as a failure", () => {
    expect(connectionTone("disconnected", false, "stopped_here")).toBe(
      "stopped",
    );
    expect(connectionTone("error", false, "stopped_elsewhere")).toBe("stopped");
    expect(connectionLabel("disconnected", "stopped_here")).toBe("stopped");
    expect(connectionLabel("disconnected", "stopped_elsewhere")).toBe(
      "stopped elsewhere",
    );
    // No intent: the three socket states are exactly as before.
    expect(connectionTone("disconnected", false, null)).toBe("down");
    expect(connectionLabel("disconnected", null)).toBe("disconnected");

    render(
      <ConnectionRowView tone="stopped" label="stopped" instance="7f3a" />,
    );
    const dot = screen.getByRole("img", { name: "Daemon stopped" });
    expect(dot.className).toContain("bg-muted-fg");
    expect(dot.className).not.toContain("bg-red");
  });

  /** Only what the shell's wait concluded may say "stopped". */
  it("words this window's own stop by its phase", () => {
    const label = (phase: Parameters<typeof connectionLabel>[2]) =>
      connectionLabel("disconnected", "stopped_here", phase);
    expect(label("stopping")).toBe("stopping");
    expect(label("stopped")).toBe("stopped");
    expect(label("still_alive")).toBe("still running");
    expect(label("lock_still_held")).toBe("lock still held");
    expect(label("unconfirmed")).toBe("stop unconfirmed");
    // A phase never renames a stop someone else made.
    expect(
      connectionLabel("disconnected", "stopped_elsewhere", "stopping"),
    ).toBe("stopped elsewhere");
  });

  it("shows the four-character instance id, and omits it when unknown", () => {
    const { rerender } = render(
      <ConnectionRowView tone="up" label="connected" instance="7f3a" />,
    );
    expect(screen.getByText("7f3a")).toBeInTheDocument();

    rerender(
      <ConnectionRowView tone="pending" label="connecting" instance={null} />,
    );
    expect(screen.queryByText("7f3a")).toBeNull();
    expect(screen.getByText("connecting")).toBeInTheDocument();
  });
});
