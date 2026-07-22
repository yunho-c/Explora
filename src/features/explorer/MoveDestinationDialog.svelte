<script lang="ts">
  import ArrowUpIcon from "@lucide/svelte/icons/arrow-up";
  import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";
  import FolderIcon from "@lucide/svelte/icons/folder";
  import HardDriveIcon from "@lucide/svelte/icons/hard-drive";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import ServerIcon from "@lucide/svelte/icons/server";

  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import { Button } from "$lib/components/ui/button";
  import * as Dialog from "$lib/components/ui/dialog";
  import { ScrollArea } from "$lib/components/ui/scroll-area";

  let { state }: { state: ExplorerState } = $props();
  // svelte-ignore non_reactive_update
  let cancelButton: HTMLButtonElement | null = null;
</script>

<Dialog.Root
  open={state.fileOperations.moveChooser !== null}
  onOpenChange={(open) => {
    if (!open && state.fileOperations.moveChooser) {
      state.fileOperations.closeMoveChooser();
    }
  }}
>
  <Dialog.Content
    showCloseButton={false}
    class="gap-4 p-0 sm:max-w-2xl"
    onOpenAutoFocus={(event) => {
      event.preventDefault();
      cancelButton?.focus();
    }}
  >
    {@const chooser = state.fileOperations.moveChooser}
    {#if chooser}
      <Dialog.Header class="border-b px-5 pt-5 pb-4">
        <Dialog.Title class="truncate"
          >{chooser.entries.length === 1
            ? `Move “${chooser.entries[0].name}”`
            : `Move ${chooser.entries.length} items`}</Dialog.Title
        >
        <Dialog.Description>
          Choose a folder in {chooser.entries[0].reference.locationId ===
          chooser.directory.locationId
            ? "this location"
            : "a compatible location"}. Existing items are never replaced
          silently.
        </Dialog.Description>
      </Dialog.Header>

      <div class="grid min-h-80 grid-cols-[11rem_minmax(0,1fr)]">
        <nav class="border-r bg-muted/25 p-2" aria-label="Move locations">
          <p
            class="px-2 pt-1 pb-2 text-[0.6875rem] font-medium tracking-wide text-muted-foreground uppercase"
          >
            Locations
          </p>
          <div class="space-y-0.5">
            {#each chooser.locations as location (location.id)}
              {@const compatible =
                location.status !== "offline" &&
                location.root.capabilities.acceptMove}
              <button
                type="button"
                disabled={!compatible}
                aria-current={chooser.directory.locationId === location.id
                  ? "location"
                  : undefined}
                title={compatible
                  ? `Browse ${location.name}`
                  : location.status === "offline"
                    ? `${location.name} is offline`
                    : "This location cannot accept moved items"}
                class="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-45 aria-[current=location]:bg-muted aria-[current=location]:font-medium"
                onclick={() =>
                  void state.fileOperations.browseMoveDestination(
                    location.root,
                  )}
              >
                {#if location.kind === "ssh"}
                  <ServerIcon class="size-4 shrink-0" />
                {:else}
                  <HardDriveIcon class="size-4 shrink-0" />
                {/if}
                <span class="truncate">{location.name}</span>
              </button>
            {/each}
          </div>
        </nav>

        <section class="flex min-w-0 flex-col" aria-label="Destination folder">
          <div class="flex h-11 items-center gap-1 border-b px-3">
            <Button
              variant="ghost"
              size="icon-sm"
              disabled={!chooser.parent || chooser.loading}
              aria-label="Go to parent folder"
              onclick={() =>
                chooser.parent &&
                void state.fileOperations.browseMoveDestination(chooser.parent)}
            >
              <ArrowUpIcon />
            </Button>
            <div
              class="flex min-w-0 items-center overflow-hidden text-xs text-muted-foreground"
              aria-label={`Current destination: ${chooser.directory.displayPath}`}
            >
              {#each chooser.breadcrumbs as breadcrumb, index (breadcrumb.directory.id)}
                {#if index > 0}<ChevronRightIcon
                    class="mx-0.5 size-3 shrink-0 opacity-60"
                  />{/if}
                <button
                  type="button"
                  class="max-w-28 truncate rounded px-1.5 py-1 hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                  onclick={() =>
                    void state.fileOperations.browseMoveDestination(
                      breadcrumb.directory,
                    )}
                >
                  {breadcrumb.label}
                </button>
              {/each}
            </div>
          </div>

          <ScrollArea class="h-64">
            {#if chooser.loading}
              <div
                class="flex h-64 items-center justify-center gap-2 text-sm text-muted-foreground"
                role="status"
              >
                <LoaderCircleIcon class="size-4 animate-spin" />
                Loading folders…
              </div>
            {:else if chooser.errorMessage}
              <div class="p-5 text-sm text-destructive" role="alert">
                {chooser.errorMessage}
              </div>
            {:else if chooser.directories.length === 0}
              <div
                class="grid h-64 place-items-center text-sm text-muted-foreground"
              >
                No folders here
              </div>
            {:else}
              <div class="p-2" aria-label="Folders">
                {#each chooser.directories as entry (entry.reference.id)}
                  {@const compatible =
                    entry.directory !== null &&
                    state.fileOperations.isCompatibleDestination(
                      entry.directory,
                    )}
                  <button
                    type="button"
                    disabled={!compatible}
                    title={compatible
                      ? `Open ${entry.name}`
                      : chooser.entries.some(
                            (source) =>
                              source.directory?.locationId ===
                                entry.directory?.locationId &&
                              source.directory?.id === entry.directory?.id,
                          )
                        ? "A folder cannot be moved into itself"
                        : "This folder cannot accept moved items"}
                    class="flex w-full items-center gap-3 rounded-md px-2.5 py-2 text-left outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-45"
                    onclick={() =>
                      entry.directory &&
                      void state.fileOperations.browseMoveDestination(
                        entry.directory,
                      )}
                  >
                    <FolderIcon class="size-5 shrink-0 text-muted-foreground" />
                    <span class="min-w-0 flex-1 truncate font-medium"
                      >{entry.name}</span
                    >
                    <ChevronRightIcon class="size-4 text-muted-foreground" />
                  </button>
                {/each}
              </div>
            {/if}
          </ScrollArea>
        </section>
      </div>

      <Dialog.Footer
        class="items-center border-t bg-muted/15 px-5 py-4 sm:justify-between"
      >
        <p
          class="min-w-0 flex-1 truncate text-left text-xs text-muted-foreground"
        >
          Destination: {chooser.directory.displayPath}
        </p>
        <div class="flex gap-2">
          <Button
            variant="outline"
            bind:ref={cancelButton}
            onclick={() => state.fileOperations.closeMoveChooser()}
          >
            Cancel
          </Button>
          <Button
            disabled={!state.fileOperations.canConfirmMove}
            onclick={() => void state.confirmMoveSelected()}
          >
            Move Here
          </Button>
        </div>
      </Dialog.Footer>
    {/if}
  </Dialog.Content>
</Dialog.Root>
