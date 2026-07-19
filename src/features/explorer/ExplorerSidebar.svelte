<script lang="ts">
  import HardDriveIcon from "@lucide/svelte/icons/hard-drive";
  import HouseIcon from "@lucide/svelte/icons/house";
  import LaptopIcon from "@lucide/svelte/icons/laptop";
  import PanelLeftCloseIcon from "@lucide/svelte/icons/panel-left-close";
  import PanelLeftOpenIcon from "@lucide/svelte/icons/panel-left-open";
  import ServerIcon from "@lucide/svelte/icons/server";

  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import { Badge } from "$lib/components/ui/badge";
  import { Button } from "$lib/components/ui/button";
  import * as Sheet from "$lib/components/ui/sheet";
  import { cn } from "$lib/utils";

  let { state }: { state: ExplorerState } = $props();

  const iconFor = (kind: "local" | "volume" | "ssh") => {
    if (kind === "ssh") return ServerIcon;
    if (kind === "volume") return HardDriveIcon;
    return HouseIcon;
  };
</script>

{#snippet navigation(compact: boolean)}
  <div class="flex h-full min-h-0 flex-col">
    <div class="flex h-14 items-center gap-2 px-3">
      <div
        class="grid size-8 shrink-0 place-items-center rounded-lg bg-primary text-primary-foreground"
      >
        <LaptopIcon class="size-4" />
      </div>
      {#if !compact}
        <div class="min-w-0">
          <p class="truncate text-sm font-semibold">Explora</p>
          <p class="truncate text-xs text-muted-foreground">Local & remote</p>
        </div>
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
          {@const Icon = iconFor(location.kind)}
          <Button
            variant={state.activeLocation?.id === location.id
              ? "secondary"
              : "ghost"}
            size={compact ? "icon" : "sm"}
            class={compact ? "w-full" : "w-full justify-start"}
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
          {@const Icon = iconFor(location.kind)}
          <Button
            variant={state.activeLocation?.id === location.id
              ? "secondary"
              : "ghost"}
            size={compact ? "icon" : "sm"}
            class={compact ? "w-full" : "w-full justify-start"}
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

      {#if !compact && state.locations.some(({ kind }) => kind === "ssh")}
        <div class="flex items-center justify-between px-2 pt-5 pb-1">
          <p class="text-xs font-medium text-muted-foreground">SSH</p>
          <Badge variant="outline">Demo</Badge>
        </div>
      {/if}
      <nav aria-label="SSH locations" class="space-y-1">
        {#each state.locations.filter(({ kind }) => kind === "ssh") as location (location.id)}
          {@const Icon = iconFor(location.kind)}
          <Button
            variant={state.activeLocation?.id === location.id
              ? "secondary"
              : "ghost"}
            size={compact ? "icon" : "sm"}
            class={compact ? "relative w-full" : "w-full justify-start"}
            aria-current={state.activeLocation?.id === location.id
              ? "page"
              : undefined}
            title={compact
              ? `${location.name} · ${location.status}`
              : undefined}
            onclick={() => void state.selectLocation(location.id)}
          >
            <Icon />
            {#if !compact}
              <span class="min-w-0 flex-1 truncate text-left"
                >{location.name}</span
              >
              <span
                class={cn(
                  "size-2 rounded-full",
                  location.status === "connected"
                    ? "bg-emerald-500"
                    : "bg-muted-foreground/40",
                )}
                aria-label={location.status}
              ></span>
            {:else}
              <span
                class={cn(
                  "absolute right-1.5 bottom-1.5 size-2 rounded-full ring-2 ring-sidebar",
                  location.status === "connected"
                    ? "bg-emerald-500"
                    : "bg-muted-foreground/40",
                )}
              ></span>
            {/if}
          </Button>
        {/each}
      </nav>
    </div>
  </div>
{/snippet}

<aside
  class={cn(
    "hidden h-full shrink-0 border-r bg-sidebar text-sidebar-foreground transition-[width] duration-200 md:block",
    state.sidebarCollapsed ? "w-16" : "w-60",
  )}
>
  {@render navigation(state.sidebarCollapsed)}
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
    <div class="min-h-0 flex-1">{@render navigation(false)}</div>
  </Sheet.Content>
</Sheet.Root>
