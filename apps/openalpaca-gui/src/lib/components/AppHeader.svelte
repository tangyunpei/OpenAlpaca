<script lang="ts">
  interface Props {
    statusState: string;
    info: { baseUrl: string; instanceId: string } | null;
  }

  let { statusState, info }: Props = $props();
</script>

<header class="header">
  <div class="brand">
    <h1>🦙 OpenAlpaca</h1>
    {#if info}
      <div class="info-tag">
        <span class="instance">{info.instanceId.slice(0, 8)}</span>
        <span class="url">{info.baseUrl}</span>
      </div>
    {/if}
  </div>
  <div
    class="status"
    class:connected={statusState === "connected"}
    class:error={statusState === "error"}
  >
    <span class="dot"></span>
    <span class="text">{statusState}</span>
  </div>
</header>

<style>
  .header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 25px;
    padding-bottom: 15px;
    border-bottom: 2px solid var(--primary);
    flex-wrap: wrap;
    gap: 10px;
  }

  .brand {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }

  h1 {
    margin: 0;
    font-size: 2rem;
    font-weight: 800;
    color: var(--text);
    letter-spacing: -0.5px;
  }

  .info-tag {
    display: flex;
    gap: 12px;
    font-family: "Fira Code", monospace;
    font-size: 0.75rem;
    color: var(--text-dim);
    flex-wrap: wrap;
  }

  @media (max-width: 480px) {
    h1 {
      font-size: 1.5rem;
    }
  }

  .info-tag .instance {
    background: var(--primary);
    padding: 0 6px;
    border-radius: 4px;
    color: var(--text);
  }

  .status {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 16px;
    border-radius: 20px;
    background: var(--surface);
    font-size: 0.85rem;
    text-transform: capitalize;
  }

  .status .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--text-dim);
    animation: pulse 2s infinite;
  }

  .status.connected .dot {
    background: var(--success);
  }

  .status.error .dot {
    background: var(--error);
    animation: none;
  }

  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.5;
    }
  }
</style>
