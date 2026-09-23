/**
 * The `Add key` form (D-F). The hooks are doubled and every mutation input is
 * recorded, so what these check is the form's own contract: the body it sends
 * (`id`, never `key_id`), that a switched-off provider produces **no**
 * `PUT /v1/settings/llm` however the form is driven, that it never flips the
 * switch by itself, and that a failed check never stands in the way of a save.
 */

import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiError } from "@/lib/http";

import { AddKeyForm } from "./AddKeyForm";
import { disabledRefusal, keylessNote } from "./key-copy";

const SECRET = "sk-ant-test-0000-not-a-real-key";

const state = vi.hoisted(() => ({
  providers: {} as Record<string, unknown>,
  upserts: [] as Array<{ provider: string; key: Record<string, unknown> }>,
  upsertFail: null as Error | null,
  validations: [] as Array<{ provider: string; secret: string }>,
  /** What `POST …/validate` answers: a result, or a thrown error. */
  validateResult: null as Record<string, unknown> | null,
  validateFail: null as Error | null,
}));

vi.mock("@/hooks/useSettings", () => ({
  useLlmSettings: () => ({
    data: {
      orchestrator: { model: "claude-haiku-4-5", fallback_models: [] },
      providers: state.providers,
    },
    isPending: false,
    error: null,
  }),
  useUpsertKey: () => ({
    isPending: false,
    mutate: (
      input: { provider: string; key: Record<string, unknown> },
      options?: { onSuccess?: () => void; onError?: (error: Error) => void },
    ) => {
      state.upserts.push(input);
      if (state.upsertFail !== null) options?.onError?.(state.upsertFail);
      else options?.onSuccess?.();
    },
  }),
  useValidateKey: () => ({
    isPending: false,
    mutate: (
      input: { provider: string; secret: string },
      options?: {
        onSuccess?: (result: Record<string, unknown>) => void;
        onError?: (error: Error) => void;
      },
    ) => {
      state.validations.push(input);
      if (state.validateFail !== null) options?.onError?.(state.validateFail);
      else if (state.validateResult !== null)
        options?.onSuccess?.(state.validateResult);
    },
  }),
}));

const provider = (enabled: boolean, requiresKey = true) => ({
  enabled,
  key_selection_strategy: "round_robin",
  keys: [],
  requires_key: requiresKey,
});

beforeEach(() => {
  state.providers = {
    anthropic: provider(true),
    openai: provider(false),
    ollama: provider(false, false),
  };
  state.upserts = [];
  state.upsertFail = null;
  state.validations = [];
  state.validateResult = null;
  state.validateFail = null;
});

function renderForm(providerId = "anthropic") {
  const props = {
    onChangeProvider: vi.fn(),
    onDone: vi.fn(),
    onCancel: vi.fn(),
    onEnableProvider: vi.fn(),
  };
  const view = render(
    <AddKeyForm provider={providerId} enableBusy={false} {...props} />,
  );
  return { ...props, ...view };
}

function typeSecret(value = SECRET) {
  fireEvent.change(screen.getByLabelText("API key"), {
    target: { value },
  });
}

describe("saving a key", () => {
  it("sends the CLI's body: `id`, never `key_id`", async () => {
    const { onDone, onCancel } = renderForm();

    typeSecret();
    await userEvent.click(screen.getByRole("button", { name: "Save key" }));

    expect(state.upserts).toHaveLength(1);
    const call = state.upserts[0];
    expect(call?.provider).toBe("anthropic");
    expect(call?.key).toMatchObject({
      secret: SECRET,
      priority: "primary",
      source: "api_console",
    });
    expect(call?.key.id).toMatch(/^anthropic_\d+$/);
    expect(call?.key).toHaveProperty("id");
    expect(call?.key).not.toHaveProperty("key_id");
    // Notes are sent only when there are some.
    expect(call?.key).not.toHaveProperty("notes");
    expect(onDone).toHaveBeenCalledWith("Key added for anthropic");
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("sends the chosen priority, source and notes", async () => {
    renderForm();

    typeSecret();
    fireEvent.change(screen.getByLabelText("Priority"), {
      target: { value: "fallback" },
    });
    fireEvent.change(screen.getByLabelText("Source"), {
      target: { value: "environment" },
    });
    fireEvent.change(screen.getByLabelText("Notes (optional)"), {
      target: { value: "  work account  " },
    });
    await userEvent.click(screen.getByRole("button", { name: "Save key" }));

    expect(state.upserts[0]?.key).toMatchObject({
      priority: "fallback",
      source: "environment",
      notes: "work account",
    });
  });

  it("keeps Save off until a key is typed", () => {
    renderForm();

    expect(screen.getByRole("button", { name: "Save key" })).toBeDisabled();
  });

  it("says a refused save in the daemon's terms and keeps the form open", async () => {
    state.upsertFail = new ApiError("empty", 400, "INVALID_KEY_FORMAT");
    const { onCancel } = renderForm();

    typeSecret();
    await userEvent.click(screen.getByRole("button", { name: "Save key" }));

    expect(screen.getByRole("alert")).toHaveTextContent(
      "The daemon would not take that key: it is empty.",
    );
    expect(onCancel).not.toHaveBeenCalled();
  });
});

describe("a switched-off provider (D-F)", () => {
  it("is refused before any typing, and no PUT is ever sent", () => {
    renderForm("openai");

    // Before anything is typed, the refusal is already on screen.
    expect(screen.getByText(disabledRefusal("openai"))).toBeInTheDocument();
    const save = screen.getByRole("button", { name: "Save key" });
    expect(save).toBeDisabled();

    // Typed, and clicked anyway — a disabled attribute is a rendering
    // decision; the invariant is on the request log.
    typeSecret();
    fireEvent.click(save);
    fireEvent.click(screen.getByRole("button", { name: "Save key" }));

    expect(state.upserts).toEqual([]);
  });

  it("offers the switch, and pressing it is the only thing that enables", async () => {
    const { onEnableProvider, rerender, onChangeProvider, onDone, onCancel } =
      renderForm("openai");

    await userEvent.click(
      screen.getByRole("button", { name: "Turn openai on" }),
    );

    expect(onEnableProvider).toHaveBeenCalledTimes(1);
    expect(onEnableProvider).toHaveBeenCalledWith("openai");
    expect(state.upserts).toEqual([]);

    // The settings query then reports the bit, and the form comes alive
    // without a reload.
    state.providers = { ...state.providers, openai: provider(true) };
    rerender(
      <AddKeyForm
        provider="openai"
        enableBusy={false}
        onChangeProvider={onChangeProvider}
        onDone={onDone}
        onCancel={onCancel}
        onEnableProvider={onEnableProvider}
      />,
    );
    expect(screen.queryByText(disabledRefusal("openai"))).toBeNull();
    typeSecret();
    expect(screen.getByRole("button", { name: "Save key" })).toBeEnabled();
  });

  it("never turns the provider on by itself", async () => {
    const { onEnableProvider } = renderForm("openai");

    typeSecret();
    fireEvent.change(screen.getByLabelText("Notes (optional)"), {
      target: { value: "note" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save key" }));
    await userEvent.click(screen.getByRole("radio", { name: "openai" }));

    expect(onEnableProvider).not.toHaveBeenCalled();
    expect(state.upserts).toEqual([]);
  });
});

describe("checking a key", () => {
  it("never gates the save: a 504 is advisory", async () => {
    state.validateFail = new ApiError(
      "validation deadline passed",
      504,
      "KEY_VALIDATION_TIMEOUT",
    );
    renderForm();

    typeSecret();
    await userEvent.click(screen.getByRole("button", { name: "Check key" }));

    expect(
      screen.getByText(/Could not check the key — the daemon did not answer/),
    ).toBeInTheDocument();
    const save = screen.getByRole("button", { name: "Save key" });
    expect(save).toBeEnabled();
    await userEvent.click(save);
    expect(state.upserts).toHaveLength(1);
  });

  it("reads the reason from format_error, not message", async () => {
    state.validateResult = {
      valid: false,
      format_error: "Anthropic API keys start with 'sk-ant-'.",
      tier: null,
      detected_source: null,
      models_available: [],
      rate_limits: null,
    };
    renderForm();

    typeSecret();
    await userEvent.click(screen.getByRole("button", { name: "Check key" }));

    expect(
      screen.getByText(
        "Key rejected — Anthropic API keys start with 'sk-ant-'.",
      ),
    ).toBeInTheDocument();
  });

  it("falls back to the CLI's literal when there is no reason", async () => {
    state.validateResult = {
      valid: false,
      format_error: null,
      tier: null,
      detected_source: null,
      models_available: [],
      rate_limits: null,
    };
    renderForm();

    typeSecret();
    await userEvent.click(screen.getByRole("button", { name: "Check key" }));

    expect(
      screen.getByText("Key rejected — Key validation failed"),
    ).toBeInTheDocument();
  });

  it("forgets the answer the moment the key changes", async () => {
    state.validateResult = {
      valid: true,
      format_error: null,
      tier: null,
      detected_source: null,
      models_available: [],
      rate_limits: null,
    };
    renderForm();

    typeSecret();
    await userEvent.click(screen.getByRole("button", { name: "Check key" }));
    expect(screen.getByText("Key looks valid.")).toBeInTheDocument();

    typeSecret(`${SECRET}-edited`);
    expect(screen.queryByText("Key looks valid.")).toBeNull();
  });

  it("runs only when asked — typing and saving do not check", async () => {
    renderForm();

    typeSecret();
    await userEvent.click(screen.getByRole("button", { name: "Save key" }));

    expect(state.validations).toEqual([]);
  });
});

describe("a provider that needs no key", () => {
  it("has no key field, no check and no save — only the note and the switch", () => {
    const { onEnableProvider } = renderForm("ollama");

    expect(screen.queryByLabelText("API key")).toBeNull();
    expect(screen.queryByRole("button", { name: "Check key" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Save key" })).toBeNull();
    expect(screen.getByText(keylessNote("ollama"))).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Turn ollama on" }));
    expect(onEnableProvider).toHaveBeenCalledWith("ollama");
  });

  it("offers no switch once it is already on", () => {
    state.providers = { ...state.providers, ollama: provider(true, false) };
    renderForm("ollama");

    expect(screen.getByText(keylessNote("ollama"))).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Turn ollama on" })).toBeNull();
  });
});

describe("the secret", () => {
  it("is hidden, and never leaves the input except in the request bodies", async () => {
    state.validateFail = new ApiError(
      "no answer",
      504,
      "KEY_VALIDATION_TIMEOUT",
    );
    state.upsertFail = new Error("socket closed");
    const { onDone, onEnableProvider } = renderForm();

    const input = screen.getByLabelText("API key");
    expect(input).toHaveAttribute("type", "password");

    typeSecret();
    await userEvent.click(screen.getByRole("button", { name: "Check key" }));
    await userEvent.click(screen.getByRole("button", { name: "Save key" }));

    // The upsert carried it (that is its job), and the check is the one other
    // request whose whole purpose is the secret.
    expect(state.upserts[0]?.key.secret).toBe(SECRET);
    expect(state.validations[0]?.secret).toBe(SECRET);
    // Nothing drawn, toasted or handed upward carries it.
    expect(document.body.textContent ?? "").not.toContain(SECRET);
    for (const call of onDone.mock.calls) {
      expect(JSON.stringify(call)).not.toContain(SECRET);
    }
    expect(onEnableProvider).not.toHaveBeenCalled();
  });

  it("is dropped when the provider changes", () => {
    const props = {
      onChangeProvider: vi.fn(),
      onDone: vi.fn(),
      onCancel: vi.fn(),
      onEnableProvider: vi.fn(),
      enableBusy: false,
    };
    const { rerender } = render(<AddKeyForm provider="anthropic" {...props} />);

    typeSecret();
    state.providers = { ...state.providers, openai: provider(true) };
    rerender(<AddKeyForm provider="openai" {...props} />);

    expect(screen.getByLabelText("API key")).toHaveValue("");
  });
});
