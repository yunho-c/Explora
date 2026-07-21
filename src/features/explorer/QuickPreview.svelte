<script lang="ts">
  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import { Badge } from "$lib/components/ui/badge";
  import * as Dialog from "$lib/components/ui/dialog";
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
  <Dialog.Content
    class="flex h-[min(48rem,calc(100vh-3rem))] max-h-[calc(100vh-3rem)] w-[min(64rem,calc(100vw-3rem))] max-w-none flex-col gap-0 overflow-hidden p-0 sm:max-w-5xl"
  >
    {#if state.previewLoading}
      <Dialog.Header class="sr-only">
        <Dialog.Title>{state.selectedEntry?.name ?? "Preview"}</Dialog.Title>
        <Dialog.Description>
          {state.selectedEntry?.displayPath ??
            state.selectedEntry?.name ??
            "Preview"}
        </Dialog.Description>
      </Dialog.Header>
      <div class="border-b px-6 py-4 pr-14" aria-busy="true">
        <Skeleton class="h-5 w-2/5 max-w-72" />
        <Skeleton class="mt-2 h-3 w-1/4 max-w-44" />
      </div>
      <div class="flex min-h-0 flex-1 flex-col gap-4 bg-muted/30 p-6">
        <Skeleton class="min-h-0 flex-1 rounded-lg" />
      </div>
      <div class="shrink-0 border-t px-6 py-4">
        <div class="grid gap-3 sm:grid-cols-3">
          <Skeleton class="h-4" />
          <Skeleton class="h-4" />
          <Skeleton class="h-4" />
        </div>
      </div>
    {:else if state.preview}
      {@const preview = state.preview}
      <header class="border-b px-6 py-4 pr-14">
        <Dialog.Header class="gap-1">
          <Dialog.Title class="truncate" title={preview.title}>
            {preview.title}
          </Dialog.Title>
          <Dialog.Description class="truncate">
            {preview.subtitle}
          </Dialog.Description>
        </Dialog.Header>
      </header>

      <div class="min-h-0 flex-1 bg-muted/30 p-4 sm:p-6">
        {#if preview.content.type === "image"}
          <div
            class="flex size-full items-center justify-center overflow-hidden"
          >
            <img
              src={preview.content.url}
              alt={`Preview of ${preview.title}`}
              width={preview.content.width}
              height={preview.content.height}
              class="max-h-full max-w-full rounded-md object-contain shadow-sm ring-1 ring-foreground/10"
            />
          </div>
        {:else if preview.content.type === "text"}
          <div
            class="flex size-full min-h-0 flex-col overflow-hidden rounded-lg border bg-background"
          >
            <div
              class="flex h-9 shrink-0 items-center justify-between border-b bg-muted/40 px-3 text-xs text-muted-foreground"
            >
              <span>{preview.content.encoding}</span>
              {#if preview.content.truncated}
                <Badge variant="secondary">First 256 KiB</Badge>
              {/if}
            </div>
            <textarea
              readonly
              spellcheck="false"
              data-preview-text
              aria-label={`Text preview of ${preview.title}`}
              value={preview.content.text}
              class="min-h-0 flex-1 resize-none overflow-auto border-0 bg-transparent p-4 font-mono text-[13px] leading-5 whitespace-pre text-foreground outline-none selection:bg-primary/20"
            ></textarea>
          </div>
        {:else}
          <div
            class="flex size-full flex-col items-center justify-center gap-4 rounded-lg border border-dashed bg-background/70 p-8 text-center"
            role="status"
          >
            <FileGlyph kind={preview.kind} size="lg" />
            <div class="max-w-md space-y-1">
              <p class="font-medium">Preview unavailable</p>
              <p class="text-sm text-muted-foreground">
                {preview.content.message}
              </p>
            </div>
          </div>
        {/if}
      </div>

      <footer class="shrink-0 border-t bg-background px-6 py-3">
        <dl class="grid gap-x-6 gap-y-1 text-xs sm:grid-cols-2 lg:grid-cols-3">
          {#each preview.details as detail (detail.label)}
            <div class="flex min-w-0 items-baseline justify-between gap-3">
              <dt class="shrink-0 text-muted-foreground">{detail.label}</dt>
              <dd class="truncate text-right" title={detail.value}>
                {detail.value}
              </dd>
            </div>
          {/each}
        </dl>
      </footer>
    {/if}
  </Dialog.Content>
</Dialog.Root>
