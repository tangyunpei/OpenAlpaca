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
  let validatedSecret = $state(""); // the secret that was last validated
  let isValidating = $state(false);
  let isSubmitting = $state(false);
  let errorMsg = $state<string | null>(null);

  // Clear stale validation when secret changes
  $effect(() => {
    const _ = secret; // subscribe to secret changes
    if (secret.trim() !== validatedSecret) {
      validationResult = null;
      validatedSecret = "";
    }
  });

  // Auto-set source when sk-ant-oat* is detected
  $effect(() => {
    if (provider === "anthropic" && secret.trim().startsWith("sk-ant-oat")) {
      source = "claude_code";
    }
  });

  // Reactive client-side format check
  let formatWarning = $derived.by(() => {
    const trimmed = secret.trim();
    if (!trimmed) return null;
    if (provider === "anthropic") {
      if (trimmed.startsWith("sk-ant-oat")) {
        if (trimmed.length < 80) return `Setup token too short (${trimmed.length} chars, expected >= 80).`;
        return null; // well-formed setup token
      }
      if (!trimmed.startsWith("sk-ant-")) return "Anthropic API keys start with 'sk-ant-'. Setup tokens start with 'sk-ant-oat'.";
      if (trimmed.length < 40) return `Key too short (${trimmed.length} chars, expected >= 40).`;
    } else if (provider === "openai") {
      if (trimmed.startsWith("sk-ant-")) return "This looks like an Anthropic key, not an OpenAI key.";
      if (!trimmed.startsWith("sk-")) return "OpenAI API keys start with 'sk-'.";
      if (trimmed.length < 20) return `Key too short (${trimmed.length} chars, expected >= 20).`;
    }
    return null;
  });

  let needsValidation = $derived(provider === "anthropic" || provider === "openai");
  let isValidated = $derived(validatedSecret === secret.trim() && validationResult?.valid === true);

  async function handleValidate() {
    if (!secret.trim()) return;
    isValidating = true;
    errorMsg = null;
    try {
      validationResult = await validateKey({ provider, secret: secret.trim() });
      validatedSecret = secret.trim(); // record what we validated
      if (validationResult.tier) {
        tier = validationResult.tier;
      }
      if (validationResult.detected_source) {
        source = validationResult.detected_source;
      }
    } catch (e) {
      errorMsg = e instanceof Error ? e.message : String(e);
      validatedSecret = "";
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
</script>

<div class="fixed inset-0 z-[1000] flex items-center justify-center bg-black/60" role="presentation">
  <!-- Backdrop: button is inherently accessible; dialog stays above it via z-index. -->
  <button
    type="button"
    class="absolute inset-0 z-0 cursor-default"
    aria-label="Close dialog"
    tabindex="-1"
    onclick={onclose}
  ></button>

  <div class="relative z-10 w-[90%] max-w-[480px] max-h-[90vh] overflow-y-auto rounded-xl border border-border bg-surface p-6 shadow-[0_4px_20px_rgba(0,0,0,0.3)]">
    <h3 class="m-0 mb-4 text-foreground text-[1.1rem] font-semibold">Add API Key — {provider}</h3>

    <div class="mb-3.5">
      <label for="key-id" class="block text-[0.8rem] text-muted-foreground mb-1 font-medium">Key ID (optional)</label>
      <input
        id="key-id"
        type="text"
        bind:value={keyId}
        placeholder="e.g. my_key_1"
        class="w-full px-2.5 py-2 bg-background border border-border text-foreground rounded-md text-[0.9rem] placeholder:text-muted-foreground focus:outline-none focus:border-accent"
      />
    </div>

    <div class="mb-3.5">
      <label for="key-secret" class="block text-[0.8rem] text-muted-foreground mb-1 font-medium">Secret</label>
      <div class="flex gap-2">
        <input
          id="key-secret"
          type="password"
          bind:value={secret}
          placeholder="sk-..."
          class="flex-1 px-2.5 py-2 bg-background border border-border text-foreground rounded-md text-[0.9rem] placeholder:text-muted-foreground focus:outline-none focus:border-accent"
        />
        <button
          class="px-3.5 py-2 text-[0.8rem] rounded-md bg-primary text-primary-foreground hover:bg-primary/80 disabled:opacity-50 cursor-pointer whitespace-nowrap"
          onclick={handleValidate}
          disabled={isValidating || !secret.trim() || !!formatWarning}
        >
          {isValidating ? "..." : "Validate"}
        </button>
      </div>
    </div>

    {#if formatWarning}
      <div class="flex items-center gap-2 px-3 py-2 rounded-md mb-3 text-[0.8rem] bg-warning/15 border border-warning/30 text-warning">
        <span>{formatWarning}</span>
      </div>
    {/if}

    {#if validationResult}
      <div class="flex items-center gap-2 flex-wrap px-3 py-2 rounded-md mb-3 text-[0.8rem] {validationResult.valid ? 'bg-success/15 border border-success/30 text-success' : 'bg-danger/15 border border-danger/30 text-danger'}">
        <span>{validationResult.valid ? "Valid" : "Invalid"}</span>
        {#if validationResult.format_error}
          <span class="text-[0.75rem]">— {validationResult.format_error}</span>
        {/if}
        {#if validationResult.tier}
          <span class="bg-white/10 px-1.5 rounded text-xs">{validationResult.tier}</span>
        {/if}
        {#if validationResult.detected_source}
          <span class="bg-white/10 px-1.5 rounded text-xs">Source: {validationResult.detected_source}</span>
        {/if}
      </div>
    {/if}

    <fieldset class="mb-3.5 border-0 p-0 m-0">
      <legend class="block text-[0.8rem] text-muted-foreground mb-1 font-medium">Source</legend>
      <div class="flex flex-wrap gap-3">
        <label class="flex items-center gap-1 text-[0.85rem] text-foreground cursor-pointer"><input type="radio" bind:group={source} value="api_console" class="accent-accent" /> API Console</label>
        <label class="flex items-center gap-1 text-[0.85rem] text-foreground cursor-pointer"><input type="radio" bind:group={source} value="claude_code" class="accent-accent" /> Claude Code</label>
        <label class="flex items-center gap-1 text-[0.85rem] text-foreground cursor-pointer"><input type="radio" bind:group={source} value="codex" class="accent-accent" /> Codex</label>
        <label class="flex items-center gap-1 text-[0.85rem] text-foreground cursor-pointer"><input type="radio" bind:group={source} value="claude_max_pro" class="accent-accent" /> Max/Pro</label>
        <label class="flex items-center gap-1 text-[0.85rem] text-foreground cursor-pointer"><input type="radio" bind:group={source} value="environment" class="accent-accent" /> Environment</label>
        <label class="flex items-center gap-1 text-[0.85rem] text-foreground cursor-pointer"><input type="radio" bind:group={source} value="other" class="accent-accent" /> Other</label>
      </div>
    </fieldset>

    <fieldset class="mb-3.5 border-0 p-0 m-0">
      <legend class="block text-[0.8rem] text-muted-foreground mb-1 font-medium">Priority</legend>
      <div class="flex flex-wrap gap-3">
        <label class="flex items-center gap-1 text-[0.85rem] text-foreground cursor-pointer"><input type="radio" bind:group={priority} value="primary" class="accent-accent" /> Primary</label>
        <label class="flex items-center gap-1 text-[0.85rem] text-foreground cursor-pointer"><input type="radio" bind:group={priority} value="fallback" class="accent-accent" /> Fallback</label>
      </div>
    </fieldset>

    <div class="mb-3.5">
      <label for="key-notes" class="block text-[0.8rem] text-muted-foreground mb-1 font-medium">Notes (optional)</label>
      <textarea
        id="key-notes"
        bind:value={notes}
        rows="2"
        placeholder="Any notes about this key..."
        class="w-full px-2.5 py-2 bg-background border border-border text-foreground rounded-md text-[0.9rem] resize-y font-sans placeholder:text-muted-foreground focus:outline-none focus:border-accent"
      ></textarea>
    </div>

    {#if errorMsg}
      <div class="text-danger text-[0.8rem] mb-3 px-2.5 py-1.5 bg-danger/10 rounded">{errorMsg}</div>
    {/if}

    <div class="flex justify-end gap-2.5 mt-4">
      <button
        class="px-4 py-2 rounded-lg bg-white/5 border border-border text-muted-foreground text-sm hover:bg-white/10 cursor-pointer"
        onclick={onclose}
      >Cancel</button>
      <button
        class="px-4 py-2 rounded-lg bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/80 disabled:opacity-50 cursor-pointer"
        onclick={handleSubmit}
        disabled={isSubmitting || !secret.trim() || !!formatWarning || (needsValidation && !isValidated)}
      >
        {isSubmitting ? "Adding..." : "Add Key"}
      </button>
    </div>
  </div>
</div>
