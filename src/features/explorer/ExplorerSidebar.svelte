<script lang="ts">
  import CheckIcon from "@lucide/svelte/icons/check";
  import CloudIcon from "@lucide/svelte/icons/cloud";
  import DownloadIcon from "@lucide/svelte/icons/download";
  import CircleMinusIcon from "@lucide/svelte/icons/circle-minus";
  import CirclePlusIcon from "@lucide/svelte/icons/circle-plus";
  import FileTextIcon from "@lucide/svelte/icons/file-text";
  import FilmIcon from "@lucide/svelte/icons/film";
  import HardDriveIcon from "@lucide/svelte/icons/hard-drive";
  import HouseIcon from "@lucide/svelte/icons/house";
  import ImagesIcon from "@lucide/svelte/icons/images";
  import LaptopIcon from "@lucide/svelte/icons/laptop";
  import MonitorIcon from "@lucide/svelte/icons/monitor";
  import Music2Icon from "@lucide/svelte/icons/music-2";
  import PanelLeftCloseIcon from "@lucide/svelte/icons/panel-left-close";
  import PanelLeftOpenIcon from "@lucide/svelte/icons/panel-left-open";
  import PencilIcon from "@lucide/svelte/icons/pencil";
  import PlusIcon from "@lucide/svelte/icons/plus";
  import ServerIcon from "@lucide/svelte/icons/server";
  import Settings2Icon from "@lucide/svelte/icons/settings-2";
  import Trash2Icon from "@lucide/svelte/icons/trash-2";
  import UnplugIcon from "@lucide/svelte/icons/unplug";
  import XIcon from "@lucide/svelte/icons/x";

  import type { ExplorerState } from "../../app/explorer-state.svelte";
  import type { WindowChromeMode } from "../../app/window-chrome.svelte";
  import { Button } from "$lib/components/ui/button";
  import * as ContextMenu from "$lib/components/ui/context-menu";
  import * as Sheet from "$lib/components/ui/sheet";
  import type {
    LocationRole,
    LocationSummary,
    SshTargetSummary,
  } from "$lib/contracts/explorer";
  import { isFavoriteRole } from "$lib/contracts/preferences";
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
    syncedFolder: CloudIcon,
    ssh: ServerIcon,
  } satisfies Record<LocationRole, typeof HouseIcon>;

  const iconFor = (role: LocationRole) => iconsByRole[role];

  const favoriteIsVisible = (role: LocationRole) =>
    isFavoriteRole(role) && state.favoriteRoles.includes(role);

  const toggleFavorite = (role: LocationRole) => {
    if (!isFavoriteRole(role)) return;
    state.setFavoriteVisible(role, !state.favoriteRoles.includes(role));
  };

  const finishEditingFavorites = (restoreFocus = false) => {
    state.editingFavorites = false;
    if (restoreFocus) focusVisibleEditor("[data-favorites-editor]");
  };

  const sshTargetIsVisible = (targetId: string) =>
    !state.hiddenSshTargetIds.includes(targetId);

  const finishEditingSshTargets = (restoreFocus = false) => {
    state.editingSshTargets = false;
    if (restoreFocus) focusVisibleEditor("[data-ssh-editor]");
  };

  const syncedFolderIsVisible = (folderId: string) =>
    !state.hiddenSyncedFolderIds.includes(folderId);

  const finishEditingSyncedFolders = (restoreFocus = false) => {
    state.editingSyncedFolders = false;
    if (restoreFocus) focusVisibleEditor("[data-synced-folder-editor]");
  };

  const focusVisibleEditor = (selector: string) => {
    queueMicrotask(() => {
      const buttons = [
        ...document.querySelectorAll<HTMLButtonElement>(selector),
      ];
      (
        buttons.find((button) => button.offsetParent !== null) ?? buttons[0]
      )?.focus();
    });
  };

  const handleKeydown = (event: KeyboardEvent) => {
    if (event.key !== "Escape") return;
    if (
      !state.editingFavorites &&
      !state.editingSyncedFolders &&
      !state.editingSshTargets
    )
      return;
    event.preventDefault();
    if (state.editingFavorites) finishEditingFavorites(true);
    else if (state.editingSyncedFolders) finishEditingSyncedFolders(true);
    else finishEditingSshTargets(true);
  };
</script>

<svelte:window onkeydown={handleKeydown} />

{#snippet sshTargetButton(
  target: SshTargetSummary,
  compact: boolean,
  contextMenuTrigger: boolean,
)}
  <Button
    variant={target.locationId === state.activeLocation?.id
      ? "secondary"
      : "ghost"}
    size={compact ? "icon" : "sm"}
    class={compact
      ? "relative w-full"
      : contextMenuTrigger
        ? "w-full min-w-0 justify-start gap-2"
        : "min-w-0 flex-1 justify-start gap-2"}
    aria-current={target.locationId === state.activeLocation?.id
      ? "page"
      : undefined}
    title={`${target.name} · ${target.endpoint} · ${target.status}`}
    onclick={() => void state.selectSshTarget(target.id)}
  >
    <ServerIcon />
    {#if !compact}
      <span class="min-w-0 flex-1 truncate text-left">{target.name}</span>
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
{/snippet}

{#snippet syncedFolderButton(location: LocationSummary, compact: boolean)}
  <Button
    variant={location.id === state.activeLocation?.id ? "secondary" : "ghost"}
    size={compact ? "icon" : "sm"}
    class={compact ? "relative w-full" : "w-full min-w-0 justify-start gap-2"}
    aria-current={location.id === state.activeLocation?.id ? "page" : undefined}
    disabled={location.status === "offline"}
    title={`${location.name} · ${location.detail}`}
    onclick={() => void state.selectLocation(location.id)}
  >
    <CloudIcon />
    {#if !compact}
      <span class="min-w-0 flex-1 truncate text-left">{location.name}</span>
      <span
        class={cn(
          "size-2 rounded-full",
          location.syncedFolder?.status === "available"
            ? "bg-emerald-500"
            : location.syncedFolder?.status === "error"
              ? "bg-destructive"
              : location.syncedFolder?.status === "paused"
                ? "bg-amber-500"
                : "bg-muted-foreground/40",
        )}
        aria-label={location.syncedFolder?.status ?? "unknown"}
      ></span>
    {/if}
  </Button>
{/snippet}

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

    <div
      class="explora-sidebar-scroll min-h-0 flex-1 overflow-y-auto px-2 pb-4"
    >
      {#if !compact}
        <div
          class="group/favorites flex items-center justify-between pt-3 pb-1 pl-2"
        >
          <p class="text-xs font-medium text-muted-foreground">Favorites</p>
          <Button
            variant="ghost"
            size="icon-xs"
            data-favorites-editor
            class={cn(
              "transition-opacity",
              state.editingFavorites
                ? "opacity-100"
                : "opacity-100 md:opacity-0 md:group-focus-within/favorites:opacity-100 md:group-hover/favorites:opacity-100",
            )}
            title={state.editingFavorites
              ? "Finish editing favorites"
              : "Configure favorites"}
            aria-label={state.editingFavorites
              ? "Finish editing favorites"
              : "Configure favorites"}
            aria-pressed={state.editingFavorites}
            onclick={() => {
              if (state.editingFavorites) finishEditingFavorites();
              else {
                finishEditingSshTargets();
                finishEditingSyncedFolders();
                state.editingFavorites = true;
              }
            }}
          >
            {#if state.editingFavorites}<CheckIcon />{:else}<Settings2Icon
              />{/if}
          </Button>
        </div>
      {/if}
      <nav aria-label="Favorites" class="space-y-1">
        {#each state.editingFavorites ? state.availableFavoriteLocations : state.visibleFavoriteLocations as location (location.id)}
          {@const Icon = iconFor(location.role)}
          {@const activeFavorite = favoriteIsVisible(location.role)}
          <div
            class="flex min-w-0 items-center gap-1"
            data-favorite-active={activeFavorite}
          >
            {#if activeFavorite}
              <Button
                variant={state.activeLocation?.id === location.id
                  ? "secondary"
                  : "ghost"}
                size={compact ? "icon" : "sm"}
                class={compact
                  ? "w-full"
                  : "min-w-0 flex-1 justify-start gap-2"}
                aria-current={state.activeLocation?.id === location.id
                  ? "page"
                  : undefined}
                title={compact ? location.name : undefined}
                onclick={() => void state.selectLocation(location.id)}
              >
                <Icon />
                {#if !compact}<span class="truncate">{location.name}</span>{/if}
              </Button>
            {:else}
              <div
                class="flex h-8 min-w-0 flex-1 items-center gap-2 rounded-md border border-dashed border-border/60 bg-muted/20 px-2.5 text-sm text-muted-foreground"
                aria-label={`${location.name}, not in Favorites`}
              >
                <Icon class="size-4 shrink-0 opacity-60" />
                <span class="truncate">{location.name}</span>
              </div>
            {/if}
            {#if state.editingFavorites && isFavoriteRole(location.role)}
              <Button
                variant="ghost"
                size="icon-xs"
                class={activeFavorite
                  ? "text-muted-foreground hover:text-destructive"
                  : "text-emerald-600 hover:bg-emerald-500/10 hover:text-emerald-700 dark:text-emerald-400 dark:hover:text-emerald-300"}
                title={activeFavorite
                  ? `Remove ${location.name} from Favorites`
                  : `Add ${location.name} to Favorites`}
                aria-label={activeFavorite
                  ? `Remove ${location.name} from Favorites`
                  : `Add ${location.name} to Favorites`}
                onclick={() => toggleFavorite(location.role)}
              >
                {#if activeFavorite}<CircleMinusIcon />{:else}<CirclePlusIcon
                  />{/if}
              </Button>
            {/if}
          </div>
        {/each}
      </nav>

      {#if !compact && state.locations.some(({ kind, status }) => kind === "volume" && status === "available")}
        <p class="px-2 pt-5 pb-1 text-xs font-medium text-muted-foreground">
          Locations
        </p>
      {/if}
      <nav aria-label="Mounted locations" class="space-y-1">
        {#each state.locations.filter(({ kind, status }) => kind === "volume" && status === "available") as location (location.id)}
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

      {#if state.syncedFolderLocations.length > 0}
        <div
          class={cn(
            "group/synced-folders flex items-center pt-5 pb-1",
            compact ? "justify-center px-1" : "justify-between pl-2",
          )}
        >
          {#if compact}
            <CloudIcon
              class="size-3.5 text-muted-foreground"
              aria-hidden="true"
            />
          {:else}
            <p class="text-xs font-medium text-muted-foreground">
              Cloud Storage
            </p>
            <Button
              variant="ghost"
              size="icon-xs"
              data-synced-folder-editor
              class={cn(
                "opacity-100 transition-opacity",
                !state.editingSyncedFolders &&
                  "md:opacity-0 md:group-focus-within/synced-folders:opacity-100 md:group-hover/synced-folders:opacity-100",
              )}
              title={state.editingSyncedFolders
                ? "Finish editing cloud storage"
                : "Configure cloud storage"}
              aria-label={state.editingSyncedFolders
                ? "Finish editing cloud storage"
                : "Configure cloud storage"}
              aria-pressed={state.editingSyncedFolders}
              onclick={() => {
                if (state.editingSyncedFolders) finishEditingSyncedFolders();
                else {
                  finishEditingFavorites();
                  finishEditingSshTargets();
                  state.editingSyncedFolders = true;
                }
              }}
            >
              {#if state.editingSyncedFolders}<CheckIcon />{:else}<Settings2Icon
                />{/if}
            </Button>
          {/if}
        </div>
        <nav aria-label="Cloud storage" class="space-y-1">
          {#each state.editingSyncedFolders && !compact ? state.syncedFolderLocations : state.visibleSyncedFolderLocations as location (location.id)}
            {@const visibleSyncedFolder = syncedFolderIsVisible(location.id)}
            <div
              class="flex min-w-0 items-center gap-1"
              data-synced-folder-visible={visibleSyncedFolder}
            >
              {#if visibleSyncedFolder}
                <div class="min-w-0 flex-1">
                  {@render syncedFolderButton(location, compact)}
                </div>
              {:else}
                <div
                  class="flex h-8 min-w-0 flex-1 items-center gap-2 rounded-md border border-dashed border-border/60 bg-muted/20 px-2.5 text-sm text-muted-foreground"
                  aria-label={`${location.name}, hidden from Cloud Storage`}
                >
                  <CloudIcon class="size-4 shrink-0 opacity-60" />
                  <span class="min-w-0 flex-1 truncate">{location.name}</span>
                </div>
              {/if}
              {#if state.editingSyncedFolders && !compact}
                <Button
                  variant="ghost"
                  size="icon-xs"
                  class={visibleSyncedFolder
                    ? "text-muted-foreground hover:text-destructive"
                    : "text-emerald-600 hover:bg-emerald-500/10 hover:text-emerald-700 dark:text-emerald-400 dark:hover:text-emerald-300"}
                  title={visibleSyncedFolder
                    ? `Hide ${location.name} from Cloud Storage`
                    : `Show ${location.name} in Cloud Storage`}
                  aria-label={visibleSyncedFolder
                    ? `Hide ${location.name} from Cloud Storage`
                    : `Show ${location.name} in Cloud Storage`}
                  onclick={() =>
                    state.setSyncedFolderVisible(
                      location.id,
                      !visibleSyncedFolder,
                    )}
                >
                  {#if visibleSyncedFolder}<CircleMinusIcon
                    />{:else}<CirclePlusIcon />{/if}
                </Button>
              {/if}
            </div>
          {/each}
        </nav>
        {#if !compact && !state.editingSyncedFolders && state.visibleSyncedFolderLocations.length === 0}
          <p class="px-2 py-2 text-xs text-muted-foreground">
            All cloud storage locations are hidden. Configure Cloud Storage to
            show one.
          </p>
        {/if}
      {/if}

      <div
        class={cn(
          "group/ssh flex items-center pt-5 pb-1",
          compact ? "justify-center px-1" : "justify-between pl-2",
        )}
      >
        {#if compact}
          <Button
            variant="ghost"
            size="icon-xs"
            title="Add SSH target"
            aria-label="Add SSH target"
            onclick={() => state.openNewSshTarget()}
          >
            <PlusIcon />
          </Button>
        {:else}
          <p class="text-xs font-medium text-muted-foreground">SSH</p>
          <div class="flex items-center">
            {#if state.sshTargets.length > 0}
              <Button
                variant="ghost"
                size="icon-xs"
                data-ssh-editor
                class={cn(
                  "opacity-100 transition-opacity",
                  !state.editingSshTargets &&
                    "md:opacity-0 md:group-focus-within/ssh:opacity-100 md:group-hover/ssh:opacity-100",
                )}
                title={state.editingSshTargets
                  ? "Finish editing SSH targets"
                  : "Configure SSH targets"}
                aria-label={state.editingSshTargets
                  ? "Finish editing SSH targets"
                  : "Configure SSH targets"}
                aria-pressed={state.editingSshTargets}
                onclick={() => {
                  if (state.editingSshTargets) finishEditingSshTargets();
                  else {
                    finishEditingFavorites();
                    finishEditingSyncedFolders();
                    state.editingSshTargets = true;
                  }
                }}
              >
                {#if state.editingSshTargets}<CheckIcon />{:else}<Settings2Icon
                  />{/if}
              </Button>
            {/if}
            {#if !state.editingSshTargets}
              <Button
                variant="ghost"
                size="icon-xs"
                title="Add SSH target"
                aria-label="Add SSH target"
                onclick={() => state.openNewSshTarget()}
              >
                <PlusIcon />
              </Button>
            {/if}
          </div>
        {/if}
      </div>
      <nav aria-label="SSH targets" class="space-y-1">
        {#each state.editingSshTargets && !compact ? state.sshTargets : state.visibleSshTargets as target (target.id)}
          {@const visibleSshTarget = sshTargetIsVisible(target.id)}
          <div
            class="group flex min-w-0 items-center gap-1"
            data-ssh-target-visible={visibleSshTarget}
          >
            {#if visibleSshTarget}
              {#if !state.editingSshTargets && (target.editable || target.status === "connected")}
                <ContextMenu.Root>
                  <ContextMenu.Trigger
                    class={compact ? "w-full" : "min-w-0 flex-1"}
                  >
                    {@render sshTargetButton(target, compact, true)}
                  </ContextMenu.Trigger>
                  <ContextMenu.Content>
                    {#if target.editable}
                      <ContextMenu.Item
                        onclick={() => state.openEditSshTarget(target.id)}
                      >
                        <PencilIcon />
                        Edit
                      </ContextMenu.Item>
                    {/if}
                    {#if target.status === "connected"}
                      <ContextMenu.Item
                        onclick={() =>
                          void state.disconnectSshTarget(target.id)}
                      >
                        <UnplugIcon />
                        Disconnect
                      </ContextMenu.Item>
                    {/if}
                    {#if target.editable}
                      <ContextMenu.Separator />
                      <ContextMenu.Item
                        variant="destructive"
                        onclick={() => {
                          if (
                            window.confirm(
                              `Remove SSH target “${target.name}”?`,
                            )
                          ) {
                            void state.deleteSshTarget(target.id);
                          }
                        }}
                      >
                        <Trash2Icon />
                        Remove
                      </ContextMenu.Item>
                    {/if}
                  </ContextMenu.Content>
                </ContextMenu.Root>
              {:else}
                {@render sshTargetButton(target, compact, false)}
              {/if}
            {:else}
              <div
                class="flex h-8 min-w-0 flex-1 items-center gap-2 rounded-md border border-dashed border-border/60 bg-muted/20 px-2.5 text-sm text-muted-foreground"
                aria-label={`${target.name}, hidden from SSH`}
              >
                <ServerIcon class="size-4 shrink-0 opacity-60" />
                <span class="min-w-0 flex-1 truncate">{target.name}</span>
              </div>
            {/if}

            {#if state.editingSshTargets && !compact}
              <Button
                variant="ghost"
                size="icon-xs"
                class={visibleSshTarget
                  ? "text-muted-foreground hover:text-destructive"
                  : "text-emerald-600 hover:bg-emerald-500/10 hover:text-emerald-700 dark:text-emerald-400 dark:hover:text-emerald-300"}
                title={visibleSshTarget
                  ? `Hide ${target.name} from SSH`
                  : `Show ${target.name} in SSH`}
                aria-label={visibleSshTarget
                  ? `Hide ${target.name} from SSH`
                  : `Show ${target.name} in SSH`}
                onclick={() =>
                  state.setSshTargetVisible(target.id, !visibleSshTarget)}
              >
                {#if visibleSshTarget}<CircleMinusIcon />{:else}<CirclePlusIcon
                  />{/if}
              </Button>
            {/if}
          </div>
        {/each}
      </nav>
      {#if !compact && state.sshTargets.length === 0}
        <p class="px-2 py-2 text-xs text-muted-foreground">
          Add a server or define a concrete Host in ~/.ssh/config.
        </p>
      {/if}
      {#if !compact && !state.editingSshTargets && state.sshTargets.length > 0 && state.visibleSshTargets.length === 0}
        <p class="px-2 py-2 text-xs text-muted-foreground">
          All SSH targets are hidden. Configure SSH to show one.
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
    onclick={() => {
      finishEditingFavorites();
      finishEditingSyncedFolders();
      finishEditingSshTargets();
      state.setSidebarCollapsed(!state.sidebarCollapsed);
    }}
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
