<script lang="ts">
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";

  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import type { FileEntrySummary } from "$lib/contracts/explorer";
  import { Input } from "$lib/components/ui/input";

  let {
    state: explorerState,
    entry,
    compact = false,
  }: {
    state: ExplorerState;
    entry: FileEntrySummary;
    compact?: boolean;
  } = $props();

  const errorId = $derived(`rename-error-${entry.reference.id}`);
  let inputRef: HTMLInputElement | null = $state(null);

  $effect(() => {
    const node = inputRef;
    if (!node) return;
    queueMicrotask(() => {
      node.focus();
      const finalDot = entry.kind === "file" ? node.value.lastIndexOf(".") : -1;
      node.setSelectionRange(0, finalDot > 0 ? finalDot : node.value.length);
    });
  });
</script>

<div class={compact ? "w-full min-w-0" : "max-w-md min-w-36"}>
  <div class="relative">
    <Input
      bind:ref={inputRef}
      bind:value={explorerState.renameDraft}
      aria-label={`Rename ${entry.name}`}
      aria-describedby={explorerState.renameErrorMessage ? errorId : undefined}
      aria-invalid={explorerState.renameErrorMessage ? "true" : undefined}
      disabled={explorerState.renameSaving}
      class={compact
        ? "h-8 bg-background px-2 text-center text-sm shadow-sm"
        : "h-8 bg-background px-2 text-sm shadow-sm"}
      onclick={(event) => event.stopPropagation()}
      onkeydown={(event) => {
        event.stopPropagation();
        if (event.key === "Enter") {
          event.preventDefault();
          void explorerState.commitRename();
        } else if (event.key === "Escape") {
          event.preventDefault();
          explorerState.cancelRename();
        }
      }}
      onblur={() => {
        if (
          !explorerState.renameSaving &&
          explorerState.renamingEntryId === entry.reference.id
        ) {
          void explorerState.commitRename();
        }
      }}
    />
    {#if explorerState.renameSaving}
      <LoaderCircleIcon
        class="absolute top-2 right-2 size-4 animate-spin text-muted-foreground"
        aria-label="Renaming"
      />
    {/if}
  </div>
  {#if explorerState.renameErrorMessage}
    <p
      id={errorId}
      class={compact
        ? "mt-1 text-center text-xs text-destructive"
        : "mt-1 text-xs text-destructive"}
      role="alert"
    >
      {explorerState.renameErrorMessage}
    </p>
  {/if}
</div>
