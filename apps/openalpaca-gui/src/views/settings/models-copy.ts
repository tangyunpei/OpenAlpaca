/**
 * The sentences Settings → Models says about a provider.
 *
 * The note about the default model moved to `@/lib/model-availability`, so the
 * chat first-run card reads the same four branches this screen's banner does.
 *
 * Pure, and out here rather than inline, for the reason `provider-toggle.ts`
 * is: what this screen *claims* is the part that can be wrong, and a claim is
 * only worth making if a test can read it back.
 */

import type { ProviderEnabledResponse, ProviderInfo } from "@/lib/api/types";

/**
 * The description under a provider's name.
 *
 * A provider that needs no key (L1) held `0 keys · round_robin` — which reads
 * as one missing step away from working, when in fact nothing is missing and
 * there is no key editor to reach for. An owner who *has* written a key for a
 * local provider still sees it counted: the line reports what is there.
 */
export function providerKeyLine(info: ProviderInfo): string {
  const keys =
    !info.requires_key && info.keys.length === 0
      ? "no key needed"
      : `${info.keys.length} ${info.keys.length === 1 ? "key" : "keys"}`;
  return `${keys} · ${info.key_selection_strategy}`;
}

/**
 * The toast a provider toggle's own `200` earns.
 *
 * `discovered_models` is the enable's answer about the catalogue (L2), and it
 * is the whole point of turning a local provider on: the owner wants to know
 * the daemon can see what `ollama pull` installed. A zero is never left to
 * speak for itself — either the provider was asked and has nothing, or it
 * could not be asked and `discovery_error` says why.
 */
export function providerToggleToast(row: ProviderEnabledResponse): string {
  if (!row.enabled) return `${row.id} off`;
  if (!row.loaded) return `${row.id} on, but the daemon could not load it`;
  if (row.discovery_error !== null) {
    return `${row.id} on — its model list could not be read (${row.discovery_error})`;
  }
  if (row.discovered_models === 0) {
    return `${row.id} on — it reported no models`;
  }
  return `${row.id} on — found ${row.discovered_models} ${
    row.discovered_models === 1 ? "model" : "models"
  }`;
}
