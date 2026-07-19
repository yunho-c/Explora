<script lang="ts">
  import SearchIcon from "@lucide/svelte/icons/search";

  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import { Input } from "$lib/components/ui/input";
  import { Progress } from "$lib/components/ui/progress";

  import ExplorerSidebar from "./ExplorerSidebar.svelte";
  import ExplorerTabs from "./ExplorerTabs.svelte";
  import ExplorerToolbar from "./ExplorerToolbar.svelte";
  import FileGrid from "./FileGrid.svelte";
  import FileList from "./FileList.svelte";
  import QuickPreview from "./QuickPreview.svelte";

  let { state }: { state: ExplorerState } = $props();

  const handleKeydown = (event: KeyboardEvent) => {
    const target = event.target;
    const isTextInput =
      target instanceof Element &&
      target.matches("input, textarea, [contenteditable='true']");

    if (isTextInput && event.key !== "Escape") return;

    if (event.key === " " && state.selectedEntryId) {
      event.preventDefault();
      void state.openPreview();
    } else if (event.key === "Escape" && state.previewOpen) {
      state.closePreview();
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      state.moveSelection(1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      state.moveSelection(-1);
    }
  };
</script>

<svelte:window onkeydown={handleKeydown} />

<div
  class="flex h-screen min-h-0 overflow-hidden bg-background text-foreground"
>
  <ExplorerSidebar {state} />

  <main class="flex min-w-0 flex-1 flex-col" aria-label="File explorer">
    <ExplorerTabs {state} />
    <ExplorerToolbar {state} />

    <div class="relative min-h-0 flex-1 overflow-auto">
      {#if state.loading}
        <Progress class="absolute inset-x-0 top-0 z-10 h-0.5" />
      {/if}

      <div class="relative p-3 sm:hidden">
        <SearchIcon
          class="pointer-events-none absolute top-1/2 left-5 size-4 -translate-y-1/2 text-muted-foreground"
        />
        <Input
          bind:value={state.searchQuery}
          aria-label="Search this location"
          placeholder="Search"
          class="pl-8"
        />
      </div>

      {#if state.errorMessage}
        <div
          class="m-4 rounded-lg border border-destructive/30 bg-destructive/5 p-4 text-sm text-destructive"
          role="alert"
        >
          {state.errorMessage}
        </div>
      {/if}

      {#if state.warningMessage}
        <div
          class="mx-4 mt-4 rounded-lg border bg-muted/50 p-3 text-sm text-muted-foreground"
          role="status"
        >
          {state.warningMessage}
        </div>
      {/if}

      {#if !state.errorMessage && !state.loading && state.visibleEntries.length === 0}
        <div class="grid min-h-72 place-items-center p-8 text-center">
          <div>
            <p class="font-medium">
              {state.searchQuery
                ? "No matching items"
                : "This location is empty"}
            </p>
            <p class="mt-1 text-sm text-muted-foreground">
              {state.searchQuery
                ? "Try a different search term."
                : "There are no items in this folder."}
            </p>
          </div>
        </div>
      {:else if state.viewMode === "list"}
        <FileList {state} />
      {:else}
        <FileGrid {state} />
      {/if}
    </div>

    <footer
      class="flex h-8 items-center justify-between border-t px-3 text-xs text-muted-foreground"
    >
      <span
        >{state.visibleEntries.length}
        {state.visibleEntries.length === 1 ? "item" : "items"}</span
      >
      <span class="truncate pl-4"
        >{state.activeDirectory?.displayPath ?? "Loading locations…"}</span
      >
    </footer>
  </main>

  <QuickPreview {state} />
</div>
