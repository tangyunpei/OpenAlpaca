/**
 * Settings → Extensions (ADR-030 §9.2). Install / uninstall is GAP-24.
 *
 * Every verb returns the resulting row, so a mutation's `onSuccess` has the
 * truth in hand — but nothing is rendered from it: the list query is
 * invalidated and the row re-read, which is the same "one source, three
 * renderings" rule the daemon follows (§8, X-18). A late or reordered event can
 * therefore never show a state the daemon is not in.
 */

import {
  useMutation,
  useQuery,
  useQueryClient,
  type UseMutationResult,
  type UseQueryResult,
} from "@tanstack/react-query";

import {
  listExtensions,
  removeExtension,
  runExtensionVerb,
  setExtensionConfig,
} from "@/lib/api/extensions";
import type {
  ExtensionKind,
  ExtensionRow,
  ExtensionVerb,
} from "@/lib/api/types";
import { qk } from "@/lib/query-keys";
import { unavailable, type Availability } from "@/lib/unavailable";

export function useExtensions(): UseQueryResult<ExtensionRow[]> {
  return useQuery({
    queryKey: qk.extensions.list(),
    queryFn: ({ signal }) => listExtensions(signal),
  });
}

export interface ExtensionVerbInput {
  kind: ExtensionKind;
  id: string;
  verb: ExtensionVerb;
}

/**
 * The four lifecycle verbs plus `reload`.
 *
 * A plugin's contributions come and go with it, so a verb invalidates the
 * skill, agent and connector lists as well as the extension and tool ones —
 * the same set §9.5 gives `extension_state_changed`, because the WS frame may
 * arrive before or after this response.
 */
export function useExtensionVerb(): UseMutationResult<
  ExtensionRow,
  Error,
  ExtensionVerbInput
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: ExtensionVerbInput) =>
      runExtensionVerb(input.kind, input.id, input.verb),
    onSuccess: () => invalidateExtensionKeys(client),
  });
}

/** `DELETE /v1/extensions/plugin/{id}` — the Remove affordance on an orphan. */
export function useRemoveExtension(): UseMutationResult<void, Error, string> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => removeExtension(id),
    onSuccess: () => invalidateExtensionKeys(client),
  });
}

export interface ExtensionConfigInput {
  id: string;
  key: string;
  value: string;
}

/** One key per call — the route's shape, and the daemon may start the plugin. */
export function useSetExtensionConfig(): UseMutationResult<
  void,
  Error,
  ExtensionConfigInput
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: ExtensionConfigInput) =>
      setExtensionConfig(input.id, input.key, input.value),
    onSuccess: () => invalidateExtensionKeys(client),
  });
}

function invalidateExtensionKeys(
  client: ReturnType<typeof useQueryClient>,
): void {
  for (const queryKey of [
    qk.extensions.all(),
    qk.tools.all(),
    qk.skills.all(),
    qk.agents.all(),
    qk.connectors.all(),
  ]) {
    void client.invalidateQueries({ queryKey });
  }
}

/**
 * GAP-24 — installing an extension still means dropping a directory into the
 * plugins root, or writing a `[servers.<name>]` block into `config/mcp.toml`,
 * and restarting. `DELETE /v1/extensions/plugin/{id}` removes an *orphan's*
 * entry; it is not an uninstall.
 */
export function useExtensionInstall(): Availability<never> {
  return unavailable("GAP-24");
}
