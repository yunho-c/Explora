<script lang="ts">
  import ArchiveIcon from "@lucide/svelte/icons/archive";
  import FileIcon from "@lucide/svelte/icons/file";
  import FileCodeIcon from "@lucide/svelte/icons/file-code-2";
  import FileTextIcon from "@lucide/svelte/icons/file-text";
  import FilmIcon from "@lucide/svelte/icons/film";
  import FolderIcon from "@lucide/svelte/icons/folder";
  import ImageIcon from "@lucide/svelte/icons/image";
  import MusicIcon from "@lucide/svelte/icons/music-2";

  import type { ContentKind } from "$lib/contracts/explorer";

  let { kind, size = "md" }: { kind: ContentKind; size?: "sm" | "md" | "lg" } =
    $props();
</script>

<span
  class={size === "lg"
    ? "grid size-28 place-items-center rounded-2xl bg-muted"
    : size === "md"
      ? "grid size-10 place-items-center rounded-lg bg-muted"
      : "grid size-7 place-items-center rounded-md bg-muted"}
  aria-hidden="true"
>
  {#if kind === "folder"}
    <FolderIcon
      class={size === "lg" ? "size-14" : size === "md" ? "size-5" : "size-4"}
    />
  {:else if kind === "image"}
    <ImageIcon
      class={size === "lg" ? "size-14" : size === "md" ? "size-5" : "size-4"}
    />
  {:else if kind === "code"}
    <FileCodeIcon
      class={size === "lg" ? "size-14" : size === "md" ? "size-5" : "size-4"}
    />
  {:else if kind === "audio"}
    <MusicIcon
      class={size === "lg" ? "size-14" : size === "md" ? "size-5" : "size-4"}
    />
  {:else if kind === "video"}
    <FilmIcon
      class={size === "lg" ? "size-14" : size === "md" ? "size-5" : "size-4"}
    />
  {:else if kind === "archive"}
    <ArchiveIcon
      class={size === "lg" ? "size-14" : size === "md" ? "size-5" : "size-4"}
    />
  {:else if kind === "document"}
    <FileTextIcon
      class={size === "lg" ? "size-14" : size === "md" ? "size-5" : "size-4"}
    />
  {:else}
    <FileIcon
      class={size === "lg" ? "size-14" : size === "md" ? "size-5" : "size-4"}
    />
  {/if}
</span>
