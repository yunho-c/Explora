<script lang="ts">
  import ArrowLeftIcon from "@lucide/svelte/icons/arrow-left";
  import ArrowRightIcon from "@lucide/svelte/icons/arrow-right";
  import ArrowUpIcon from "@lucide/svelte/icons/arrow-up";
  import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";
  import GridIcon from "@lucide/svelte/icons/grid-2x2";
  import ListIcon from "@lucide/svelte/icons/list";
  import MenuIcon from "@lucide/svelte/icons/menu";
  import MoreHorizontalIcon from "@lucide/svelte/icons/ellipsis";
  import SearchIcon from "@lucide/svelte/icons/search";

  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import * as Breadcrumb from "$lib/components/ui/breadcrumb";
  import { Button } from "$lib/components/ui/button";
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu";
  import { Input } from "$lib/components/ui/input";

  let { state }: { state: ExplorerState } = $props();
</script>

<div class="flex min-h-14 items-center gap-2 border-b px-3 py-2">
  <Button
    variant="ghost"
    size="icon-sm"
    class="md:hidden"
    aria-label="Open locations"
    onclick={() => (state.mobileSidebarOpen = true)}
  >
    <MenuIcon />
  </Button>
  <div class="flex items-center">
    <Button
      variant="ghost"
      size="icon-sm"
      disabled={!state.canGoBack}
      aria-label="Go back"
      onclick={() => void state.goBack()}
    >
      <ArrowLeftIcon />
    </Button>
    <Button
      variant="ghost"
      size="icon-sm"
      disabled={!state.canGoForward}
      aria-label="Go forward"
      onclick={() => void state.goForward()}
    >
      <ArrowRightIcon />
    </Button>
    <Button
      variant="ghost"
      size="icon-sm"
      disabled={!state.canGoUp}
      aria-label="Go to parent folder"
      onclick={() => void state.goUp()}
    >
      <ArrowUpIcon />
    </Button>
  </div>

  <div class="min-w-0 flex-1 overflow-hidden px-1">
    <Breadcrumb.Root>
      <Breadcrumb.List class="flex-nowrap">
        {#each state.breadcrumbs as segment, index (segment.directory.id)}
          {#if index > 0}
            <Breadcrumb.Separator><ChevronRightIcon /></Breadcrumb.Separator>
          {/if}
          <Breadcrumb.Item
            class={index === state.breadcrumbs.length - 1
              ? "min-w-0"
              : "shrink-0"}
          >
            {#if index === state.breadcrumbs.length - 1}
              <Breadcrumb.Page class="truncate">{segment.label}</Breadcrumb.Page
              >
            {:else}
              <button
                type="button"
                class="max-w-36 truncate transition-colors hover:text-foreground"
                title={segment.directory.displayPath}
                onclick={() => void state.openDirectory(segment.directory)}
              >
                {segment.label}
              </button>
            {/if}
          </Breadcrumb.Item>
        {/each}
        {#if state.breadcrumbs.length === 0}
          <Breadcrumb.Item>
            <Breadcrumb.Page>Loading</Breadcrumb.Page>
          </Breadcrumb.Item>
        {/if}
      </Breadcrumb.List>
    </Breadcrumb.Root>
  </div>

  <div class="relative hidden w-52 sm:block">
    <SearchIcon
      class="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground"
    />
    <Input
      bind:value={state.searchQuery}
      aria-label="Search this location"
      placeholder="Search"
      class="pl-8"
    />
  </div>

  <div class="flex rounded-md border p-0.5">
    <Button
      variant={state.viewMode === "list" ? "secondary" : "ghost"}
      size="icon-xs"
      aria-label="List view"
      aria-pressed={state.viewMode === "list"}
      onclick={() => state.setViewMode("list")}
    >
      <ListIcon />
    </Button>
    <Button
      variant={state.viewMode === "grid" ? "secondary" : "ghost"}
      size="icon-xs"
      aria-label="Grid view"
      aria-pressed={state.viewMode === "grid"}
      onclick={() => state.setViewMode("grid")}
    >
      <GridIcon />
    </Button>
  </div>

  <DropdownMenu.Root>
    <DropdownMenu.Trigger>
      {#snippet child({ props })}
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="More options"
          {...props}
        >
          <MoreHorizontalIcon />
        </Button>
      {/snippet}
    </DropdownMenu.Trigger>
    <DropdownMenu.Content align="end">
      <DropdownMenu.Label>Actions</DropdownMenu.Label>
      <DropdownMenu.Item disabled>New folder</DropdownMenu.Item>
      <DropdownMenu.Item disabled>Connect to server</DropdownMenu.Item>
      <DropdownMenu.Separator />
      <DropdownMenu.Item onclick={() => void state.openTab()}
        >Open current location in new tab</DropdownMenu.Item
      >
    </DropdownMenu.Content>
  </DropdownMenu.Root>
</div>
