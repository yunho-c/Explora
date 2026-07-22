<script lang="ts">
  import ArrowDownIcon from "@lucide/svelte/icons/arrow-down";
  import ArrowUpIcon from "@lucide/svelte/icons/arrow-up";

  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import type { SortColumn } from "$lib/contracts/explorer";
  import * as ContextMenu from "$lib/components/ui/context-menu";
  import { formatFileSize } from "$lib/file-metadata";
  import { isRenameShortcut } from "$lib/platform-shortcuts";
  import * as Table from "$lib/components/ui/table";

  import FileGlyph from "./FileGlyph.svelte";
  import RenameInput from "./RenameInput.svelte";

  let { state }: { state: ExplorerState } = $props();

  const formatDate = (value: number | null) =>
    value === null
      ? "—"
      : new Intl.DateTimeFormat(undefined, {
          month: "short",
          day: "numeric",
          hour: "numeric",
          minute: "2-digit",
        }).format(new Date(value));

  const sortLabel = (column: SortColumn) => {
    if (state.sort.column !== column) return "";
    return state.sort.direction === "ascending" ? "ascending" : "descending";
  };
</script>

<ContextMenu.Root>
  <ContextMenu.Trigger>
    <Table.Root aria-multiselectable="true">
      <Table.Header>
        <Table.Row>
          <Table.Head>
            <button
              class="flex items-center gap-1"
              type="button"
              onclick={() => state.toggleSort("name")}
            >
              Name
              {#if state.sort.column === "name"}
                {#if state.sort.direction === "ascending"}<ArrowUpIcon
                    class="size-3"
                  />{:else}<ArrowDownIcon class="size-3" />{/if}
              {/if}
            </button>
          </Table.Head>
          <Table.Head class="hidden sm:table-cell">
            <button
              class="flex items-center gap-1"
              type="button"
              onclick={() => state.toggleSort("modifiedAt")}
            >
              Modified
              {#if state.sort.column === "modifiedAt"}
                {#if state.sort.direction === "ascending"}<ArrowUpIcon
                    class="size-3"
                  />{:else}<ArrowDownIcon class="size-3" />{/if}
              {/if}
            </button>
          </Table.Head>
          <Table.Head class="w-28 text-right">
            <button
              class="ml-auto flex items-center gap-1"
              type="button"
              onclick={() => state.toggleSort("size")}
            >
              Size
              {#if state.sort.column === "size"}
                {#if state.sort.direction === "ascending"}<ArrowUpIcon
                    class="size-3"
                  />{:else}<ArrowDownIcon class="size-3" />{/if}
              {/if}
            </button>
          </Table.Head>
        </Table.Row>
      </Table.Header>
      <Table.Body>
        {#each state.visibleEntries as entry (entry.reference.id)}
          <Table.Row
            data-state={state.isEntrySelected(entry.reference.id)
              ? "selected"
              : undefined}
            aria-selected={state.isEntrySelected(entry.reference.id)}
            draggable={state.renamingEntryId !== entry.reference.id}
            data-cut={state.isEntryCut(entry.reference.id) || undefined}
            data-drop-target={entry.directory &&
            state.isDirectoryDropTarget(entry.directory)
              ? "true"
              : undefined}
            class={`${state.isEntryCut(entry.reference.id) ? "opacity-50" : ""} ${entry.directory && state.isDirectoryDropTarget(entry.directory) ? "outline-2 outline-offset-[-2px] outline-ring" : ""}`}
            tabindex={0}
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
                event.preventDefault();
                void state.openEntry(entry.reference.id);
              }
            }}
          >
            <Table.Cell>
              <div class="flex min-w-0 items-center gap-3">
                <FileGlyph kind={entry.contentKind} size="sm" />
                <div class="min-w-0">
                  {#if state.renamingEntryId === entry.reference.id}
                    <RenameInput {state} {entry} />
                  {:else}
                    <p class="truncate font-medium">{entry.name}</p>
                  {/if}
                  {#if entry.detail && state.renamingEntryId !== entry.reference.id}<p
                      class="truncate text-xs text-muted-foreground sm:hidden"
                    >
                      {entry.detail}
                    </p>{/if}
                </div>
              </div>
            </Table.Cell>
            <Table.Cell class="hidden text-muted-foreground sm:table-cell"
              >{formatDate(entry.modifiedAt)}</Table.Cell
            >
            <Table.Cell class="text-right text-muted-foreground"
              >{formatFileSize(entry.size)}</Table.Cell
            >
          </Table.Row>
        {/each}
      </Table.Body>
    </Table.Root>
  </ContextMenu.Trigger>
  <ContextMenu.Content>
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

<span class="sr-only" aria-live="polite">
  Sorted by {state.sort.column}, {sortLabel(state.sort.column)}.
</span>
