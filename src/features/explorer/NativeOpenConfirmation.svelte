<script lang="ts">
  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import { Button } from "$lib/components/ui/button";
  import * as Dialog from "$lib/components/ui/dialog";
  import { formatFileSize } from "$lib/file-metadata";

  let { state }: { state: ExplorerState } = $props();
</script>

<Dialog.Root
  open={state.pendingNativeOpenConfirmation !== null}
  onOpenChange={(open) => {
    if (!open) state.dismissNativeOpenConfirmation();
  }}
>
  <Dialog.Content showCloseButton={false}>
    {@const pending = state.pendingNativeOpenConfirmation}
    {#if pending}
      <Dialog.Header>
        <Dialog.Title>Download and open this remote file?</Dialog.Title>
        <Dialog.Description>
          Explora will download a read-only snapshot from {pending.locationName}
          and open it with your default application.
        </Dialog.Description>
      </Dialog.Header>
      <div class="rounded-lg border bg-muted/40 p-3 text-sm">
        <p class="truncate font-medium">{pending.entry.name}</p>
        <p class="mt-1 text-xs text-muted-foreground">
          {pending.size === null
            ? "Size unknown"
            : formatFileSize(pending.size)}
          · 2 GiB maximum
        </p>
      </div>
      <p class="text-xs text-muted-foreground">
        Changes made in the native application are not uploaded to the SSH host.
      </p>
      <Dialog.Footer>
        <Button
          variant="outline"
          onclick={() => state.dismissNativeOpenConfirmation()}
        >
          Cancel
        </Button>
        <Button onclick={() => void state.confirmNativeOpen()}>
          Download and open
        </Button>
      </Dialog.Footer>
    {/if}
  </Dialog.Content>
</Dialog.Root>
