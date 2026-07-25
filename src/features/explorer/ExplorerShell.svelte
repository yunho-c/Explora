<script lang="ts">
  import SearchIcon from "@lucide/svelte/icons/search";
  import ServerOffIcon from "@lucide/svelte/icons/server-off";
  import SquareTerminalIcon from "@lucide/svelte/icons/square-terminal";

  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import type { TerminalState } from "../../app/terminal-state.svelte";
  import type { WindowChromeController } from "../../app/window-chrome.svelte";
  import { Input } from "$lib/components/ui/input";
  import { Button } from "$lib/components/ui/button";
  import { Progress } from "$lib/components/ui/progress";
  import * as Resizable from "$lib/components/ui/resizable";

  import ExplorerSidebar from "./ExplorerSidebar.svelte";
  import ExplorerTabs from "./ExplorerTabs.svelte";
  import ExplorerToolbar from "./ExplorerToolbar.svelte";
  import FileGrid from "./FileGrid.svelte";
  import FileList from "./FileList.svelte";
  import QuickPreview from "./QuickPreview.svelte";
  import SshPromptDialog from "./SshPromptDialog.svelte";
  import SshTargetDialog from "./SshTargetDialog.svelte";
  import TerminalPane from "../terminal/TerminalPane.svelte";

  let {
    state,
    terminalState,
    windowChrome,
  }: {
    state: ExplorerState;
    terminalState: TerminalState;
    windowChrome: WindowChromeController;
  } = $props();

  const handleKeydown = (event: KeyboardEvent) => {
    const newTerminal =
      event.key === "`" &&
      event.ctrlKey &&
      !event.metaKey &&
      !event.altKey &&
      event.shiftKey;
    if (newTerminal) {
      event.preventDefault();
      void terminalState.newTerminal();
      return;
    }
    if (
      event.ctrlKey &&
      !event.metaKey &&
      !event.altKey &&
      (event.key === "PageDown" || event.key === "PageUp")
    ) {
      event.preventDefault();
      terminalState.selectRelativeSession(event.key === "PageDown" ? 1 : -1);
      return;
    }
    const toggleTerminal =
      event.key === "`" &&
      event.ctrlKey &&
      !event.metaKey &&
      !event.altKey &&
      !event.shiftKey;
    if (toggleTerminal) {
      event.preventDefault();
      terminalState.toggleVisibility();
      return;
    }

    const target = event.target;
    const terminalHasFocus =
      target instanceof Element &&
      target.closest("[data-terminal-surface]") !== null;
    if (terminalHasFocus) return;

    const refreshShortcut =
      event.key === "F5" ||
      (event.key.toLocaleLowerCase() === "r" &&
        (event.metaKey || event.ctrlKey));
    if (refreshShortcut) {
      event.preventDefault();
      void state.refreshDirectory();
      return;
    }

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

    {#snippet fileView()}
      <div class="relative h-full min-h-0 overflow-auto">
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
    {/snippet}

    <div class="min-h-0 flex-1">
      {#if terminalState.visible}
        <Resizable.PaneGroup
          direction="vertical"
          onLayoutChange={(layout) => {
            if (layout.length === 2) {
              terminalState.setPaneHeightPercent(layout[1]);
            }
          }}
        >
          <Resizable.Pane
            defaultSize={100 - terminalState.paneHeightPercent}
            minSize={30}
            order={1}
          >
            {@render fileView()}
          </Resizable.Pane>
          <Resizable.Handle
            withHandle
            aria-label="Resize integrated terminal"
          />
          <Resizable.Pane
            defaultSize={terminalState.paneHeightPercent}
            minSize={20}
            maxSize={70}
            order={2}
          >
            <TerminalPane state={terminalState} />
          </Resizable.Pane>
        </Resizable.PaneGroup>
      {:else}
        {@render fileView()}
      {/if}
    </div>

    <footer
      class="flex h-8 items-center gap-3 border-t px-3 text-xs text-muted-foreground"
    >
      <span
        >{state.visibleEntries.length}
        {state.visibleEntries.length === 1 ? "item" : "items"}</span
      >
      <span class="min-w-0 flex-1 truncate text-right"
        >{state.activeDirectory?.displayPath ?? "Loading locations…"}</span
      >
      <button
        type="button"
        class="flex h-6 shrink-0 items-center gap-1.5 rounded px-1.5 outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
        class:bg-accent={terminalState.visible}
        class:text-foreground={terminalState.visible}
        aria-pressed={terminalState.visible}
        aria-label="Show or hide terminal (Ctrl+`)"
        title="Show or hide terminal (Ctrl+`)"
        onclick={() => terminalState.toggleVisibility()}
      >
        <SquareTerminalIcon class="size-3.5" />
        <span>Terminal</span>
        {#if terminalState.sessions.length > 0}
          <span class="tabular-nums">{terminalState.sessions.length}</span>
        {/if}
      </button>
    </footer>
  </main>

  <QuickPreview {state} />
  <SshTargetDialog {state} />
  <SshPromptDialog {state} />
</div>
