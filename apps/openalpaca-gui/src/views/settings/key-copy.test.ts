/**
 * D-F: what the `Add key` form claims, and when it may save. The claim is the
 * part that can be wrong, so each sentence is read back here.
 */

import { describe, expect, it } from "vitest";

import { ApiError } from "@/lib/http";

import {
  INCOMPLETE_NOTE,
  disabledRefusal,
  keySavePlan,
  keylessNote,
  mintKeyId,
  saveErrorCopy,
  validationFailureLine,
  validationLine,
} from "./key-copy";

describe("whether the form may save", () => {
  const plan = (over: {
    enabled: boolean;
    requiresKey: boolean;
    secret?: string;
  }) => keySavePlan({ provider: "anthropic", secret: "", ...over }).action;

  it("has nothing to refuse for a provider that needs no key", () => {
    expect(plan({ enabled: false, requiresKey: false })).toBe("keyless");
    expect(plan({ enabled: true, requiresKey: false, secret: "x" })).toBe(
      "keyless",
    );
  });

  it("names the switch before asking for a key", () => {
    // Disabled beats empty: the real blocker is the switch, not the field.
    expect(plan({ enabled: false, requiresKey: true, secret: "" })).toBe(
      "refuse-disabled",
    );
    expect(
      plan({ enabled: false, requiresKey: true, secret: "sk-ant-x" }),
    ).toBe("refuse-disabled");
  });

  it("waits for a key on an enabled provider", () => {
    const result = keySavePlan({
      provider: "anthropic",
      enabled: true,
      requiresKey: true,
      secret: "   ",
    });
    expect(result).toEqual({ action: "incomplete", reason: INCOMPLETE_NOTE });
  });

  it("saves once an enabled provider has a key typed", () => {
    expect(plan({ enabled: true, requiresKey: true, secret: "sk-ant-x" })).toBe(
      "save",
    );
  });

  it("carries the sentence it will show", () => {
    expect(
      keySavePlan({
        provider: "openai",
        enabled: false,
        requiresKey: true,
        secret: "",
      }),
    ).toEqual({ action: "refuse-disabled", reason: disabledRefusal("openai") });
    expect(
      keySavePlan({
        provider: "ollama",
        enabled: false,
        requiresKey: false,
        secret: "",
      }),
    ).toEqual({ action: "keyless", reason: keylessNote("ollama") });
  });
});

describe("the refusal and the keyless page", () => {
  it("says the key would be ignored and how to get out, never that it is wrong", () => {
    const refusal = disabledRefusal("anthropic");
    expect(refusal).toMatch(/^anthropic is switched off/);
    expect(refusal).toContain("would do nothing");
    expect(refusal).toContain("ignored");
    expect(refusal).toMatch(/Turn anthropic on first, then save the key\.$/);
    expect(refusal.toLowerCase()).not.toContain("invalid");
    expect(refusal.toLowerCase()).not.toContain("error");
  });

  it("tells a local provider's owner there is nothing to type", () => {
    const note = keylessNote("ollama");
    expect(note).toMatch(/^ollama needs no API key\./);
    expect(note).toContain("Turning it on is the whole setup");
  });
});

describe("the key id", () => {
  it("is minted the CLI's way: <provider>_<unix seconds>", () => {
    expect(mintKeyId("anthropic", new Date(1_700_000_000_000))).toBe(
      "anthropic_1700000000",
    );
    expect(mintKeyId("openai", new Date(1_700_000_000_999))).toBe(
      "openai_1700000000",
    );
  });
});

describe("a save the daemon refused", () => {
  it("names each of the daemon's own refusals", () => {
    expect(
      saveErrorCopy(new ApiError("empty", 400, "INVALID_KEY_FORMAT")),
    ).toBe("The daemon would not take that key: it is empty.");
    expect(
      saveErrorCopy(new ApiError("no master key", 500, "ENCRYPTION_FAILED")),
    ).toBe(
      "The key could not be encrypted, so nothing was written. no master key",
    );
    expect(
      saveErrorCopy(new ApiError("read-only", 500, "DISK_WRITE_FAILED")),
    ).toBe("llm.toml could not be written, so nothing was saved. read-only");
  });

  it("passes anything else through with the daemon's message", () => {
    expect(saveErrorCopy(new ApiError("teapot", 418, "TEAPOT"))).toBe(
      "Could not save the key — teapot",
    );
    expect(saveErrorCopy(new Error("network down"))).toBe(
      "Could not save the key — network down",
    );
  });
});

describe("what a key check found", () => {
  const result = {
    valid: true,
    tier: null,
    detected_source: null,
    models_available: [] as string[],
    rate_limits: null,
    format_error: null,
  };

  it("reports a valid key and what it can see", () => {
    expect(validationLine(result)).toBe("Key looks valid.");
    expect(
      validationLine({
        ...result,
        models_available: ["a", "b"],
        tier: "build",
      }),
    ).toBe("Key looks valid. · 2 models visible · tier build");
  });

  it("reads the reason from format_error, with the CLI's fallback", () => {
    expect(
      validationLine({
        ...result,
        valid: false,
        format_error: "Anthropic API keys start with 'sk-ant-'.",
      }),
    ).toBe("Key rejected — Anthropic API keys start with 'sk-ant-'.");
    expect(validationLine({ ...result, valid: false })).toBe(
      "Key rejected — Key validation failed",
    );
  });

  it("says a failed check is no reason not to save", () => {
    const line = validationFailureLine(
      new ApiError("deadline", 504, "KEY_VALIDATION_TIMEOUT"),
    );
    expect(line).toContain("Could not check the key");
    expect(line).toContain("(deadline)");
    expect(line).toContain("You can save it anyway");
    expect(line).not.toContain("timed out");
  });
});
