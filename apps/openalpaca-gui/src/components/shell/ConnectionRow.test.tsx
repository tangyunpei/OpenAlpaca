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
