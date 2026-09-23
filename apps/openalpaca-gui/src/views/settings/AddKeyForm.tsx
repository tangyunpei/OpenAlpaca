/**
 * `Add key` — Settings → Models & keys (DESIGN_SPEC §5.4a, D-F).
 *
 * A port of `openalpaca llm keys add` (`apps/openalpaca/src/commands/
 * llm_keys.rs`, `keys_add`): the same provider choice, hidden secret,
 * priority, source and notes, and the same body —
 * `PUT /v1/settings/llm {provider, key: {id, secret, priority, source, notes}}`.
 * The field is **`id`**, minted the CLI's way; `key_id` would be dropped by
 * serde without a word.
 *
 * Three rules shape it:
 *
 *  * **A switched-off provider is refused before any typing** (D-F), with its
 *    switch in reach. A key saved there would be written and ignored — the
 *    router holds none of a disabled provider's models. `Turn <provider> on`
 *    is the section's own `toggleProvider`, the same mutation the row's switch
 *    calls; nothing here flips the bit as a side effect of opening, typing or
 *    saving. `Save key` is guarded twice — disabled in the DOM, and re-planned
 *    in the handler — because the invariant is "no `PUT` for a disabled
 *    provider", not "the button looked off". And **"enabled" means the
 *    daemon's word, not the optimistic cache's**: while a switch is in
 *    flight the cache already shows it on, so saving waits for the switch to
 *    settle — a failed enable must not leave a key behind on a provider that
 *    stayed off.
 *  * **Checking never gates saving.** `Check key` is its own button
 *    (`POST /v1/settings/llm/validate`, which answers `504` for every failure
 *    and goes to the network on success); its answer is advisory, cleared the
 *    moment the secret changes, and `Save key` never waits for it.
 *  * **A provider that needs no key gets no key field** (L1). Ollama's page
 *    says there is nothing to type and offers the switch if it is off.
 *
 * The secret lives in this component's state and nowhere else: never in a
 * toast, an error line, a query key or a log. It is cleared on a successful
 * save and whenever the provider changes.
 */

import { useRef, useState } from "react";

import { Button } from "@/components/ui";
import { cn } from "@/lib/cn";
import type { KeyPriorityValue, KeySourceValue } from "@/lib/api/types";
import {
  useLlmSettings,
  useUpsertKey,
  useValidateKey,
} from "@/hooks/useSettings";

import {
  KEY_PRIORITIES,
  KEY_SOURCE_OPTIONS,
  keySavePlan,
  mintKeyId,
  saveErrorCopy,
  validationFailureLine,
  validationLine,
} from "./key-copy";

export interface AddKeyFormProps {
  /** The provider the form is adding a key for. */
  provider: string;
  onChangeProvider: (provider: string) => void;
  /** Called with a sentence to toast once the key is saved. */
  onDone: (message: string) => void;
  onCancel: () => void;
  /** The section's own `toggleProvider(provider, true)` — one code path. */
  onEnableProvider: (provider: string) => void;
  /**
   * Whether that toggle is in flight. It also holds the save: the settings
   * cache is flipped optimistically before the daemon answers.
   */
  enableBusy: boolean;
}

type CheckLine = { ok: boolean; text: string } | null;

const FIELD =
  "rounded-md border border-line bg-raised px-[8px] py-[4px] text-2xs-plus text-ink";

export function AddKeyForm({
  provider,
  onChangeProvider,
  onDone,
  onCancel,
  onEnableProvider,
  enableBusy,
}: AddKeyFormProps) {
  const llm = useLlmSettings();
  const upsert = useUpsertKey();
  const validate = useValidateKey();

  const [secret, setSecret] = useState("");
  const [priority, setPriority] = useState<KeyPriorityValue>("primary");
  const [source, setSource] = useState<KeySourceValue>("api_console");
  const [notes, setNotes] = useState("");
  const [check, setCheck] = useState<CheckLine>(null);
  const [failure, setFailure] = useState<string | null>(null);
  // Each secret edit bumps this, so a check answered for an older secret is
  // dropped rather than shown under a key it was not about.
  const checkSeq = useRef(0);

  // A different provider is a different key: nothing typed for one travels to
  // the next.
  const [draftFor, setDraftFor] = useState(provider);
  if (draftFor !== provider) {
    setDraftFor(provider);
    setSecret("");
    setCheck(null);
    setFailure(null);
  }

  const providers = Object.entries(llm.data?.providers ?? {});
  const info = llm.data?.providers[provider];

  const editSecret = (next: string) => {
    checkSeq.current += 1;
    setSecret(next);
    setCheck(null);
    setFailure(null);
  };

  const plan =
    info === undefined
      ? null
      : keySavePlan({
          provider,
          enabled: info.enabled,
          requiresKey: info.requires_key,
          secret,
          switchPending: enableBusy,
        });

  const onCheck = () => {
    if (secret.trim().length === 0) return;
    const seq = checkSeq.current;
    validate.mutate(
      { provider, secret: secret.trim() },
      {
        onSuccess: (result) => {
          if (seq !== checkSeq.current) return;
          setCheck({ ok: result.valid, text: validationLine(result) });
        },
        onError: (error) => {
          if (seq !== checkSeq.current) return;
          setCheck({ ok: false, text: validationFailureLine(error) });
        },
      },
    );
  };

  const onSave = () => {
    // The second guard: re-plan from the state as it is now, whatever the
    // button looked like.
    if (info === undefined) return;
    const now = keySavePlan({
      provider,
      enabled: info.enabled,
      requiresKey: info.requires_key,
      secret,
      switchPending: enableBusy,
    });
    if (now.action !== "save") return;
    const trimmedNotes = notes.trim();
    upsert.mutate(
      {
        provider,
        key: {
          id: mintKeyId(provider),
          secret: secret.trim(),
          priority,
          source,
          ...(trimmedNotes.length > 0 ? { notes: trimmedNotes } : {}),
        },
      },
      {
        onSuccess: () => {
          setSecret("");
          onDone(`Key added for ${provider}`);
          onCancel();
        },
        onError: (error) => setFailure(saveErrorCopy(error)),
      },
    );
  };

  const turnOn = (
    <span>
      <Button
        variant="primarySm"
        disabled={enableBusy}
        onClick={() => onEnableProvider(provider)}
      >
        {enableBusy ? "Turning on…" : `Turn ${provider} on`}
      </Button>
    </span>
  );

  return (
    <div className="flex flex-col gap-[10px] border-b border-line-hair-2 bg-sunken px-[16px] py-[12px]">
      <div
        role="radiogroup"
        aria-label="Provider"
        className="flex flex-wrap gap-[6px]"
      >
        {providers.map(([id]) => (
          <Button
            key={id}
            role="radio"
            aria-checked={provider === id}
            variant={provider === id ? "primarySm" : "secondarySm"}
            onClick={() => onChangeProvider(id)}
          >
            {id}
          </Button>
        ))}
      </div>

      {plan === null ? (
        <span className="text-base text-tertiary">
          {llm.isPending
            ? "Loading providers…"
            : `No provider named ${provider}.`}
        </span>
      ) : plan.action === "keyless" ? (
        <div className="flex flex-col gap-[6px]">
          <span className="text-base leading-[1.5] text-secondary">
            {plan.reason}
          </span>
          {info?.enabled === false && turnOn}
        </div>
      ) : (
        <>
          {plan.action === "wait-switch" && (
            <span className="text-base leading-[1.5] text-tertiary">
              {plan.reason}
            </span>
          )}
          {plan.action === "refuse-disabled" && (
            <div className="flex flex-col gap-[6px] rounded-md border border-amber-line bg-amber-surface px-[10px] py-[8px]">
              <span className="text-base leading-[1.5] text-amber-ink">
                {plan.reason}
              </span>
              {turnOn}
            </div>
          )}

          <span className="flex flex-wrap items-center gap-[6px]">
            <input
              type="password"
              aria-label="API key"
              autoComplete="off"
              spellCheck={false}
              placeholder="Paste the key"
              value={secret}
              onChange={(event) => editSecret(event.target.value)}
              className={cn("min-w-[280px] flex-1 font-mono", FIELD)}
            />
            <Button
              variant="secondarySm"
              disabled={secret.trim().length === 0 || validate.isPending}
              onClick={onCheck}
            >
              {validate.isPending ? "Checking…" : "Check key"}
            </Button>
          </span>

          {check !== null && (
            <span
              className={cn(
                "text-base leading-[1.5]",
                check.ok ? "text-green-ink" : "text-amber-ink",
              )}
            >
              {check.text}
            </span>
          )}

          <span className="flex flex-wrap items-center gap-[6px]">
            <select
              aria-label="Priority"
              value={priority}
              onChange={(event) =>
                setPriority(event.target.value as KeyPriorityValue)
              }
              className={FIELD}
            >
              {KEY_PRIORITIES.map((value) => (
                <option key={value} value={value}>
                  {value}
                </option>
              ))}
            </select>
            <select
              aria-label="Source"
              value={source}
              onChange={(event) =>
                setSource(event.target.value as KeySourceValue)
              }
              className={FIELD}
            >
              {KEY_SOURCE_OPTIONS.map((option) => (
                <option key={option.value} value={option.value}>
                  {option.label}
                </option>
              ))}
            </select>
            <input
              aria-label="Notes (optional)"
              placeholder="Notes (optional)"
              value={notes}
              onChange={(event) => setNotes(event.target.value)}
              className={cn("min-w-[180px] flex-1", FIELD)}
            />
          </span>

          {failure !== null && (
            <span role="alert" className="text-base leading-[1.5] text-red-ink">
              {failure}
            </span>
          )}
        </>
      )}

      <span className="flex items-center gap-[6px]">
        {plan !== null && plan.action !== "keyless" && (
          <Button
            variant="primarySm"
            disabled={plan.action !== "save" || upsert.isPending}
            title={plan.action === "save" ? undefined : plan.reason}
            onClick={onSave}
          >
            {upsert.isPending ? "Saving…" : "Save key"}
          </Button>
        )}
        <Button variant="ghostSm" onClick={onCancel}>
          {plan?.action === "keyless" ? "Close" : "Cancel"}
        </Button>
      </span>
    </div>
  );
}
