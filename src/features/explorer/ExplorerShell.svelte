<script lang="ts">
  import SearchIcon from "@lucide/svelte/icons/search";
  import ServerOffIcon from "@lucide/svelte/icons/server-off";

  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import type { WindowChromeController } from "../../app/window-chrome.svelte";
  import { Input } from "$lib/components/ui/input";
  import { Button } from "$lib/components/ui/button";
  import { Progress } from "$lib/components/ui/progress";
  import { deletionShortcut, isRenameShortcut } from "$lib/platform-shortcuts";

  import ExplorerSidebar from "./ExplorerSidebar.svelte";
  import ExplorerTabs from "./ExplorerTabs.svelte";
  import ExplorerToolbar from "./ExplorerToolbar.svelte";
  import FileGrid from "./FileGrid.svelte";
  import FileList from "./FileList.svelte";
  import FileOperationConfirmationDialog from "./FileOperationConfirmationDialog.svelte";
  import MoveDestinationDialog from "./MoveDestinationDialog.svelte";
  import QuickPreview from "./QuickPreview.svelte";
  import SshPromptDialog from "./SshPromptDialog.svelte";
  import SshTargetDialog from "./SshTargetDialog.svelte";

  let {
    state,
    windowChrome,
  }: {
    state: ExplorerState;
    windowChrome: WindowChromeController;
  } = $props();

  const handleKeydown = (event: KeyboardEvent) => {
    if (
      isRenameShortcut(event) &&
      state.selectedEntry?.capabilities.rename &&
      state.fileOperations.activeEntryId === null
    ) {
      event.preventDefault();
      state.startRename();
      return;
    }

    const refreshShortcut =
      event.key === "F5" ||
      (event.key.toLocaleLowerCase() === "r" &&
        (event.metaKey || event.ctrlKey));
    if (refreshShortcut) {
      event.preventDefault();
      void state.refreshDirectory();
      return;
    }

    const target = event.target;
    const isInteractiveControl =
      target instanceof Element &&
      target.matches(
        "a[href], button, input, select, textarea, [contenteditable='true'], [role='button'], [role='checkbox'], [role='menuitem'], [role='switch']",
      );
    const isPreviewText =
      target instanceof Element && target.matches("[data-preview-text]");
    const isPreviewDocument =
      target instanceof Element &&
      target.closest("[data-preview-document]") !== null;

    if (
      state.previewOpen &&
      isPreviewDocument &&
      (event.key === "ArrowDown" || event.key === "ArrowUp")
    ) {
      return;
    }

    if (
      state.previewOpen &&
      (event.key === "ArrowDown" || event.key === "ArrowUp")
    ) {
      event.preventDefault();
      state.moveSelection(event.key === "ArrowDown" ? 1 : -1);
      return;
    }

    if (isInteractiveControl && !isPreviewText && event.key !== "Escape")
      return;

    const deletion = deletionShortcut(event);
    if (
      deletion === "trash" &&
      state.selectedEntry?.capabilities.trash &&
      state.fileOperations.activeEntryId === null
    ) {
      event.preventDefault();
      void state.moveSelectedToTrash();
      return;
    }
    if (
      deletion === "deletePermanently" &&
      state.selectedEntry?.capabilities.deletePermanently &&
      state.fileOperations.activeEntryId === null
    ) {
      event.preventDefault();
      void state.deleteSelectedPermanently();
      return;
    }

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
  data-window-chrome={windowChrome.mode}
  data-sidebar-collapsed={state.sidebarCollapsed}
>
  <ExplorerSidebar {state} chromeMode={windowChrome.mode} />

  <main class="flex min-w-0 flex-1 flex-col" aria-label="File explorer">
    <ExplorerTabs
      {state}
      chromeMode={windowChrome.mode}
      sidebarCollapsed={state.sidebarCollapsed}
    />
    <ExplorerToolbar {state} />

    <div class="relative min-h-0 flex-1 overflow-auto">
      {#if state.loading || state.fileOperations.activeEntryId}
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

      {#if state.activeSshLocationOffline}
        <div
          class="mx-4 mt-4 flex items-center gap-3 rounded-lg border bg-muted/50 p-3 text-sm"
          role="status"
        >
          <ServerOffIcon class="size-4 shrink-0 text-muted-foreground" />
          <div class="min-w-0 flex-1">
            <p class="font-medium">Remote location is offline</p>
            <p class="truncate text-xs text-muted-foreground">
              Your folder and tab history are preserved.
            </p>
          </div>
          <Button
            size="sm"
            disabled={Boolean(state.connectingTargetId)}
            onclick={() => void state.reconnectActiveSshLocation()}
          >
            Reconnect
          </Button>
        </div>
      {/if}

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

      {#if state.fileOperations.errorMessage}
        <div
          class="mx-4 mt-4 flex items-center justify-between gap-3 rounded-lg border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive"
          role="alert"
        >
          <span>{state.fileOperations.errorMessage}</span>
          <Button
            variant="ghost"
            size="sm"
            class="text-destructive hover:text-destructive"
            onclick={() => state.fileOperations.clearError()}>Dismiss</Button
          >
        </div>
      {/if}

      {#if state.fileOperations.activeAction && state.fileOperations.activeEntryName && state.fileOperations.progress && state.fileOperations.progress.totalItems > 1}
        <div
          class="mx-4 mt-4 rounded-lg border bg-muted/50 p-3 text-sm"
          role="status"
          aria-live="polite"
        >
          <span class="font-medium">{state.fileOperations.activeAction}</span>
          <span class="text-muted-foreground">
            “{state.fileOperations.activeEntryName}” · {state.fileOperations
              .progress.completedItems} of {state.fileOperations.progress
              .totalItems} items
          </span>
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
  <FileOperationConfirmationDialog {state} />
  <MoveDestinationDialog {state} />
  <SshTargetDialog {state} />
  <SshPromptDialog {state} />
</div>
