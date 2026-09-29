<script lang="ts">
  import { tick } from "svelte";
  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import { Button } from "$lib/components/ui/button";
  import * as Dialog from "$lib/components/ui/dialog";
  import { Input } from "$lib/components/ui/input";
  import { Label } from "$lib/components/ui/label";

  let { state: explorerState }: { state: ExplorerState } = $props();
  let nameInput = $state<HTMLInputElement | null>(null);
  let previousFocus: HTMLElement | null = null;
</script>

<Dialog.Root
  open={explorerState.newFolderParent !== null}
  onOpenChange={(open) => {
    if (!open) explorerState.cancelNewFolder();
  }}
>
  <Dialog.Content
    class="sm:max-w-md"
    showCloseButton={false}
    onOpenAutoFocus={(event) => {
      event.preventDefault();
      previousFocus =
        document.activeElement instanceof HTMLElement
          ? document.activeElement
          : null;
      nameInput?.focus();
    }}
    onCloseAutoFocus={(event) => {
      event.preventDefault();
      void tick().then(() => {
        const selected = document.querySelector<HTMLElement>(
          '[data-file-entry][aria-selected="true"]',
        );
        (
          selected ??
          (previousFocus?.isConnected
            ? previousFocus
            : document.querySelector<HTMLElement>(
                '[aria-label="More options"]',
              ))
        )?.focus();
      });
    }}
  >
    <Dialog.Header>
      <Dialog.Title>New folder</Dialog.Title>
      <Dialog.Description class="break-words"
        >Create a folder in {explorerState.newFolderParent
          ?.displayPath}.</Dialog.Description
      >
    </Dialog.Header>
    <form
      class="grid gap-4"
      onsubmit={(event) => {
        event.preventDefault();
        void explorerState.createFolder();
      }}
      aria-busy={explorerState.newFolderSaving}
    >
      <div class="grid gap-2">
        <Label for="new-folder-name">Folder name</Label>
        <Input
          id="new-folder-name"
          bind:ref={nameInput}
          bind:value={explorerState.newFolderName}
          disabled={explorerState.newFolderSaving}
          autocomplete="off"
          spellcheck={false}
          aria-invalid={Boolean(explorerState.newFolderErrorMessage)}
          aria-describedby={explorerState.newFolderErrorMessage
            ? "new-folder-error"
            : undefined}
        />
        {#if explorerState.newFolderErrorMessage}
          <p
            id="new-folder-error"
            role="alert"
            class="text-sm text-destructive"
          >
            {explorerState.newFolderErrorMessage}
          </p>
        {/if}
        {#if explorerState.newFolderSaving}
          <p role="status" class="text-sm text-muted-foreground">
            {explorerState.newFolderCancelling
              ? "Waiting for the folder operation to finish…"
              : "Creating folder…"}
          </p>
        {/if}
      </div>
      <Dialog.Footer>
        <Button
          type="button"
          variant="outline"
          disabled={explorerState.newFolderCancelling}
          onclick={() => explorerState.cancelNewFolder()}>Cancel</Button
        >
        <Button type="submit" disabled={explorerState.newFolderSaving}
          >Create</Button
        >
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
