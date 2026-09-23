import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { setStopIntent } from "@/lib/connection";

import type { ModelEntry } from "@/lib/api/types";

import {
  Composer,
  STOPPED_PLACEHOLDER,
  composerPlaceholder,
  type ComposerProps,
} from "./Composer";

const MODELS: ModelEntry[] = [
  {
    id: "claude-sonnet-4-6",
    provider: "anthropic",
    context_window: 200_000,
    input_price_per_million: 3,
    output_price_per_million: 15,
    supports_tools: true,
  },
  {
    id: "gpt-5",
    provider: "openai",
    context_window: 128_000,
    input_price_per_million: 2,
    output_price_per_million: 8,
    supports_tools: true,
  },
];

function setup(overrides: Partial<ComposerProps> = {}) {
  const props: ComposerProps = {
    blocked: false,
    value: "",
    onChange: vi.fn(),
    onSend: vi.fn(),
    models: MODELS,
    model: "claude-sonnet-4-6",
    modelPickerOpen: false,
    onToggleModelPicker: vi.fn(),
    onCloseModelPicker: vi.fn(),
    onPickModel: vi.fn(),
    onManageProviders: vi.fn(),
    spend: "$0.0184",
    ...overrides,
  };
  render(<Composer {...props} />);
  return props;
}

describe("composerPlaceholder (§3.16)", () => {
  it("uses the design's three state-dependent strings", () => {
    expect(composerPlaceholder(null)).toBe(
      "Ask, or describe a job to run in the background…",
    );
    expect(
      composerPlaceholder({ mode: "steer", label: "connector audit" }),
    ).toBe("Steer connector audit mid-run…");
    expect(
      composerPlaceholder({ mode: "queue", label: "connector audit" }),
    ).toBe("Queue a follow-up after connector audit…");
  });
});

describe("Composer — normal state (§3.16b)", () => {
  it("advertises the key affordances and today's spend", () => {
    setup();
    expect(
      screen.getByText("⏎ send · ⇧⏎ newline · ⌘K commands"),
    ).toBeInTheDocument();
    expect(screen.getByText("$0.0184 today")).toBeInTheDocument();
  });

  it("sends on Enter and leaves Shift+Enter to the textarea", () => {
    const props = setup({ value: "audit the connectors" });
    const textarea = screen.getByLabelText("Message");

    fireEvent.keyDown(textarea, { key: "Enter", shiftKey: true });
    expect(props.onSend).not.toHaveBeenCalled();

    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(props.onSend).toHaveBeenCalledTimes(1);
  });

  it("refuses to send an empty draft", () => {
    const props = setup({ value: "   " });
    fireEvent.keyDown(screen.getByLabelText("Message"), { key: "Enter" });
    expect(props.onSend).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
  });

  it("shows the steer banner with its escape hatch", () => {
    const props = setup({
      steer: { mode: "steer", label: "connector audit" },
      onClearSteer: vi.fn(),
    });
    expect(screen.getByText("steering → connector audit")).toBeInTheDocument();
    screen.getByRole("button", { name: "send to assistant instead" }).click();
    expect(props.onClearSteer).toHaveBeenCalled();
  });

  it("opens the model picker and reports the scope it actually writes", () => {
    const props = setup({
      modelPickerOpen: true,
      modelNote: "Changing the model here changes the daemon default",
    });
    expect(
      screen.getByRole("dialog", { name: "Chat model" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Changing the model here changes the daemon default"),
    ).toBeInTheDocument();

    screen.getByRole("button", { name: /gpt-5/ }).click();
    expect(props.onPickModel).toHaveBeenCalledWith("gpt-5");
  });
});

describe("Composer — blocked state (§3.16a)", () => {
  it("replaces the textarea entirely with the action bar", () => {
    setup({ blocked: true, pendingToolName: "shell_execute" });
    expect(screen.queryByLabelText("Message")).toBeNull();
    expect(
      screen.getByText("shell_execute is waiting on you"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("composer paused until answered"),
    ).toBeInTheDocument();
  });

  it("wires approve, deny and always-allow", () => {
    const props = setup({
      blocked: true,
      pendingToolName: "shell_execute",
      onApprove: vi.fn(),
      onDeny: vi.fn(),
      onAlwaysAllow: vi.fn(),
    });

    screen.getByRole("button", { name: /Approve/ }).click();
    screen.getByRole("button", { name: /Deny/ }).click();
    screen.getByRole("button", { name: "Always allow" }).click();

    expect(props.onApprove).toHaveBeenCalledTimes(1);
    expect(props.onDeny).toHaveBeenCalledTimes(1);
    expect(props.onAlwaysAllow).toHaveBeenCalledTimes(1);
  });

  it("disables the three answers while one is in flight", () => {
    setup({ blocked: true, answering: true });
    expect(screen.getByRole("button", { name: /Approve/ })).toBeDisabled();
    expect(screen.getByRole("button", { name: /Deny/ })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Always allow" })).toBeDisabled();
  });
});

/**
 * §6.3: while this window's daemon is stopped, nothing can be sent — through
 * the one gate Send and Enter already share — and the placeholder says why.
 * The draft is kept.
 */
describe("Composer — daemon stopped", () => {
  it("closes Send and Enter, says why, and keeps the draft", () => {
    setStopIntent("stopped_here");
    try {
      const props = setup({ value: "audit the connectors" });
      const textarea = screen.getByLabelText("Message");

      expect(textarea).toHaveAttribute("placeholder", STOPPED_PLACEHOLDER);
      expect(textarea).toHaveValue("audit the connectors");
      expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
      fireEvent.keyDown(textarea, { key: "Enter" });
      expect(props.onSend).not.toHaveBeenCalled();

      // Start clears the intent, and the same composer sends again.
      act(() => setStopIntent(null));
      expect(screen.getByRole("button", { name: "Send" })).toBeEnabled();
    } finally {
      setStopIntent(null);
    }
  });
});
