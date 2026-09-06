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

/** What the toast says when a toggle is refused. */
export function providerToggleErrorCopy(
  provider: string,
  error: Error,
): string {
  const code = error instanceof ApiError ? error.code : null;
  switch (code) {
    case "PROVIDER_IS_DEFAULT":
      return `${provider} serves the model you chat with — pick a different model first.`;
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
