<script lang="ts">
  import AlertTriangleIcon from "@lucide/svelte/icons/triangle-alert";

  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import { Button } from "$lib/components/ui/button";
  import * as Dialog from "$lib/components/ui/dialog";

  let { state }: { state: ExplorerState } = $props();
</script>

<Dialog.Root
  open={state.fileOperations.pendingConfirmation !== null}
  onOpenChange={(open) => {
    if (!open && state.fileOperations.pendingConfirmation) {
      state.fileOperations.cancelConfirmation();
    }
  }}
>
  <Dialog.Content showCloseButton={false} class="sm:max-w-md">
    {@const pending = state.fileOperations.pendingConfirmation}
    {#if pending}
      <Dialog.Header>
        <div
          class="mb-1 grid size-10 place-items-center rounded-full bg-destructive/10 text-destructive"
          aria-hidden="true"
        >
          <AlertTriangleIcon class="size-5" />
        </div>
        <Dialog.Title>{pending.confirmation.title}</Dialog.Title>
        <Dialog.Description>
          {pending.confirmation.message}
        </Dialog.Description>
      </Dialog.Header>

      <div class="rounded-lg border bg-muted/35 px-3 py-2.5">
        <p class="truncate font-medium">{pending.confirmation.targetName}</p>
        <p class="mt-0.5 text-xs text-muted-foreground">
          In {pending.confirmation.locationName}
        </p>
      </div>

      <Dialog.Footer>
        <Button
          variant="outline"
          autofocus
          disabled={pending.responding}
          onclick={() => state.fileOperations.cancelConfirmation()}
        >
          Cancel
        </Button>
        <Button
          variant="destructive"
          disabled={pending.responding}
          onclick={() =>
            void state.fileOperations.answerConfirmation("confirm")}
        >
          {pending.responding ? "Deleting…" : pending.confirmation.confirmLabel}
        </Button>
      </Dialog.Footer>
    {/if}
  </Dialog.Content>
</Dialog.Root>
