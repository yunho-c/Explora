<script lang="ts">
  import CopyIcon from "@lucide/svelte/icons/copy";
  import AlertTriangleIcon from "@lucide/svelte/icons/triangle-alert";

  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import { Button } from "$lib/components/ui/button";
  import * as Dialog from "$lib/components/ui/dialog";

  let { state }: { state: ExplorerState } = $props();
</script>

<Dialog.Root
  open={state.fileOperations.pendingPrompt !== null}
  onOpenChange={(open) => {
    if (!open && state.fileOperations.pendingPrompt) {
      state.fileOperations.cancelPrompt();
    }
  }}
>
  <Dialog.Content showCloseButton={false} class="sm:max-w-md">
    {@const pending = state.fileOperations.pendingPrompt}
    {#if pending}
      <Dialog.Header>
        <div
          class={pending.prompt.kind === "permanentDelete"
            ? "mb-1 grid size-10 place-items-center rounded-full bg-destructive/10 text-destructive"
            : "mb-1 grid size-10 place-items-center rounded-full bg-muted text-foreground"}
          aria-hidden="true"
        >
          {#if pending.prompt.kind === "permanentDelete"}
            <AlertTriangleIcon class="size-5" />
          {:else}
            <CopyIcon class="size-5" />
          {/if}
        </div>
        <Dialog.Title>{pending.prompt.title}</Dialog.Title>
        <Dialog.Description>{pending.prompt.message}</Dialog.Description>
      </Dialog.Header>

      <div class="rounded-lg border bg-muted/35 px-3 py-2.5">
        <p class="truncate font-medium">{pending.prompt.targetName}</p>
        <p class="mt-0.5 text-xs text-muted-foreground">
          {pending.prompt.kind === "permanentDelete"
            ? `In ${pending.prompt.locationName}`
            : `Destination: ${pending.prompt.destinationName}`}
        </p>
      </div>

      <Dialog.Footer>
        <Button
          variant="outline"
          autofocus
          disabled={pending.responding}
          onclick={() => state.fileOperations.cancelPrompt()}
        >
          Cancel
        </Button>
        {#if pending.prompt.kind === "permanentDelete"}
          <Button
            variant="destructive"
            disabled={pending.responding}
            onclick={() => void state.fileOperations.answerPrompt("confirm")}
          >
            {pending.responding ? "Deleting…" : pending.prompt.confirmLabel}
          </Button>
        {:else}
          {#if pending.prompt.decisions.includes("skip")}
            <Button
              variant="secondary"
              disabled={pending.responding}
              onclick={() => void state.fileOperations.answerPrompt("skip")}
            >
              Skip
            </Button>
          {/if}
          {#if pending.prompt.decisions.includes("keepBoth")}
            <Button
              disabled={pending.responding}
              onclick={() => void state.fileOperations.answerPrompt("keepBoth")}
            >
              {pending.responding ? "Moving…" : "Keep Both"}
            </Button>
          {/if}
        {/if}
      </Dialog.Footer>
    {/if}
  </Dialog.Content>
</Dialog.Root>
