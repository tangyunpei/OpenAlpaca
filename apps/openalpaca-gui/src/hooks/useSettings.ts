/** Settings → Models & keys. */

import {
  useMutation,
  useQuery,
  useQueryClient,
  type UseMutationResult,
  type UseQueryResult,
} from "@tanstack/react-query";

import {
  getCliBackends,
  getDiscoveredCredentials,
  getKeyStatus,
  getLlmSettings,
  getProviderUsage,
  listModels,
  refreshModels,
  removeKey,
  reorderKeys,
  rescanCredentials,
  setKeyPriority,
  setProviderEnabled,
  upsertKey,
  validateKey,
} from "@/lib/api/settings";
import type {
  AddKeyRequest,
  CliBackendStatus,
  DiscoveredCredentialInfo,
  KeyStatusMap,
  KeyValidationResult,
  LlmSettingsResponse,
  ModelEntry,
  ProviderEnabledResponse,
  ProviderUsageSummary,
  ReorderKeysRequest,
  SetKeyPriorityRequest,
  ValidateKeyRequest,
} from "@/lib/api/types";
import { qk } from "@/lib/query-keys";

export function useLlmSettings(): UseQueryResult<LlmSettingsResponse> {
  return useQuery({
    queryKey: qk.settings.llm(),
    queryFn: ({ signal }) => getLlmSettings(signal),
  });
}

export function useKeyStatus(): UseQueryResult<KeyStatusMap> {
  return useQuery({
    queryKey: qk.settings.keyStatus(),
    queryFn: ({ signal }) => getKeyStatus(signal),
    staleTime: 15_000,
  });
}

/** `health` is hardcoded `"healthy"` and `total_tokens` is lifetime (GAP-08c). */
export function useProviderUsage(): UseQueryResult<ProviderUsageSummary[]> {
  return useQuery({
    queryKey: qk.settings.providerUsage(),
    queryFn: ({ signal }) => getProviderUsage(signal),
  });
}

export function useDiscoveredCredentials(): UseQueryResult<
  DiscoveredCredentialInfo[]
> {
  return useQuery({
    queryKey: qk.settings.credentials(),
    queryFn: ({ signal }) => getDiscoveredCredentials(signal),
  });
}

export function useCliBackends(): UseQueryResult<CliBackendStatus[]> {
  return useQuery({
    queryKey: qk.settings.cliBackends(),
    queryFn: ({ signal }) => getCliBackends(signal),
  });
}

export function useModels(): UseQueryResult<ModelEntry[]> {
  return useQuery({
    queryKey: qk.models.list(),
    queryFn: ({ signal }) => listModels(signal),
    staleTime: 5 * 60_000,
  });
}

function useSettingsMutation<TData, TVariables>(
  mutationFn: (variables: TVariables) => Promise<TData>,
  extraKeys: readonly (readonly unknown[])[] = [],
): UseMutationResult<TData, Error, TVariables> {
  const client = useQueryClient();
  return useMutation({
    mutationFn,
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: qk.settings.all() });
      for (const queryKey of extraKeys)
        void client.invalidateQueries({ queryKey });
    },
  });
}

export function useUpsertKey(): UseMutationResult<void, Error, AddKeyRequest> {
  return useSettingsMutation<void, AddKeyRequest>(
    (req) => upsertKey(req),
    [qk.models.all()],
  );
}

export function useRemoveKey(): UseMutationResult<
  void,
  Error,
  { provider: string; keyId: string }
> {
  return useSettingsMutation<void, { provider: string; keyId: string }>(
    (input) => removeKey(input.provider, input.keyId),
  );
}

export function useReorderKeys(): UseMutationResult<
  void,
  Error,
  ReorderKeysRequest
> {
  return useSettingsMutation<void, ReorderKeysRequest>((req) =>
    reorderKeys(req),
  );
}

export function useSetKeyPriority(): UseMutationResult<
  void,
  Error,
  SetKeyPriorityRequest
> {
  return useSettingsMutation<void, SetKeyPriorityRequest>((req) =>
    setKeyPriority(req),
  );
}

/**
 * The settings payload with one provider's `enabled` flipped — the optimistic
 * write, kept pure so the revert is just handing the old object back.
 */
export function withProviderEnabled(
  settings: LlmSettingsResponse,
  provider: string,
  enabled: boolean,
): LlmSettingsResponse {
  const current = settings.providers[provider];
  if (current === undefined) return settings;
  return {
    ...settings,
    providers: { ...settings.providers, [provider]: { ...current, enabled } },
  };
}

export interface ProviderEnabledInput {
  provider: string;
  enabled: boolean;
}

/**
 * `PUT /v1/settings/llm/providers/{provider}/enabled` (GAP-15), optimistic in
 * the settings cache.
 *
 * The switch moves under the finger and is put back verbatim if the daemon
 * refuses — the 409 on the default model's provider is a refusal an owner will
 * meet, and a switch that stayed moved would be a lie about the daemon's
 * state. The models list is invalidated too: a disable takes the provider's
 * models out of the registry with it.
 */
export function useSetProviderEnabled(): UseMutationResult<
  ProviderEnabledResponse,
  Error,
  ProviderEnabledInput,
  { previous: LlmSettingsResponse | undefined }
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ provider, enabled }: ProviderEnabledInput) =>
      setProviderEnabled(provider, enabled),
    onMutate: async ({ provider, enabled }) => {
      await client.cancelQueries({ queryKey: qk.settings.llm() });
      const previous = client.getQueryData<LlmSettingsResponse>(
        qk.settings.llm(),
      );
      if (previous !== undefined) {
        client.setQueryData(
          qk.settings.llm(),
          withProviderEnabled(previous, provider, enabled),
        );
      }
      return { previous };
    },
    onError: (_error, _input, context) => {
      if (context?.previous !== undefined)
        client.setQueryData(qk.settings.llm(), context.previous);
    },
    onSettled: () => {
      void client.invalidateQueries({ queryKey: qk.settings.all() });
      void client.invalidateQueries({ queryKey: qk.models.all() });
    },
  });
}

export function useValidateKey(): UseMutationResult<
  KeyValidationResult,
  Error,
  ValidateKeyRequest
> {
  return useMutation({
    mutationFn: (req: ValidateKeyRequest) => validateKey(req),
  });
}

export function useRescanCredentials(): UseMutationResult<
  DiscoveredCredentialInfo[],
  Error,
  void
> {
  return useSettingsMutation<DiscoveredCredentialInfo[], void>(() =>
    rescanCredentials(),
  );
}

export function useRefreshModels(): UseMutationResult<
  ModelEntry[],
  Error,
  void
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: () => refreshModels(),
    onSuccess: (models) => {
      client.setQueryData(qk.models.list(), models);
    },
  });
}
