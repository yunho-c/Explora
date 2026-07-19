<script lang="ts">
  import PlusIcon from "@lucide/svelte/icons/plus";
  import XIcon from "@lucide/svelte/icons/x";

  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import { Button } from "$lib/components/ui/button";

  let { state }: { state: ExplorerState } = $props();
</script>

<div
  class="flex h-11 items-end gap-1 overflow-x-auto border-b bg-muted/30 px-2 pt-2"
  role="tablist"
  aria-label="Open locations"
>
  {#each state.tabs as tab (tab.id)}
    <div
      class={tab.id === state.activeTabId
        ? "flex h-9 max-w-52 min-w-32 items-center rounded-t-lg border border-b-background bg-background px-1"
        : "flex h-9 max-w-52 min-w-32 items-center rounded-t-lg border border-transparent px-1 text-muted-foreground"}
    >
      <button
        type="button"
        role="tab"
        aria-selected={tab.id === state.activeTabId}
        class="min-w-0 flex-1 truncate px-2 text-left text-sm"
        onclick={() => void state.activateTab(tab.id)}
      >
        {tab.title}
      </button>
      <Button
        variant="ghost"
        size="icon-xs"
        disabled={state.tabs.length === 1}
        aria-label={`Close ${tab.title} tab`}
        onclick={() => void state.closeTab(tab.id)}
      >
        <XIcon />
      </Button>
    </div>
  {/each}
  <Button
    variant="ghost"
    size="icon-sm"
    aria-label="Open a new tab"
    onclick={() => void state.openTab()}
  >
    <PlusIcon />
  </Button>
</div>
