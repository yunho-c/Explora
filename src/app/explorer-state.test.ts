import { describe, expect, it, vi } from "vitest";

import type {
  DirectoryRef,
  FileEntrySummary,
  LocationSummary,
  SshConnectionEvent,
  SyncedFolderSnapshot,
  VolumeSnapshot,
} from "$lib/contracts/explorer";
import type {
  PreferencesSnapshot,
  UserPreferences,
  UserPreferencesPatch,
} from "$lib/contracts/preferences";
import { DemoExplorerDataSource } from "$lib/data/demo-explorer-data-source";
import type {
  ConnectSshOptions,
  ListDirectoryOptions,
  PreparePreviewOptions,
  PreparedPreview,
  RequestContentOptions,
  WatchSyncedFoldersOptions,
  WatchVolumesOptions,
} from "$lib/data/explorer-data-source";
import { MemoryPreferencesDataSource } from "$lib/data/memory-preferences-data-source";
import type { PreferencesDataSource } from "$lib/data/preferences-data-source";

import { ExplorerState } from "./explorer-state.svelte";

const initializedState = async () => {
  const state = new ExplorerState(new DemoExplorerDataSource());
  await state.initialize();
  return state;
};

class StaleResultDataSource extends DemoExplorerDataSource {
  override async listDirectory(
    directory: DirectoryRef,
    options: ListDirectoryOptions,
  ): Promise<void> {
    if (directory.name !== "Projects") {
      return super.listDirectory(directory, options);
    }

    await new Promise((resolve) => window.setTimeout(resolve, 250));
    options.onStart({
      directory,
      parent: {
        id: "home",
        locationId: "home",
        name: "Home",
        displayPath: "Home",
      },
      breadcrumbs: [{ label: directory.name, directory }],
    });
    options.onComplete({ skippedEntries: 0 });
  }
}

class ObservableSshDataSource extends DemoExplorerDataSource {
  listingCount = 0;
  private onSshEvent: ConnectSshOptions["onEvent"] | null = null;

  override async connectSshTarget(
    targetId: string,
    options: ConnectSshOptions,
  ) {
    this.onSshEvent = options.onEvent;
    return super.connectSshTarget(targetId, options);
  }

  override async listDirectory(
    directory: DirectoryRef,
    options: ListDirectoryOptions,
  ): Promise<void> {
    this.listingCount += 1;
    return super.listDirectory(directory, options);
  }

  emitSshEvent(event: SshConnectionEvent): void {
    this.onSshEvent?.(event, async () => {});
  }
}

class StalePreviewDataSource extends DemoExplorerDataSource {
  readonly disposedEntryIds: string[] = [];
  readonly imageModes: PreparePreviewOptions["imageMode"][] = [];

  override async getPreview(
    entry: FileEntrySummary,
    { signal, imageMode }: PreparePreviewOptions,
  ): Promise<PreparedPreview> {
    void signal;
    this.imageModes.push(imageMode);
    if (entry.name === "explora-notes.md") {
      await new Promise((resolve) => window.setTimeout(resolve, 100));
    }
    return {
      preview: {
        entryId: entry.reference.id,
        kind: entry.contentKind,
        title: entry.name,
        accessibilityDescription: entry.displayPath,
        content: {
          type: "metadata",
          reason: "unsupported",
          message: "Test preview",
          requestContent: null,
        },
        details: [],
      },
      dispose: () => this.disposedEntryIds.push(entry.reference.id),
    };
  }
}

class SlowContentDataSource extends DemoExplorerDataSource {
  aborted = false;

  override async requestContent(
    entry: FileEntrySummary,
    { signal, onEvent }: RequestContentOptions,
  ): Promise<void> {
    onEvent({ event: "started", providerWorkCancellable: false });
    onEvent({ event: "progress", availability: entry.availability });
    await new Promise<void>((_resolve, reject) => {
      signal.addEventListener(
        "abort",
        () => {
          this.aborted = true;
          const error = new Error("Stopped waiting.");
          error.name = "AbortError";
          reject(error);
        },
        { once: true },
      );
    });
  }
}

class FailingPreferencesDataSource implements PreferencesDataSource {
  async getPreferences(): Promise<PreferencesSnapshot> {
    return {
      preferences: {
        layout: {
          sidebarCollapsed: false,
          viewMode: "list",
          sort: { column: "name", direction: "ascending" },
          favoriteRoles: [
            "home",
            "desktop",
            "documents",
            "downloads",
            "pictures",
            "music",
            "videos",
          ],
          hiddenSyncedFolderIds: [],
          hiddenSshTargetIds: [],
        },
      },
      warning: null,
    };
  }

  async updatePreferences(): Promise<UserPreferences> {
    throw new Error("The preference file is read-only.");
  }
}

class DelayedPreferencesDataSource extends MemoryPreferencesDataSource {
  override async updatePreferences(
    patch: UserPreferencesPatch,
  ): Promise<UserPreferences> {
    if (patch.layout.viewMode === "grid") {
      await new Promise((resolve) => window.setTimeout(resolve, 25));
    }
    return super.updatePreferences(patch);
  }
}

class HangingPreferencesDataSource extends MemoryPreferencesDataSource {
  override async getPreferences(): Promise<PreferencesSnapshot> {
    return new Promise(() => {});
  }
}

class ControllableVolumeDataSource extends DemoExplorerDataSource {
  private onVolumeSnapshot: ((snapshot: VolumeSnapshot) => void) | null = null;

  override async watchVolumes({
    signal,
    onSnapshot,
  }: WatchVolumesOptions): Promise<void> {
    this.onVolumeSnapshot = onSnapshot;
    await new Promise<void>((resolve) => {
      signal.addEventListener("abort", () => resolve(), { once: true });
    });
  }

  emitVolumes(revision: number, volumes: readonly LocationSummary[]): void {
    this.onVolumeSnapshot?.({ revision, volumes, warning: null });
  }
}

class ControllableSyncedFolderDataSource extends DemoExplorerDataSource {
  listingCount = 0;
  private onSyncedFolderSnapshot:
    ((snapshot: SyncedFolderSnapshot) => void) | null = null;

  override async watchSyncedFolders({
    signal,
    onSnapshot,
  }: WatchSyncedFoldersOptions): Promise<void> {
    this.onSyncedFolderSnapshot = onSnapshot;
    await new Promise<void>((resolve) => {
      signal.addEventListener("abort", () => resolve(), { once: true });
    });
  }

  override async listDirectory(
    directory: DirectoryRef,
    options: ListDirectoryOptions,
  ): Promise<void> {
    this.listingCount += 1;
    return super.listDirectory(directory, options);
  }

  emitSyncedFolders(
    revision: number,
    folders: readonly LocationSummary[],
  ): void {
    this.onSyncedFolderSnapshot?.({
      revision,
      folders,
      warning: null,
      canAddFolder: false,
    });
  }
}

describe("ExplorerState", () => {
  it("loads locations and directory batches through the data-source boundary", async () => {
    const state = await initializedState();

    expect(state.locations).toHaveLength(13);
    expect(
      state.locations
        .filter(({ kind }) => kind === "local")
        .map(({ role }) => role),
    ).toEqual([
      "home",
      "desktop",
      "documents",
      "downloads",
      "pictures",
      "music",
      "videos",
    ]);
    expect(state.activeLocation?.name).toBe("Home");
    expect(state.entries).toHaveLength(10);
    expect(state.activeDirectory?.name).toBe("Home");
    expect(state.breadcrumbs.map(({ label }) => label)).toEqual(["Home"]);
    expect(state.loading).toBe(false);
  });

  it("preserves an active volume tab across removal and restores it at the root", async () => {
    const dataSource = new ControllableVolumeDataSource();
    const state = new ExplorerState(dataSource);
    await state.initialize();
    const workspace = state.locations.find(({ id }) => id === "workspace");
    expect(workspace).toBeDefined();
    await state.selectLocation("workspace");

    dataSource.emitVolumes(1, []);
    await vi.waitFor(() =>
      expect(state.activeLocation?.status).toBe("offline"),
    );
    expect(state.activeTab?.locationId).toBe("workspace");
    expect(state.warningMessage).toContain("no longer available");

    dataSource.emitVolumes(2, [workspace as LocationSummary]);
    await vi.waitFor(() =>
      expect(state.activeLocation?.status).toBe("available"),
    );
    expect(state.activeDirectory?.id).toBe(workspace?.root.id);
    expect(state.warningMessage).toBeNull();
    state.dispose();
  });

  it("preserves an active synced-folder tab across removal and restores it at the root", async () => {
    const dataSource = new ControllableSyncedFolderDataSource();
    const state = new ExplorerState(dataSource);
    await state.initialize();
    const icloud = state.locations.find(({ id }) => id === "synced:icloud");
    expect(icloud).toBeDefined();
    await state.selectLocation("synced:icloud");

    dataSource.emitSyncedFolders(1, []);
    await vi.waitFor(() =>
      expect(state.activeLocation?.status).toBe("offline"),
    );
    expect(state.activeTab?.locationId).toBe("synced:icloud");
    expect(state.activeSyncedFolderOffline).toBe(true);
    expect(state.warningMessage).toContain("sync provider");
    const listingCount = dataSource.listingCount;
    await state.selectLocation("synced:icloud");
    expect(dataSource.listingCount).toBe(listingCount);
    expect(state.warningMessage).toContain("sync provider");

    dataSource.emitSyncedFolders(2, [icloud as LocationSummary]);
    await vi.waitFor(() =>
      expect(state.activeLocation?.status).toBe("available"),
    );
    expect(state.activeDirectory?.id).toBe(icloud?.root.id);
    expect(state.warningMessage).toBeNull();
    state.dispose();
  });

  it("keeps a configured manual folder in place while it is offline", async () => {
    const dataSource = new ControllableSyncedFolderDataSource();
    const state = new ExplorerState(dataSource);
    await state.initialize();
    const existing = state.locations.find(({ id }) => id === "synced:icloud");
    expect(existing).toBeDefined();
    await state.selectLocation("synced:icloud");
    const offline: LocationSummary = {
      ...(existing as LocationSummary),
      status: "offline",
      detail: "Manually added · Folder unavailable",
      syncedFolder: {
        provider: "other",
        status: "offline",
        source: "manual",
      },
    };

    dataSource.emitSyncedFolders(1, [offline]);
    await vi.waitFor(() =>
      expect(state.activeLocation?.status).toBe("offline"),
    );
    expect(state.activeTab?.locationId).toBe(offline.id);
    expect(state.warningMessage).toContain("Restore the folder");

    dataSource.emitSyncedFolders(2, [
      {
        ...offline,
        status: "available",
        detail: "Manually added · Synced folder",
        syncedFolder: { ...offline.syncedFolder!, status: "available" },
      },
    ]);
    await vi.waitFor(() =>
      expect(state.activeLocation?.status).toBe("available"),
    );
    expect(state.warningMessage).toBeNull();
    state.dispose();
  });

  it("filters, sorts, and switches views without mutating source entries", async () => {
    const state = await initializedState();
    const originalCount = state.entries.length;

    state.searchQuery = "explora";
    expect(state.visibleEntries.map(({ name }) => name)).toEqual([
      "explora-notes.md",
    ]);

    state.searchQuery = "";
    state.toggleSort("modifiedAt");
    state.setViewMode("grid");

    expect(state.entries).toHaveLength(originalCount);
    expect(state.viewMode).toBe("grid");
    expect(state.sort.column).toBe("modifiedAt");
  });

  it("restores global layout preferences before loading explorer data", async () => {
    const preferences = new MemoryPreferencesDataSource({
      layout: {
        sidebarCollapsed: true,
        viewMode: "grid",
        sort: { column: "size", direction: "descending" },
        favoriteRoles: ["home", "music"],
        hiddenSyncedFolderIds: ["synced:google-drive"],
        hiddenSshTargetIds: ["demo:render-node"],
      },
    });
    const state = new ExplorerState(new DemoExplorerDataSource(), preferences);

    await state.initialize();

    expect(state.sidebarCollapsed).toBe(true);
    expect(state.viewMode).toBe("grid");
    expect(state.sort).toEqual({ column: "size", direction: "descending" });
    expect(state.visibleFavoriteLocations.map(({ role }) => role)).toEqual([
      "home",
      "music",
    ]);
    expect(state.visibleSshTargets.map(({ id }) => id)).toEqual([
      "demo:staging-box",
    ]);
    expect(state.visibleSyncedFolderLocations.map(({ id }) => id)).toEqual([
      "synced:icloud",
      "synced:onedrive",
    ]);
  });

  it("persists favorite visibility in canonical sidebar order", async () => {
    const preferences = new MemoryPreferencesDataSource();
    const state = new ExplorerState(new DemoExplorerDataSource(), preferences);
    await state.initialize();

    state.setFavoriteVisible("downloads", false);
    state.setFavoriteVisible("home", false);
    await vi.waitFor(async () => {
      expect(
        (await preferences.getPreferences()).preferences.layout.favoriteRoles,
      ).toEqual(["desktop", "documents", "pictures", "music", "videos"]);
    });

    expect(
      state.visibleFavoriteLocations.map(({ role }) => role),
    ).not.toContain("home");
  });

  it("persists SSH target visibility without changing connection state", async () => {
    const preferences = new MemoryPreferencesDataSource();
    const state = new ExplorerState(new DemoExplorerDataSource(), preferences);
    await state.initialize();
    const connectedTarget = state.sshTargets.find(
      ({ id }) => id === "demo:staging-box",
    );

    state.setSshTargetVisible("demo:staging-box", false);
    await vi.waitFor(async () => {
      expect(
        (await preferences.getPreferences()).preferences.layout
          .hiddenSshTargetIds,
      ).toEqual(["demo:staging-box"]);
    });

    expect(state.visibleSshTargets.map(({ id }) => id)).not.toContain(
      "demo:staging-box",
    );
    expect(connectedTarget?.status).toBe("connected");
    expect(connectedTarget?.connectedLocationId).toBe("staging-box");
  });

  it("persists synced-folder visibility without changing discovery state", async () => {
    const preferences = new MemoryPreferencesDataSource();
    const state = new ExplorerState(new DemoExplorerDataSource(), preferences);
    await state.initialize();

    state.setSyncedFolderVisible("synced:onedrive", false);
    await vi.waitFor(async () => {
      expect(
        (await preferences.getPreferences()).preferences.layout
          .hiddenSyncedFolderIds,
      ).toEqual(["synced:onedrive"]);
    });

    expect(
      state.visibleSyncedFolderLocations.map(({ id }) => id),
    ).not.toContain("synced:onedrive");
    expect(
      state.syncedFolderLocations.find(({ id }) => id === "synced:onedrive")
        ?.status,
    ).toBe("available");
  });

  it("serializes rapid preference writes so the latest choice wins", async () => {
    const preferences = new DelayedPreferencesDataSource();
    const state = new ExplorerState(new DemoExplorerDataSource(), preferences);
    await state.initializePreferences();

    state.setViewMode("grid");
    state.setViewMode("list");
    await vi.waitFor(async () => {
      expect(
        (await preferences.getPreferences()).preferences.layout.viewMode,
      ).toBe("list");
    });

    expect(state.viewMode).toBe("list");
  });

  it("keeps optimistic layout state and reports preference write failures", async () => {
    const state = new ExplorerState(
      new DemoExplorerDataSource(),
      new FailingPreferencesDataSource(),
    );
    await state.initializePreferences();

    state.setSidebarCollapsed(true);
    await vi.waitFor(() =>
      expect(state.preferencesWarningMessage).toBe(
        "The preference file is read-only.",
      ),
    );

    expect(state.sidebarCollapsed).toBe(true);
  });

  it("bounds preference loading so a hidden application can still recover", async () => {
    vi.useFakeTimers();
    try {
      const state = new ExplorerState(
        new DemoExplorerDataSource(),
        new HangingPreferencesDataSource(),
      );
      const initialization = state.initializePreferences();

      await vi.advanceTimersByTimeAsync(2_000);
      await initialization;

      expect(state.preferencesWarningMessage).toBe(
        "Explora timed out while loading saved preferences.",
      );
    } finally {
      vi.useRealTimers();
    }
  });

  it("maintains independent tab state and navigation history", async () => {
    const state = await initializedState();

    await state.selectLocation("documents");
    expect(state.canGoBack).toBe(true);
    expect(state.activeLocation?.id).toBe("documents");

    await state.openTab("staging-box");
    expect(state.tabs).toHaveLength(2);
    expect(state.activeLocation?.id).toBe("staging-box");

    await state.closeTab(state.activeTabId);
    expect(state.tabs).toHaveLength(1);
    expect(state.activeLocation?.id).toBe("documents");
  });

  it("opens folders and navigates with Up, Back, and Forward", async () => {
    const state = await initializedState();
    const projects = state.entries.find(({ name }) => name === "Projects");
    expect(projects?.directory).not.toBeNull();

    await state.openEntry(projects!.reference.id);
    expect(state.activeDirectory?.name).toBe("Projects");
    expect(state.canGoBack).toBe(true);
    expect(state.canGoUp).toBe(true);

    await state.goBack();
    expect(state.activeDirectory?.name).toBe("Home");
    expect(state.canGoForward).toBe(true);

    await state.goForward();
    expect(state.activeDirectory?.name).toBe("Projects");

    await state.goUp();
    expect(state.activeDirectory?.name).toBe("Home");
  });

  it("keeps the current directory visible when navigation fails", async () => {
    const state = await initializedState();
    const originalEntries = state.entries;

    await state.openDirectory({
      id: "missing",
      locationId: "home",
      name: "Missing",
      displayPath: "Home/Missing",
    });

    expect(state.activeDirectory?.name).toBe("Home");
    expect(state.entries).toEqual(originalEntries);
    expect(state.errorMessage).toContain("Unknown demo directory");
  });

  it("ignores stale directory events after newer navigation wins", async () => {
    const state = new ExplorerState(new StaleResultDataSource());
    await state.initialize();
    const projects = state.entries.find(({ name }) => name === "Projects")!;
    const photos = state.entries.find(({ name }) => name === "Photos")!;

    const staleNavigation = state.openDirectory(projects.directory!);
    await state.openDirectory(photos.directory!);
    await staleNavigation;

    expect(state.activeDirectory?.name).toBe("Photos");
    expect(state.breadcrumbs.at(-1)?.label).toBe("Photos");
  });

  it("loads and navigates previews from the keyboard selection model", async () => {
    const state = await initializedState();
    const firstEntry = state.visibleEntries.find(
      ({ name }) => name === "explora-notes.md",
    )!;

    state.selectEntry(firstEntry.reference.id);
    await state.openPreview();
    expect(state.preview?.entryId).toBe(firstEntry.reference.id);

    state.moveSelection(1);
    expect(state.selectedEntryId).not.toBe(firstEntry.reference.id);

    state.closePreview();
    expect(state.previewOpen).toBe(false);
  });

  it("disposes stale and closed preview resources", async () => {
    const dataSource = new StalePreviewDataSource();
    const state = new ExplorerState(dataSource);
    await state.initialize();
    const first = state.entries.find(
      ({ name }) => name === "explora-notes.md",
    )!;
    const second = state.entries.find(
      ({ name }) => name === "summer-light.jpg",
    )!;

    state.selectEntry(first.reference.id);
    const stalePreview = state.openPreview();
    state.selectEntry(second.reference.id);
    await state.openPreview();
    await stalePreview;

    expect(state.preview?.entryId).toBe(second.reference.id);
    expect(dataSource.disposedEntryIds).toContain(first.reference.id);

    state.closePreview();
    expect(dataSource.disposedEntryIds).toContain(second.reference.id);
  });

  it("uses direct image rendering by default and reloads after explicit sanitizing", async () => {
    const dataSource = new StalePreviewDataSource();
    const state = new ExplorerState(dataSource);
    await state.initialize();
    const image = state.entries.find(
      ({ name }) => name === "summer-light.jpg",
    )!;

    state.selectEntry(image.reference.id);
    await state.openPreview();
    expect(dataSource.imageModes.at(-1)).toBe("direct");

    await state.setImagePreviewMode("sanitized");
    expect(state.imagePreviewMode).toBe("sanitized");
    expect(dataSource.imageModes.at(-1)).toBe("sanitized");
  });

  it("keeps SSH content previews metadata-only", async () => {
    const state = await initializedState();
    await state.selectLocation("staging-box");
    const remoteFile = state.entries.find(({ name }) => name === "README.md")!;

    state.selectEntry(remoteFile.reference.id);
    await state.openPreview();

    expect(state.preview?.content).toMatchObject({
      type: "metadata",
      reason: "remote",
      requestContent: null,
    });
  });

  it("requests online-only content explicitly and reopens the bounded preview", async () => {
    const state = await initializedState();
    await state.selectLocation("synced:icloud");
    const onlineOnly = state.entries.find(
      ({ name }) => name === "Reference library.pdf",
    )!;

    state.selectEntry(onlineOnly.reference.id);
    await state.openPreview();
    expect(state.preview?.content).toMatchObject({
      type: "metadata",
      reason: "downloadRequired",
      requestContent: {
        intent: "downloadToPreview",
        providerWorkCancellable: false,
      },
    });

    const request = state.requestPreviewContent();
    expect(state.previewContentRequest).not.toBeNull();
    await request;

    expect(state.previewContentRequest).toBeNull();
    expect(state.preview?.content.type).toBe("pdf");
  });

  it("stops waiting without claiming that provider-owned work was cancelled", async () => {
    const dataSource = new SlowContentDataSource();
    const state = new ExplorerState(dataSource);
    await state.initialize();
    await state.selectLocation("synced:icloud");
    const onlineOnly = state.entries.find(
      ({ name }) => name === "Reference library.pdf",
    )!;
    state.selectEntry(onlineOnly.reference.id);
    await state.openPreview();

    const request = state.requestPreviewContent();
    state.stopWaitingForPreviewContent();
    await request;

    expect(dataSource.aborted).toBe(true);
    expect(state.previewContentRequest).toBeNull();
    expect(state.previewContentRequestMessage).toContain(
      "operating system may continue",
    );
  });

  it("connects SSH targets as locations and preserves their tab when disconnected", async () => {
    const state = await initializedState();

    expect(state.sshTargets.map(({ name }) => name)).toEqual([
      "staging-box",
      "render-node",
    ]);
    await state.selectSshTarget("demo:render-node");

    expect(state.activeLocation?.name).toBe("render-node");
    expect(
      state.sshTargets.find(({ id }) => id === "demo:render-node")?.status,
    ).toBe("connected");
    const remoteTabId = state.activeTabId;

    await state.disconnectSshTarget("demo:render-node");

    expect(state.activeTabId).toBe(remoteTabId);
    expect(state.activeLocation?.status).toBe("offline");
    expect(state.activeDirectory?.name).toBe("render-node");
  });

  it("marks dropped SSH tabs offline and reconnects their current directory", async () => {
    const dataSource = new ObservableSshDataSource();
    const state = new ExplorerState(dataSource);
    await state.initialize();
    await state.selectSshTarget("demo:render-node");
    const directory = state.activeDirectory!;
    const tabId = state.activeTabId;

    dataSource.emitSshEvent({
      event: "disconnected",
      targetId: "demo:render-node",
      message: "The SSH connection was lost. Reconnect to continue browsing.",
    });

    expect(state.activeTabId).toBe(tabId);
    expect(state.activeDirectory).toEqual(directory);
    expect(state.activeLocation?.status).toBe("offline");
    expect(state.warningMessage).toContain("connection was lost");

    await state.reconnectActiveSshLocation();

    expect(state.activeTabId).toBe(tabId);
    expect(state.activeDirectory?.id).toBe(directory.id);
    expect(state.activeLocation?.status).toBe("connected");
  });

  it("refreshes the active directory in place without changing history", async () => {
    const dataSource = new ObservableSshDataSource();
    const state = new ExplorerState(dataSource);
    await state.initialize();
    const history = [...state.activeTab!.history];
    const initialListings = dataSource.listingCount;

    await state.refreshDirectory();

    expect(dataSource.listingCount).toBe(initialListings + 1);
    expect(state.activeTab?.history).toEqual(history);
    expect(state.activeTab?.historyIndex).toBe(0);
  });
});
