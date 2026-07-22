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
            aria-selected={state.selectedEntryId === entry.reference.id}
            class={state.selectedEntryId === entry.reference.id
              ? "flex min-h-32 flex-col items-center justify-center gap-3 rounded-xl bg-muted p-3 text-center ring-2 ring-ring"
              : "flex min-h-32 flex-col items-center justify-center gap-3 rounded-xl p-3 text-center hover:bg-muted/60"}
            oncontextmenu={() => state.selectEntry(entry.reference.id)}
            onclick={() => state.selectEntry(entry.reference.id)}
            ondblclick={() => void state.openEntry(entry.reference.id)}
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
      disabled={!state.selectedEntry}
      onclick={() => void state.openPreview()}>Quick Preview</ContextMenu.Item
    >
    <ContextMenu.Separator />
    <ContextMenu.Item
      disabled={!state.selectedEntry?.capabilities.rename ||
        state.fileOperations.activeEntryId !== null}
      onclick={() => state.startRename()}>Rename</ContextMenu.Item
    >
    <ContextMenu.Item
      disabled={!state.selectedEntry?.capabilities.move ||
        state.fileOperations.activeEntryId !== null}
      onclick={() => void state.openMoveSelected()}>Move…</ContextMenu.Item
    >
    <ContextMenu.Item
      disabled={!state.selectedEntry?.capabilities.trash ||
        state.fileOperations.activeEntryId !== null}
      onclick={() => void state.moveSelectedToTrash()}
      >Move to Trash</ContextMenu.Item
    >
    <ContextMenu.Separator />
    <ContextMenu.Item
      class="text-destructive focus:text-destructive"
      disabled={!state.selectedEntry?.capabilities.deletePermanently ||
        state.fileOperations.activeEntryId !== null}
      onclick={() => void state.deleteSelectedPermanently()}
      >Delete Permanently</ContextMenu.Item
    >
  </ContextMenu.Content>
</ContextMenu.Root>
