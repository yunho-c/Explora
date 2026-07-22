<script lang="ts">
  import ArrowDownIcon from "@lucide/svelte/icons/arrow-down";
  import ArrowUpIcon from "@lucide/svelte/icons/arrow-up";

  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import type { SortColumn } from "$lib/contracts/explorer";
  import * as ContextMenu from "$lib/components/ui/context-menu";
  import { formatFileSize } from "$lib/file-metadata";
  import * as Table from "$lib/components/ui/table";

  import FileGlyph from "./FileGlyph.svelte";

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
    <Table.Root>
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
            data-state={state.selectedEntryId === entry.reference.id
              ? "selected"
              : undefined}
            aria-selected={state.selectedEntryId === entry.reference.id}
            tabindex={0}
            oncontextmenu={() => state.selectEntry(entry.reference.id)}
            onclick={() => state.selectEntry(entry.reference.id)}
            ondblclick={() => void state.openEntry(entry.reference.id)}
            onkeydown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                void state.openEntry(entry.reference.id);
              }
            }}
          >
            <Table.Cell>
              <div class="flex min-w-0 items-center gap-3">
                <FileGlyph kind={entry.contentKind} size="sm" />
                <div class="min-w-0">
                  <p class="truncate font-medium">{entry.name}</p>
                  {#if entry.detail}<p
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

<span class="sr-only" aria-live="polite">
  Sorted by {state.sort.column}, {sortLabel(state.sort.column)}.
</span>
