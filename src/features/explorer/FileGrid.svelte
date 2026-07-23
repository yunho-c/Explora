<script lang="ts">
  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import * as ContextMenu from "$lib/components/ui/context-menu";

  import FileGlyph from "./FileGlyph.svelte";

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
            if (event.key === "Enter") void state.openEntry(entry.reference.id);
          }}
        >
          <FileGlyph kind={entry.contentKind} />
          <span class="line-clamp-2 max-w-full text-sm font-medium"
            >{entry.name}</span
          >
        </button>
      {/each}
    </div>
  </ContextMenu.Trigger>
  <ContextMenu.Content>
    <ContextMenu.Item
      disabled={!state.selectedEntry}
      onclick={() => {
        if (state.selectedEntry)
          void state.openEntry(state.selectedEntry.reference.id);
      }}>Open</ContextMenu.Item
    >
    <ContextMenu.Item
      disabled={!state.selectedEntry}
      onclick={() => void state.openPreview()}>Quick Preview</ContextMenu.Item
    >
    <ContextMenu.Separator />
    <ContextMenu.Item disabled>Rename</ContextMenu.Item>
    <ContextMenu.Item disabled>Move to Trash</ContextMenu.Item>
  </ContextMenu.Content>
</ContextMenu.Root>
