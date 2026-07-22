<script lang="ts">
  import DownloadIcon from "@lucide/svelte/icons/download";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import ShieldCheckIcon from "@lucide/svelte/icons/shield-check";
  import type { Action } from "svelte/action";

  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import { Badge } from "$lib/components/ui/badge";
  import { Button } from "$lib/components/ui/button";
  import * as Dialog from "$lib/components/ui/dialog";
  import { Progress } from "$lib/components/ui/progress";
  import { Skeleton } from "$lib/components/ui/skeleton";
  import { Toggle } from "$lib/components/ui/toggle";

  import FileGlyph from "./FileGlyph.svelte";
  import PdfPreview from "./PdfPreview.svelte";

  let { state }: { state: ExplorerState } = $props();

  const contentRequestStatus = (availability: string) => {
    switch (availability) {
      case "downloading":
        return "The operating system is downloading this file…";
      case "syncing":
        return "Waiting for the current version…";
      case "partial":
        return "Waiting for the remaining file content…";
      case "error":
        return "The operating system reported a download error.";
      default:
        return "Waiting for the file to become available locally…";
    }
  };

  const guardImageRender: Action<
    HTMLImageElement,
    { entryId: string; direct: boolean }
  > = (node, initialOptions) => {
    let options = initialOptions;
    let timeout: number | undefined;

    const clearTimeout = () => {
      if (timeout !== undefined) window.clearTimeout(timeout);
      timeout = undefined;
    };
    const fail = () => {
      clearTimeout();
      state.handlePreviewImageFailure(options.entryId);
    };
    const scheduleTimeout = () => {
      clearTimeout();
      if (options.direct) timeout = window.setTimeout(fail, 5_000);
    };

    node.addEventListener("load", clearTimeout);
    node.addEventListener("error", fail);
    scheduleTimeout();

    return {
      update(nextOptions) {
        options = nextOptions;
        scheduleTimeout();
      },
      destroy() {
        clearTimeout();
        node.removeEventListener("load", clearTimeout);
        node.removeEventListener("error", fail);
      },
    };
  };
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
    {#if state.selectedEntry?.contentKind === "image" && state.activeLocation?.backend !== "ssh"}
      <Toggle
        variant="default"
        size="sm"
        pressed={state.imagePreviewMode === "sanitized"}
        onPressedChange={(pressed) =>
          void state.setImagePreviewMode(pressed ? "sanitized" : "direct")}
        aria-label={state.imagePreviewMode === "sanitized"
          ? "Use direct image preview"
          : "Use sanitized image preview"}
        title={state.imagePreviewMode === "sanitized"
          ? "Use direct image preview"
          : "Use sanitized image preview"}
        class="absolute top-4 right-14 z-10 size-8 p-0"
      >
        <ShieldCheckIcon />
      </Toggle>
    {/if}

    {#if state.previewLoading}
      <Dialog.Header class="sr-only">
        <Dialog.Title>{state.selectedEntry?.name ?? "Preview"}</Dialog.Title>
        <Dialog.Description>
          {state.selectedEntry?.displayPath ??
            state.selectedEntry?.name ??
            "Preview"}
        </Dialog.Description>
      </Dialog.Header>
      <div class="border-b px-6 py-4 pr-24" aria-busy="true">
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
      {#if preview.content.type === "pdf"}
        <Dialog.Header class="sr-only">
          <Dialog.Title>{preview.title}</Dialog.Title>
          <Dialog.Description>
            {preview.accessibilityDescription}
          </Dialog.Description>
        </Dialog.Header>
      {:else}
        <header class="border-b px-6 py-4 pr-24">
          <Dialog.Header class="gap-0">
            <Dialog.Title class="truncate" title={preview.title}>
              {preview.title}
            </Dialog.Title>
            <Dialog.Description class="sr-only">
              {preview.accessibilityDescription}
            </Dialog.Description>
          </Dialog.Header>
        </header>
      {/if}

      <div
        class="min-h-0 flex-1 bg-muted/30 {preview.content.type === 'pdf'
          ? ''
          : 'p-4 sm:p-6'}"
      >
        {#if preview.content.type === "image"}
          <div
            class="flex size-full items-center justify-center overflow-hidden"
          >
            <img
              src={preview.content.url}
              alt={`Preview of ${preview.title}`}
              width={preview.content.width}
              height={preview.content.height}
              use:guardImageRender={{
                entryId: preview.entryId,
                direct: preview.content.imageMode === "direct",
              }}
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
        {:else if preview.content.type === "pdf"}
          {#key preview.entryId}
            <PdfPreview data={preview.content.data} title={preview.title} />
          {/key}
        {:else}
          <div
            class="flex size-full flex-col items-center justify-center gap-4 rounded-lg border border-dashed bg-background/70 p-8 text-center"
          >
            <FileGlyph kind={preview.kind} size="lg" />
            <div class="max-w-md space-y-1">
              <p class="font-medium">Preview unavailable</p>
              <p
                class="text-sm text-muted-foreground"
                role="status"
                aria-live="polite"
              >
                {preview.content.message}
              </p>
            </div>
            {#if preview.content.reason === "downloadRequired" && state.previewContentRequest}
              <div class="w-full max-w-sm space-y-3" aria-live="polite">
                <div
                  class="flex items-center justify-center gap-2 text-sm font-medium"
                >
                  <LoaderCircleIcon class="size-4 animate-spin" />
                  <span>
                    {contentRequestStatus(
                      state.previewContentRequest.availability,
                    )}
                  </span>
                </div>
                <Progress
                  aria-label="Downloading file for preview"
                  class="[&_[data-slot=progress-indicator]]:!translate-x-0 [&_[data-slot=progress-indicator]]:animate-pulse"
                />
                <Button
                  variant="outline"
                  size="sm"
                  onclick={() => state.stopWaitingForPreviewContent()}
                >
                  Stop waiting
                </Button>
                {#if !state.previewContentRequest.providerWorkCancellable}
                  <p class="text-xs text-muted-foreground">
                    Stopping here will not stop the operating system download.
                  </p>
                {/if}
              </div>
            {:else if preview.content.reason === "downloadRequired" && preview.content.requestContent}
              <Button
                size="sm"
                onclick={() => void state.requestPreviewContent()}
              >
                <DownloadIcon />
                Download to Preview
              </Button>
            {/if}
            {#if state.previewContentRequestMessage}
              <p
                class="max-w-md text-sm text-destructive"
                role="status"
                aria-live="polite"
              >
                {state.previewContentRequestMessage}
              </p>
            {/if}
          </div>
        {/if}
      </div>

      {#if preview.content.type !== "pdf"}
        <footer class="shrink-0 border-t bg-background px-6 py-3">
          <dl
            class="grid gap-x-6 gap-y-1 text-xs sm:grid-cols-2 lg:grid-cols-3"
          >
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
    {/if}
  </Dialog.Content>
</Dialog.Root>
