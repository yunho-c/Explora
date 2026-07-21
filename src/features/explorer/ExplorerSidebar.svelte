<script lang="ts">
  import DownloadIcon from "@lucide/svelte/icons/download";
  import FileTextIcon from "@lucide/svelte/icons/file-text";
  import FilmIcon from "@lucide/svelte/icons/film";
  import HardDriveIcon from "@lucide/svelte/icons/hard-drive";
  import HouseIcon from "@lucide/svelte/icons/house";
  import ImagesIcon from "@lucide/svelte/icons/images";
  import LaptopIcon from "@lucide/svelte/icons/laptop";
  import MonitorIcon from "@lucide/svelte/icons/monitor";
  import Music2Icon from "@lucide/svelte/icons/music-2";
  import MoreHorizontalIcon from "@lucide/svelte/icons/more-horizontal";
  import PanelLeftCloseIcon from "@lucide/svelte/icons/panel-left-close";
  import PanelLeftOpenIcon from "@lucide/svelte/icons/panel-left-open";
  import PencilIcon from "@lucide/svelte/icons/pencil";
  import PlusIcon from "@lucide/svelte/icons/plus";
  import ServerIcon from "@lucide/svelte/icons/server";
  import Trash2Icon from "@lucide/svelte/icons/trash-2";
  import UnplugIcon from "@lucide/svelte/icons/unplug";
  import XIcon from "@lucide/svelte/icons/x";

  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import type { WindowChromeMode } from "../../app/window-chrome.svelte";
  import { Badge } from "$lib/components/ui/badge";
  import { Button } from "$lib/components/ui/button";
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu";
  import * as Sheet from "$lib/components/ui/sheet";
  import type { LocationRole } from "$lib/contracts/explorer";
  import { cn } from "$lib/utils";

  let {
    state,
    chromeMode,
  }: { state: ExplorerState; chromeMode: WindowChromeMode } = $props();

  const customChrome = $derived(
    chromeMode === "activating" || chromeMode === "custom",
  );

  const iconsByRole = {
    home: HouseIcon,
    desktop: MonitorIcon,
    documents: FileTextIcon,
    downloads: DownloadIcon,
    pictures: ImagesIcon,
    music: Music2Icon,
    videos: FilmIcon,
    volume: HardDriveIcon,
    ssh: ServerIcon,
  } satisfies Record<LocationRole, typeof HouseIcon>;

  const iconFor = (role: LocationRole) => iconsByRole[role];
</script>

{#snippet navigation(compact: boolean, titlebar: boolean)}
  <div class="flex h-full min-h-0 flex-col">
    <div
      class={cn(
        "flex items-center gap-2 overflow-hidden",
        titlebar
          ? "explora-titlebar-content explora-titlebar-left h-8 shrink-0 bg-muted/30"
          : "h-14 px-3",
      )}
      data-tauri-drag-region={titlebar && customChrome ? "" : undefined}
    >
      {#if !titlebar}
        <div
          class="grid size-8 shrink-0 place-items-center rounded-lg bg-primary text-primary-foreground"
        >
          <LaptopIcon class="size-4" />
        </div>
        {#if !compact}
          <p class="truncate text-sm font-semibold">Explora</p>
        {/if}
      {/if}
    </div>

    <div class="min-h-0 flex-1 overflow-y-auto px-2 pb-4">
      {#if !compact}
        <p class="px-2 pt-3 pb-1 text-xs font-medium text-muted-foreground">
          Favorites
        </p>
      {/if}
      <nav aria-label="Favorites" class="space-y-1">
        {#each state.locations.filter(({ kind }) => kind === "local") as location (location.id)}
          {@const Icon = iconFor(location.role)}
          <Button
            variant={state.activeLocation?.id === location.id
              ? "secondary"
              : "ghost"}
            size={compact ? "icon" : "sm"}
            class={compact ? "w-full" : "w-full justify-start gap-2"}
            aria-current={state.activeLocation?.id === location.id
              ? "page"
              : undefined}
            title={compact ? location.name : undefined}
            onclick={() => void state.selectLocation(location.id)}
          >
            <Icon />
            {#if !compact}<span class="truncate">{location.name}</span>{/if}
          </Button>
        {/each}
      </nav>

      {#if !compact}
        <p class="px-2 pt-5 pb-1 text-xs font-medium text-muted-foreground">
          Locations
        </p>
      {/if}
      <nav aria-label="Mounted locations" class="space-y-1">
        {#each state.locations.filter(({ kind }) => kind === "volume") as location (location.id)}
          {@const Icon = iconFor(location.role)}
          <Button
            variant={state.activeLocation?.id === location.id
              ? "secondary"
              : "ghost"}
            size={compact ? "icon" : "sm"}
            class={compact ? "w-full" : "w-full justify-start gap-2"}
            aria-current={state.activeLocation?.id === location.id
              ? "page"
              : undefined}
            title={compact ? location.name : undefined}
            onclick={() => void state.selectLocation(location.id)}
          >
            <Icon />
            {#if !compact}<span class="truncate">{location.name}</span>{/if}
          </Button>
        {/each}
      </nav>

      <div
        class={cn(
          "flex items-center pt-5 pb-1",
          compact ? "justify-center px-1" : "justify-between px-2",
        )}
      >
        {#if !compact}
          <p class="text-xs font-medium text-muted-foreground">SSH</p>
        {/if}
        <Button
          variant="ghost"
          size="icon-xs"
          title="Add SSH target"
          aria-label="Add SSH target"
          onclick={() => state.openNewSshTarget()}
        >
          <PlusIcon />
        </Button>
      </div>
      <nav aria-label="SSH targets" class="space-y-1">
        {#each state.sshTargets as target (target.id)}
          <div class="group flex min-w-0 items-center">
            <Button
              variant={target.connectedLocationId === state.activeLocation?.id
                ? "secondary"
                : "ghost"}
              size={compact ? "icon" : "sm"}
              class={compact
                ? "relative w-full"
                : "min-w-0 flex-1 justify-start gap-2"}
              aria-current={target.connectedLocationId ===
              state.activeLocation?.id
                ? "page"
                : undefined}
              title={`${target.name} · ${target.endpoint} · ${target.status}`}
              onclick={() => void state.selectSshTarget(target.id)}
            >
              <ServerIcon />
              {#if !compact}
                <span class="min-w-0 flex-1 truncate text-left"
                  >{target.name}</span
                >
                {#if target.source === "openSshConfig"}
                  <Badge variant="outline" class="px-1 text-[10px]"
                    >Config</Badge
                  >
                {/if}
                <span
                  class={cn(
                    "size-2 rounded-full",
                    target.status === "connected"
                      ? "bg-emerald-500"
                      : target.status === "connecting"
                        ? "animate-pulse bg-amber-500"
                        : target.status === "error"
                          ? "bg-destructive"
                          : "bg-muted-foreground/40",
                  )}
                  aria-label={target.status}
                ></span>
              {:else}
                <span
                  class={cn(
                    "absolute right-1.5 bottom-1.5 size-2 rounded-full ring-2 ring-sidebar",
                    target.status === "connected"
                      ? "bg-emerald-500"
                      : target.status === "connecting"
                        ? "animate-pulse bg-amber-500"
                        : target.status === "error"
                          ? "bg-destructive"
                          : "bg-muted-foreground/40",
                  )}
                ></span>
              {/if}
            </Button>

            {#if !compact}
              <DropdownMenu.Root>
                <DropdownMenu.Trigger>
                  {#snippet child({ props })}
                    <Button
                      {...props}
                      variant="ghost"
                      size="icon-xs"
                      class="opacity-0 group-hover:opacity-100 focus:opacity-100"
                      aria-label={`Manage ${target.name}`}
                    >
                      <MoreHorizontalIcon />
                    </Button>
                  {/snippet}
                </DropdownMenu.Trigger>
                <DropdownMenu.Content align="start">
                  {#if target.editable}
                    <DropdownMenu.Item
                      onclick={() => state.openEditSshTarget(target.id)}
                    >
                      <PencilIcon />
                      Edit
                    </DropdownMenu.Item>
                  {/if}
                  {#if target.status === "connected"}
                    <DropdownMenu.Item
                      onclick={() => void state.disconnectSshTarget(target.id)}
                    >
                      <UnplugIcon />
                      Disconnect
                    </DropdownMenu.Item>
                  {/if}
                  {#if target.editable}
                    <DropdownMenu.Separator />
                    <DropdownMenu.Item
                      variant="destructive"
                      onclick={() => {
                        if (
                          window.confirm(`Remove SSH target “${target.name}”?`)
                        ) {
                          void state.deleteSshTarget(target.id);
                        }
                      }}
                    >
                      <Trash2Icon />
                      Remove
                    </DropdownMenu.Item>
                  {/if}
                </DropdownMenu.Content>
              </DropdownMenu.Root>
            {/if}
          </div>
        {/each}
      </nav>
      {#if !compact && state.sshTargets.length === 0}
        <p class="px-2 py-2 text-xs text-muted-foreground">
          Add a server or define a concrete Host in ~/.ssh/config.
        </p>
      {/if}
      {#if state.connectingTargetId}
        <div
          class={cn(
            "mt-2 flex items-center gap-2 text-xs text-muted-foreground",
            compact ? "justify-center px-1" : "px-2",
          )}
          role="status"
        >
          {#if !compact}
            <span class="min-w-0 flex-1 truncate">
              {state.sshConnectionMessage ?? "Connecting…"}
            </span>
          {/if}
          <Button
            variant="ghost"
            size="icon-xs"
            title="Cancel SSH connection"
            aria-label="Cancel SSH connection"
            onclick={() => state.cancelSshConnection()}
          >
            <XIcon />
          </Button>
        </div>
      {/if}
      {#if !compact && state.sshErrorMessage}
        <p class="px-2 py-2 text-xs text-destructive" role="status">
          {state.sshErrorMessage}
        </p>
      {/if}
    </div>
  </div>
{/snippet}

<aside
  class={cn(
    "relative hidden h-full shrink-0 border-r bg-sidebar text-sidebar-foreground transition-[width] duration-200 md:block",
    state.sidebarCollapsed ? "w-16" : "w-60",
  )}
>
  {@render navigation(state.sidebarCollapsed, true)}
  <Button
    variant="ghost"
    size="icon-sm"
    class="absolute bottom-3 ml-3"
    title={state.sidebarCollapsed ? "Expand sidebar" : "Collapse sidebar"}
    aria-label={state.sidebarCollapsed ? "Expand sidebar" : "Collapse sidebar"}
    onclick={() => (state.sidebarCollapsed = !state.sidebarCollapsed)}
  >
    {#if state.sidebarCollapsed}<PanelLeftOpenIcon />{:else}<PanelLeftCloseIcon
      />{/if}
  </Button>
</aside>

<Sheet.Root bind:open={state.mobileSidebarOpen}>
  <Sheet.Content side="left">
    <Sheet.Header>
      <Sheet.Title>Locations</Sheet.Title>
      <Sheet.Description>Choose a favorite or saved location.</Sheet.Description
      >
    </Sheet.Header>
    <div class="min-h-0 flex-1">{@render navigation(false, false)}</div>
  </Sheet.Content>
</Sheet.Root>
