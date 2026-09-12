/**
 * Settings → Extensions (ADR-030 §9.2), install and uninstall included
 * (GAP-24).
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
  addMcpServer,
  installPlugin,
  listExtensions,
  removeExtension,
  runExtensionVerb,
  setExtensionConfig,
  uninstallExtension,
  updatePlugin,
  validatePlugin,
} from "@/lib/api/extensions";
import type {
  ExtensionKind,
  ExtensionRow,
  ExtensionVerb,
  InstallResponse,
  McpDeclaration,
  UninstallResponse,
  ValidateResponse,
} from "@/lib/api/types";
import { qk } from "@/lib/query-keys";

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
 * `POST /v1/extensions/plugin` — copy a directory in.
 *
 * It starts nothing: the row comes back `unapproved`/`never_seen` and the
 * `approve` verb is what runs it. The caller shows `manifest` as the approval
 * preview rather than pretending the plugin is live.
 */
export function useInstallPlugin(): UseMutationResult<
  InstallResponse,
  Error,
  string
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (path: string) => installPlugin(path),
    onSuccess: () => invalidateExtensionKeys(client),
  });
}

/**
 * `POST /v1/extensions/plugin/validate` — the dry run behind the Add form's
 * preview. It invalidates nothing, because it changed nothing.
 */
export function useValidatePlugin(): UseMutationResult<
  ValidateResponse,
  Error,
  string
> {
  return useMutation({ mutationFn: (path: string) => validatePlugin(path) });
}

export interface UpdatePluginInput {
  id: string;
  path: string;
}

/** `PUT /v1/extensions/plugin/{id}` — replace an installed plugin's tree. */
export function useUpdatePlugin(): UseMutationResult<
  InstallResponse,
  Error,
  UpdatePluginInput
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: UpdatePluginInput) =>
      updatePlugin(input.id, input.path),
    onSuccess: () => invalidateExtensionKeys(client),
  });
}

/** `POST /v1/extensions/mcp` — declare a server and connect it. */
export function useAddMcpServer(): UseMutationResult<
  InstallResponse,
  Error,
  McpDeclaration
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (declaration: McpDeclaration) => addMcpServer(declaration),
    onSuccess: () => invalidateExtensionKeys(client),
  });
}

export interface UninstallInput {
  kind: ExtensionKind;
  id: string;
  keepData: boolean;
}

/**
 * `DELETE /v1/extensions/{kind}/{id}?uninstall=true` — the real removal.
 *
 * Distinct from [`useRemoveExtension`], which only drops an orphan's row. The
 * response names where the directory went, because nothing was deleted.
 */
export function useUninstallExtension(): UseMutationResult<
  UninstallResponse,
  Error,
  UninstallInput
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: UninstallInput) =>
      uninstallExtension(input.kind, input.id, input.keepData),
    onSuccess: () => invalidateExtensionKeys(client),
  });
}
