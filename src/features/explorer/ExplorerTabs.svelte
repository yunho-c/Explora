<script lang="ts">
  import PlusIcon from "@lucide/svelte/icons/plus";
  import XIcon from "@lucide/svelte/icons/x";

  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import type { WindowChromeMode } from "../../app/window-chrome.svelte";
  import { Button } from "$lib/components/ui/button";

  let {
    state,
    chromeMode,
    sidebarCollapsed,
  }: {
    state: ExplorerState;
    chromeMode: WindowChromeMode;
    sidebarCollapsed: boolean;
  } = $props();

  const customChrome = $derived(
    chromeMode === "activating" || chromeMode === "custom",
  );
</script>

<div
  class="explora-titlebar-content explora-titlebar-tabs flex h-8 shrink-0 items-stretch gap-px overflow-x-auto border-b bg-muted/30"
  class:explora-titlebar-tabs-collapsed={sidebarCollapsed}
  role="tablist"
  aria-label="Open locations"
>
  {#each state.tabs as tab (tab.id)}
    <div
      class={tab.id === state.activeTabId
        ? "flex h-full max-w-52 min-w-32 items-center border-b-2 border-foreground/70 bg-background/80 px-1"
        : "flex h-full max-w-52 min-w-32 items-center border-b-2 border-transparent px-1 text-muted-foreground hover:bg-background/45 hover:text-foreground"}
    >
      <button
        type="button"
        role="tab"
        aria-selected={tab.id === state.activeTabId}
        class="h-full min-w-0 flex-1 truncate px-2 text-left text-xs font-medium outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset"
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
    size="icon-xs"
    class="m-1 shrink-0"
    aria-label="Open a new tab"
    onclick={() => void state.openTab()}
  >
    <PlusIcon />
  </Button>
  <div
    class="min-w-6 flex-1"
    data-tauri-drag-region={customChrome ? "" : undefined}
    aria-hidden="true"
  ></div>
</div>
