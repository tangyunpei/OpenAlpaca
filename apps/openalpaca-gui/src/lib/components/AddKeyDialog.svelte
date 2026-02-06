<script lang="ts">
  import { addOrUpdateKey, validateKey } from "$lib/api/settings";
  import type { KeyValidationResult } from "$lib/types";

  interface Props {
    provider: string;
    onclose: () => void;
    onAdded: () => void;
  }

  let { provider, onclose, onAdded }: Props = $props();

  let keyId = $state("");
  let secret = $state("");
  let source = $state<string>("api_console");
  let priority = $state<string>("primary");
  let notes = $state("");
  let tier = $state<string | null>(null);

  let validationResult = $state<KeyValidationResult | null>(null);
  let isValidating = $state(false);
  let isSubmitting = $state(false);
  let errorMsg = $state<string | null>(null);

  async function handleValidate() {
    if (!secret.trim()) return;
    isValidating = true;
    errorMsg = null;
    try {
      validationResult = await validateKey({ provider, secret: secret.trim() });
      if (validationResult.tier) {
        tier = validationResult.tier;
      }
      if (validationResult.detected_source) {
        source = validationResult.detected_source;
      }
    } catch (e) {
      errorMsg = e instanceof Error ? e.message : String(e);
    } finally {
      isValidating = false;
    }
  }

  async function handleSubmit() {
    if (!secret.trim()) return;
    isSubmitting = true;
    errorMsg = null;
    try {
      await addOrUpdateKey({
        provider,
        key: {
          id: keyId.trim() || undefined,
          secret: secret.trim(),
          tier: tier ?? undefined,
          priority,
          source,
          notes: notes.trim() || undefined,
        },
      });
      onAdded();
    } catch (e) {
      errorMsg = e instanceof Error ? e.message : String(e);
    } finally {
      isSubmitting = false;
    }
  }

  function handleBackdropClick(e: MouseEvent) {
    if (e.target === e.currentTarget) onclose();
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
<div class="modal-backdrop" onclick={handleBackdropClick}>
  <div class="modal">
    <h3>Add API Key — {provider}</h3>

    <div class="form-group">
      <label for="key-id">Key ID (optional)</label>
      <input id="key-id" type="text" bind:value={keyId} placeholder="e.g. my_key_1" />
    </div>

    <div class="form-group">
      <label for="key-secret">Secret</label>
      <div class="secret-row">
        <input id="key-secret" type="password" bind:value={secret} placeholder="sk-..." />
        <button class="validate-btn" onclick={handleValidate} disabled={isValidating || !secret.trim()}>
          {isValidating ? "..." : "Validate"}
        </button>
      </div>
    </div>

    {#if validationResult}
      <div class="validation-result" class:valid={validationResult.valid} class:invalid={!validationResult.valid}>
        <span>{validationResult.valid ? "Valid" : "Invalid"}</span>
        {#if validationResult.tier}
          <span class="tag">Tier: {validationResult.tier}</span>
        {/if}
        {#if validationResult.detected_source}
          <span class="tag">Source: {validationResult.detected_source}</span>
        {/if}
      </div>
    {/if}

    <div class="form-group">
      <label>Source</label>
      <div class="radio-group">
        <label class="radio"><input type="radio" bind:group={source} value="api_console" /> API Console</label>
        <label class="radio"><input type="radio" bind:group={source} value="claude_code" /> Claude Code</label>
        <label class="radio"><input type="radio" bind:group={source} value="claude_max_pro" /> Max/Pro</label>
        <label class="radio"><input type="radio" bind:group={source} value="other" /> Other</label>
      </div>
    </div>

    <div class="form-group">
      <label>Priority</label>
      <div class="radio-group">
        <label class="radio"><input type="radio" bind:group={priority} value="primary" /> Primary</label>
        <label class="radio"><input type="radio" bind:group={priority} value="fallback" /> Fallback</label>
      </div>
    </div>

    <div class="form-group">
      <label for="key-notes">Notes (optional)</label>
      <textarea id="key-notes" bind:value={notes} rows="2" placeholder="Any notes about this key..."></textarea>
    </div>

    {#if errorMsg}
      <div class="error-msg">{errorMsg}</div>
    {/if}

    <div class="modal-actions">
      <button class="secondary" onclick={onclose}>Cancel</button>
      <button onclick={handleSubmit} disabled={isSubmitting || !secret.trim()}>
        {isSubmitting ? "Adding..." : "Add Key"}
      </button>
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    top: 0;
    left: 0;
    width: 100%;
    height: 100%;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    justify-content: center;
    align-items: center;
    z-index: 1000;
  }

  .modal {
    background: var(--surface);
    padding: 24px;
    border-radius: 12px;
    width: 90%;
    max-width: 480px;
    box-shadow: 0 4px 20px rgba(0, 0, 0, 0.3);
    border: 1px solid rgba(255, 255, 255, 0.08);
    max-height: 90vh;
    overflow-y: auto;
  }

  .modal h3 {
    margin: 0 0 16px;
    color: var(--text);
    font-size: 1.1rem;
  }

  .form-group {
    margin-bottom: 14px;
  }

  .form-group label {
    display: block;
    font-size: 0.8rem;
    color: var(--text-dim);
    margin-bottom: 4px;
    font-weight: 500;
  }

  .form-group input[type="text"],
  .form-group input[type="password"],
  .form-group textarea {
    width: 100%;
    padding: 8px 10px;
    background: var(--bg);
    border: 1px solid rgba(255, 255, 255, 0.08);
    color: var(--text);
    border-radius: 6px;
    box-sizing: border-box;
    font-size: 0.9rem;
  }

  .form-group textarea {
    resize: vertical;
    font-family: inherit;
  }

  .secret-row {
    display: flex;
    gap: 8px;
  }

  .secret-row input {
    flex: 1;
  }

  .validate-btn {
    padding: 8px 14px !important;
    font-size: 0.8rem !important;
    white-space: nowrap;
  }

  .radio-group {
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
  }

  .radio {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 0.85rem;
    color: var(--text);
    cursor: pointer;
  }

  .radio input[type="radio"] {
    accent-color: var(--accent);
  }

  .validation-result {
    padding: 8px 12px;
    border-radius: 6px;
    font-size: 0.8rem;
    margin-bottom: 12px;
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
  }

  .validation-result.valid {
    background: rgba(16, 185, 129, 0.15);
    border: 1px solid rgba(16, 185, 129, 0.3);
    color: var(--success);
  }

  .validation-result.invalid {
    background: rgba(239, 68, 68, 0.15);
    border: 1px solid rgba(239, 68, 68, 0.3);
    color: var(--error);
  }

  .tag {
    background: rgba(255, 255, 255, 0.1);
    padding: 2px 6px;
    border-radius: 4px;
    font-size: 0.75rem;
  }

  .error-msg {
    color: var(--error);
    font-size: 0.8rem;
    margin-bottom: 12px;
    padding: 6px 10px;
    background: rgba(239, 68, 68, 0.1);
    border-radius: 4px;
  }

  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
    margin-top: 16px;
  }
</style>
