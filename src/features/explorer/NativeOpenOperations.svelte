<script lang="ts">
  import DownloadIcon from "@lucide/svelte/icons/download";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import XIcon from "@lucide/svelte/icons/x";

  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import { Button } from "$lib/components/ui/button";
  import { Progress } from "$lib/components/ui/progress";
  import { formatFileSize } from "$lib/file-metadata";

  let { state }: { state: ExplorerState } = $props();

  const progressValue = (transferred: string, total: string | null) => {
    if (total === null || total === "0") return null;
    const ratio = Number(transferred) / Number(total);
    return Number.isFinite(ratio)
      ? Math.min(Math.max(ratio * 100, 0), 100)
      : null;
  };
</script>

{#if state.nativeOpenOperations.length > 0}
  <section
    class="fixed right-4 bottom-11 z-40 w-[min(22rem,calc(100vw-2rem))] overflow-hidden rounded-xl border bg-background shadow-lg"
    aria-label="Files opening in native applications"
    aria-live="polite"
  >
    <header class="border-b px-3 py-2">
      <p class="text-xs font-medium text-muted-foreground">
        Opening {state.nativeOpenOperations.length === 1
          ? "file"
          : `${state.nativeOpenOperations.length} files`}
      </p>
    </header>
    <div class="divide-y">
      {#each state.nativeOpenOperations as operation (operation.id)}
        {@const percentage = progressValue(
          operation.transferredBytes,
          operation.totalBytes,
        )}
        <div class="grid grid-cols-[auto_minmax(0,1fr)_auto] gap-3 p-3">
          <div class="grid size-8 place-items-center rounded-lg bg-muted">
            {#if operation.phase === "downloading"}
              <DownloadIcon class="size-4" aria-hidden="true" />
            {:else}
              <LoaderCircleIcon
                class="size-4 animate-spin motion-reduce:animate-none"
                aria-hidden="true"
              />
            {/if}
          </div>
          <div class="min-w-0">
            <p class="truncate text-sm font-medium">{operation.title}</p>
            <p class="truncate text-xs text-muted-foreground">
              {#if operation.phase === "queued"}
                Waiting to download from {operation.locationName}
              {:else if operation.phase === "launching"}
                Opening with the default application
              {:else if operation.totalBytes}
                {formatFileSize(operation.transferredBytes)} of
                {formatFileSize(operation.totalBytes)}
              {:else}
                Downloading from {operation.locationName}
              {/if}
            </p>
            {#if operation.phase === "downloading"}
              {#if percentage === null}
                <div
                  class="mt-2 h-1 overflow-hidden rounded-full bg-muted"
                  role="progressbar"
                  aria-label={`Downloading ${operation.title}`}
                  aria-valuetext="Total size unknown"
                >
                  <div
                    class="h-full w-1/3 animate-pulse rounded-full bg-primary motion-reduce:animate-none"
                  ></div>
                </div>
              {:else}
                <Progress
                  class="mt-2 h-1"
                  value={percentage}
                  aria-label={`Downloading ${operation.title}`}
                />
              {/if}
            {/if}
          </div>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={`Cancel opening ${operation.title}`}
            onclick={() => state.cancelNativeOpen(operation.id)}
          >
            <XIcon class="size-4" />
          </Button>
        </div>
      {/each}
    </div>
  </section>
{/if}
