/**
 * What the provider on/off switch says when the daemon refuses it (GAP-15).
 *
 * The copy is driven by the error *code*, not by matching message strings —
 * the same rule the extension rows follow. The 409 is the one an owner will
 * actually meet: you cannot turn off the provider that serves the model you
 * chat with, and the sentence has to say what to do next rather than only
 * what failed.
 *
 * The optimistic half lives with the mutation that performs it
 * (`useSetProviderEnabled`).
 */

import { ApiError } from "@/lib/http";

/**
 * What the toast says when a toggle is refused.
 *
 * `defaultModel` is what the daemon reports as `[orchestrator] model` — the
 * same field the guard resolves — and is only used by
 * `DEFAULT_MODEL_UNRESOLVED`, which is about that model rather than about the
 * provider. Omitted, the sentence drops the id rather than inventing one.
 */
export function providerToggleErrorCopy(
  provider: string,
  error: Error,
  defaultModel?: string,
): string {
  const code = error instanceof ApiError ? error.code : null;
  switch (code) {
    case "PROVIDER_IS_DEFAULT":
      return `${provider} serves the model you chat with — pick a different model first.`;
    // Not the same sentence as the 409 above: there, this provider is the one
    // that would have answered. Here nothing would have — the default model
    // names no provider at all — so saying "<provider> serves the model you
    // chat with" would be false about the row the owner just touched (R61a).
    case "DEFAULT_MODEL_UNRESOLVED": {
      const named =
        defaultModel === undefined || defaultModel === ""
          ? "The default model"
          : `The default model ${defaultModel}`;
      return `${named} does not resolve to any provider — fix it in Settings → Models before disabling a provider.`;
    }
    case "PROVIDER_NOT_FOUND":
      return `This daemon does not know a provider called ${provider}.`;
    case "LLM_NOT_CONFIGURED":
      return "No LLM router is configured, so providers cannot be switched.";
    case "DISK_WRITE_FAILED":
      return `${provider} was left as it was — the daemon could not write llm.toml.`;
    default:
      return `${provider} — ${error.message}`;
  }
}
