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

<div class="flex flex-wrap bg-black/25 p-1 rounded-[10px] mx-auto mb-6 gap-1 w-fit max-w-full border border-white/5 backdrop-blur-[10px] max-[480px]:w-full">
  {#each tabs as tab}
    <button
      class="tab-btn px-6 py-2 border-none bg-transparent text-muted-foreground cursor-pointer text-sm font-medium rounded-md transition-all duration-200 whitespace-nowrap hover:text-foreground max-[480px]:flex-1 max-[480px]:px-3 max-[480px]:text-xs max-[480px]:text-center {activeTab === tab.id ? 'bg-white/10 text-white shadow-sm' : ''}"
      onclick={() => onTabChange(tab.id)}
    >
      {tab.label}
    </button>
  {/each}
</div>
