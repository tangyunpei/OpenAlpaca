<script lang="ts">
  interface Tab {
    id: string;
    label: string;
  }

  interface Props {
    activeTab: string;
    tabs: Tab[];
    onTabChange: (id: string) => void;
  }

  let { activeTab, tabs, onTabChange }: Props = $props();
</script>

<div class="nav-tabs">
  {#each tabs as tab}
    <button
      class="tab-btn"
      class:active={activeTab === tab.id}
      onclick={() => onTabChange(tab.id)}
    >
      {tab.label}
    </button>
  {/each}
</div>

<style>
  .nav-tabs {
    display: flex;
    flex-wrap: wrap;
    background: rgba(0, 0, 0, 0.25);
    padding: 4px;
    border-radius: 10px;
    margin: 0 auto 24px auto;
    gap: 4px;
    width: fit-content;
    max-width: 100%;
    border: 1px solid rgba(255, 255, 255, 0.05);
    backdrop-filter: blur(10px);
    -webkit-backdrop-filter: blur(10px);
    box-sizing: border-box;
  }

  .tab-btn {
    padding: 8px 24px;
    border: none;
    background: transparent;
    color: var(--text-dim);
    cursor: pointer;
    font-size: 0.9rem;
    font-weight: 500;
    border-radius: 6px;
    transition: all 0.2s cubic-bezier(0.4, 0, 0.2, 1);
    white-space: nowrap;
  }

  @media (max-width: 480px) {
    .nav-tabs {
      width: 100%;
    }

    .tab-btn {
      flex: 1;
      padding: 8px 12px;
      font-size: 0.8rem;
      text-align: center;
    }
  }

  .tab-btn:hover {
    color: var(--text);
  }

  .tab-btn.active {
    background: rgba(255, 255, 255, 0.1);
    color: #fff;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.2);
  }
</style>
