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

  /** Design 09 §4.2: a per-call approval has no *Always allow*. */
  it("offers no Always allow for a per-call approval", () => {
    const props = setup({
      blocked: true,
      pendingToolName: "memory_confirm",
      perCall: true,
      onApprove: vi.fn(),
      onDeny: vi.fn(),
      onAlwaysAllow: vi.fn(),
    });
    expect(screen.queryByRole("button", { name: "Always allow" })).toBeNull();
    screen.getByRole("button", { name: /Approve/ }).click();
    expect(props.onApprove).toHaveBeenCalledTimes(1);
  });

  /**
   * The bar sits right above Approve, so it names the tool as the card does:
   * an MCP or plugin name is its third party's, and a bidi override, a
   * zero-width space or a line break in it is shown, not obeyed (one-line
   * escape, `escapeHiddenLine`).
   */
  it("names the tool it waits on with its hidden characters shown as escapes", () => {
    setup({
      blocked: true,
      pendingToolName: "srv__rm\u{202E}fdp\u{200B}\nArguments: {}",
    });
    const bar = screen.getByText(
      (_, node) =>
        node?.tagName === "SPAN" &&
        (node.textContent ?? "").endsWith(" is waiting on you"),
    );
    expect(bar.textContent).toBe(
      "srv__rm<U+202E>fdp<U+200B><U+000A>Arguments: {} is waiting on you",
    );
    expect(bar.querySelectorAll("[data-hidden-char]")).toHaveLength(3);
  });

  /**
   * The bar shows the name alone, right above Approve. When the request holds
   * hidden characters anywhere — in its arguments too, which the bar does not
   * show — it says how many, and that the card shows them.
   */
  it("counts the whole request's hidden characters beside the name and points at its card", () => {
    setup({
      blocked: true,
      pendingToolName: "shell_execute",
      pendingHidden: 2,
    });
    const bar = screen.getByText(
      (_, node) =>
        node?.tagName === "SPAN" &&
        (node.textContent ?? "").startsWith("shell_execute is waiting on you"),
    );
    expect(bar.textContent).toBe(
      "shell_execute is waiting on you · its card shows 2 hidden characters",
    );
  });

  /**
   * Approve and Deny answer the oldest pending prompt. With more than one,
   * the bar says so and names the card it answers by its red outline.
   */
  it("says it answers the oldest of several pending prompts, and which card that is", () => {
    setup({
      blocked: true,
      pendingToolName: "shell_execute",
      pendingCount: 2,
      pendingHidden: 1,
    });
    const bar = screen.getByText(
      (_, node) =>
        node?.tagName === "SPAN" &&
        (node.textContent ?? "").startsWith("shell_execute is waiting on you"),
    );
    expect(bar.textContent).toBe(
      "shell_execute is waiting on you · oldest of 2 pending — its card is outlined in red · its card shows 1 hidden character",
    );
  });

  it("says nothing of hidden characters when the request holds none", () => {
    setup({
      blocked: true,
      pendingToolName: "shell_execute",
      pendingHidden: 0,
    });
    expect(screen.getByText("shell_execute is waiting on you")).toBeVisible();
    expect(screen.queryByText(/hidden character/)).toBeNull();
  });

  /**
   * A name with no break opportunity in it (escapes included: `<`, the hex
   * and `>` do not break) longer than the bar ran past the transcript's edge.
   * The bar wraps it anywhere instead, right above Approve.
   */
  it("wraps a tool name too long for the bar instead of running past it", () => {
    setup({ blocked: true, pendingToolName: `srv__${"x".repeat(200)}` });
    const bar = screen.getByText(
      (_, node) =>
        node?.tagName === "SPAN" &&
        (node.textContent ?? "").endsWith(" is waiting on you"),
    );
    expect(bar).toHaveClass("min-w-0", "wrap-anywhere");
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
