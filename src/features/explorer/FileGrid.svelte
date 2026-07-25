<script lang="ts">
  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import * as ContextMenu from "$lib/components/ui/context-menu";
  import { isRenameShortcut } from "$lib/platform-shortcuts";

  import FileGlyph from "./FileGlyph.svelte";
  import RenameInput from "./RenameInput.svelte";

  let { state }: { state: ExplorerState } = $props();
</script>

<ContextMenu.Root>
  <ContextMenu.Trigger>
    <div
      class="grid grid-cols-[repeat(auto-fill,minmax(8.5rem,1fr))] gap-3 p-4"
      role="grid"
      aria-label="Files"
      aria-multiselectable="true"
    >
      {#each state.visibleEntries as entry (entry.reference.id)}
        {#if state.renamingEntryId === entry.reference.id}
          <div
            role="gridcell"
            aria-selected="true"
            class="flex min-h-32 flex-col items-center justify-center gap-3 rounded-xl bg-muted p-3 text-center ring-2 ring-ring"
          >
            <FileGlyph kind={entry.contentKind} />
            <RenameInput {state} {entry} compact />
          </div>
        {:else}
          <button
            type="button"
            role="gridcell"
            aria-selected={state.isEntrySelected(entry.reference.id)}
            draggable="true"
            data-cut={state.isEntryCut(entry.reference.id) || undefined}
            data-drop-target={entry.directory &&
            state.isDirectoryDropTarget(entry.directory)
              ? "true"
              : undefined}
            class={`${state.isEntrySelected(entry.reference.id) ? "bg-muted ring-2 ring-ring" : "hover:bg-muted/60"} ${state.isEntryCut(entry.reference.id) ? "opacity-50" : ""} ${entry.directory && state.isDirectoryDropTarget(entry.directory) ? "outline-2 outline-offset-2 outline-ring" : ""} flex min-h-32 flex-col items-center justify-center gap-3 rounded-xl p-3 text-center`}
            oncontextmenu={() =>
              state.selectEntryForContextMenu(entry.reference.id)}
            onclick={(event) =>
              state.selectEntry(entry.reference.id, {
                toggle: event.metaKey || event.ctrlKey,
                range: event.shiftKey,
              })}
            ondblclick={() => void state.openEntry(entry.reference.id)}
            ondragstart={(event) =>
              state.startEntryDrag(entry.reference.id, event)}
            ondragover={(event) =>
              entry.directory &&
              state.dragOverDirectory(entry.directory, event)}
            ondragleave={() =>
              entry.directory && state.leaveDropDirectory(entry.directory)}
            ondrop={(event) =>
              entry.directory &&
              void state.dropDraggedEntries(entry.directory, event)}
            ondragend={() => state.endEntryDrag()}
            onkeydown={(event) => {
              if (isRenameShortcut(event)) {
                event.preventDefault();
                state.startRename(entry.reference.id);
              } else if (event.key === "Enter") {
                void state.openEntry(entry.reference.id);
              }
            }}
          >
            <FileGlyph kind={entry.contentKind} />
            <span class="line-clamp-2 max-w-full text-sm font-medium"
              >{entry.name}</span
            >
          </button>
        {/if}
      {/each}
    </div>
  </ContextMenu.Trigger>
  <ContextMenu.Content>
    <ContextMenu.Item
      disabled={state.selectedEntries.length !== 1}
      onclick={() => {
        if (state.selectedEntry)
          void state.openEntry(state.selectedEntry.reference.id);
      }}>Open</ContextMenu.Item
    >
    <ContextMenu.Item
      disabled={state.selectedEntries.length !== 1}
      onclick={() => void state.openPreview()}>Quick Preview</ContextMenu.Item
    >
    <ContextMenu.Separator />
    <ContextMenu.Item
      disabled={!state.canRenameSelection ||
        state.fileOperations.activeEntryId !== null}
      onclick={() => state.startRename()}>Rename</ContextMenu.Item
    >
    <ContextMenu.Item
      disabled={!state.canCutSelection ||
        state.fileOperations.activeEntryId !== null}
      onclick={() => state.cutSelected()}>Cut</ContextMenu.Item
    >
    <ContextMenu.Item
      disabled={!state.canMoveSelection ||
        state.fileOperations.activeEntryId !== null}
      onclick={() => void state.openMoveSelected()}>Move…</ContextMenu.Item
    >
    <ContextMenu.Item
      disabled={!state.canPasteCutEntries ||
        state.fileOperations.activeEntryId !== null}
      onclick={() => void state.pasteCutEntries()}
      >Paste into This Folder</ContextMenu.Item
    >
    <ContextMenu.Item
      disabled={!state.canTrashSelection ||
        state.fileOperations.activeEntryId !== null}
      onclick={() => void state.moveSelectedToTrash()}
      >Move to Trash</ContextMenu.Item
    >
    <ContextMenu.Separator />
    <ContextMenu.Item
      class="text-destructive focus:text-destructive"
      disabled={!state.canDeleteSelectionPermanently ||
        state.fileOperations.activeEntryId !== null}
      onclick={() => void state.deleteSelectedPermanently()}
      >Delete Permanently</ContextMenu.Item
    >
  </ContextMenu.Content>
</ContextMenu.Root>
