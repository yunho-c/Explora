<script lang="ts">
  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import * as Dialog from "$lib/components/ui/dialog";
  import { Separator } from "$lib/components/ui/separator";
  import { Skeleton } from "$lib/components/ui/skeleton";

  import FileGlyph from "./FileGlyph.svelte";

  let { state }: { state: ExplorerState } = $props();
</script>

<Dialog.Root
  bind:open={state.previewOpen}
  onOpenChange={(open) => {
    if (!open) state.closePreview();
  }}
>
  <Dialog.Content>
    {#if state.previewLoading}
      <Dialog.Header>
        <Dialog.Title>Preparing preview</Dialog.Title>
        <Dialog.Description>Reading file metadata…</Dialog.Description>
      </Dialog.Header>
      <div class="space-y-3">
        <Skeleton class="h-40 w-full" />
        <Skeleton class="h-4 w-2/3" />
        <Skeleton class="h-4 w-1/2" />
      </div>
    {:else if state.preview}
      <Dialog.Header>
        <Dialog.Title>{state.preview.title}</Dialog.Title>
        <Dialog.Description>{state.preview.subtitle}</Dialog.Description>
      </Dialog.Header>

      <div
        class="flex min-h-48 flex-col items-center justify-center gap-5 rounded-xl bg-muted/50 p-6 text-center"
      >
        <FileGlyph kind={state.preview.kind} size="lg" />
        {#if state.preview.excerpt}
          <pre
            class="max-h-28 w-full overflow-auto font-sans text-sm whitespace-pre-wrap text-muted-foreground">{state
              .preview.excerpt}</pre>
        {/if}
      </div>

      <Separator />
      <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
        {#each state.preview.details as detail (detail.label)}
          <dt class="text-muted-foreground">{detail.label}</dt>
          <dd class="truncate text-right" title={detail.value}>
            {detail.value}
          </dd>
        {/each}
      </dl>
      <p class="text-center text-xs text-muted-foreground">
        Use ↑ and ↓ to move between items · Esc to close
      </p>
    {/if}
  </Dialog.Content>
</Dialog.Root>
