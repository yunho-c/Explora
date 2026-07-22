import type {
  BreadcrumbSegment,
  ContentAvailability,
  DirectoryRef,
  ExplorerTab,
  FileEntrySummary,
  ImagePreviewMode,
  LocationSummary,
  ManualSshTargetInput,
  PreviewSummary,
  SshConnectionEvent,
  SshPromptResponse,
  SshTargetSummary,
  SortColumn,
  SortDescriptor,
  SyncedFolderSnapshot,
  ViewMode,
  VolumeSnapshot,
} from "$lib/contracts/explorer";
import type { ExplorerDataSource } from "$lib/data/explorer-data-source";
import { MemoryPreferencesDataSource } from "$lib/data/memory-preferences-data-source";
import type { PreferencesDataSource } from "$lib/data/preferences-data-source";
import {
  DEFAULT_FAVORITE_ROLES,
  isFavoriteRole,
  type FavoriteRole,
  type LayoutPreferencesPatch,
} from "$lib/contracts/preferences";
import { compareFileSizes } from "$lib/file-metadata";

const nameCollator = new Intl.Collator(undefined, {
  numeric: true,
  sensitivity: "base",
});

const isAbortError = (error: unknown) =>
  error instanceof Error && error.name === "AbortError";

const PREFERENCES_LOAD_TIMEOUT_MS = 2_000;

const withTimeout = <T>(promise: Promise<T>, timeoutMs: number): Promise<T> =>
  new Promise((resolve, reject) => {
    const timeoutId = window.setTimeout(
      () =>
        reject(new Error("Explora timed out while loading saved preferences.")),
      timeoutMs,
    );
    promise.then(
      (value) => {
        window.clearTimeout(timeoutId);
        resolve(value);
      },
      (error: unknown) => {
        window.clearTimeout(timeoutId);
        reject(error instanceof Error ? error : new Error(String(error)));
      },
    );
  });

type SshPromptEvent = Extract<
  SshConnectionEvent,
  { event: "hostKeyPrompt" | "authenticationPrompt" }
>;

export interface PendingSshPrompt {
  targetId: string;
  event: SshPromptEvent;
  respond: (response: SshPromptResponse) => Promise<void>;
}

export interface PreviewContentRequestState {
  availability: ContentAvailability;
  providerWorkCancellable: boolean;
}

export class ExplorerState {
  locations = $state<LocationSummary[]>([]);
  sshTargets = $state<SshTargetSummary[]>([]);
  tabs = $state<ExplorerTab[]>([]);
  activeTabId = $state("");
  entries = $state<FileEntrySummary[]>([]);
  breadcrumbs = $state<BreadcrumbSegment[]>([]);
  parentDirectory = $state<DirectoryRef | null>(null);
  selectedEntryId = $state<string | null>(null);
  searchQuery = $state("");
  viewMode = $state<ViewMode>("list");
  sort = $state<SortDescriptor>({ column: "name", direction: "ascending" });
  loading = $state(false);
  errorMessage = $state<string | null>(null);
  warningMessage = $state<string | null>(null);
  preferencesWarningMessage = $state<string | null>(null);
  volumeWarningMessage = $state<string | null>(null);
  syncedFolderWarningMessage = $state<string | null>(null);
  syncedFolderConfigurationError = $state<string | null>(null);
  previewOpen = $state(false);
  previewLoading = $state(false);
  preview = $state<PreviewSummary | null>(null);
  previewContentRequest = $state<PreviewContentRequestState | null>(null);
  previewContentRequestMessage = $state<string | null>(null);
  imagePreviewMode = $state<ImagePreviewMode>("direct");
  sidebarCollapsed = $state(false);
  favoriteRoles = $state<FavoriteRole[]>([...DEFAULT_FAVORITE_ROLES]);
  hiddenSyncedFolderIds = $state<string[]>([]);
  hiddenSshTargetIds = $state<string[]>([]);
  editingFavorites = $state(false);
  editingSyncedFolders = $state(false);
  canAddSyncedFolder = $state(false);
  syncedFolderSaving = $state(false);
  editingSshTargets = $state(false);
  mobileSidebarOpen = $state(false);
  sshTargetDialogOpen = $state(false);
  editingSshTargetId = $state<string | null>(null);
  sshTargetSaving = $state(false);
  sshErrorMessage = $state<string | null>(null);
  pendingSshPrompt = $state<PendingSshPrompt | null>(null);
  connectingTargetId = $state<string | null>(null);
  sshConnectionMessage = $state<string | null>(null);

  private directoryController: AbortController | null = null;
  private previewController: AbortController | null = null;
  private previewContentRequestController: AbortController | null = null;
  private previewDisposer: (() => void) | null = null;
  private sshConnectionController: AbortController | null = null;
  private volumeWatchController: AbortController | null = null;
  private syncedFolderWatchController: AbortController | null = null;
  private syncedFolderMutationController: AbortController | null = null;
  private volumeRevision = -1;
  private syncedFolderRevision = -1;
  private tabSequence = 0;
  private preferencesInitialization: Promise<void> | null = null;
  private preferenceWriteQueue: Promise<void> = Promise.resolve();

  constructor(
    private readonly dataSource: ExplorerDataSource,
    private readonly preferencesDataSource: PreferencesDataSource = new MemoryPreferencesDataSource(),
  ) {}

  get activeTab(): ExplorerTab | undefined {
    return this.tabs.find(({ id }) => id === this.activeTabId);
  }

  get activeLocation(): LocationSummary | undefined {
    const locationId = this.activeTab?.locationId;
    return this.locations.find(({ id }) => id === locationId);
  }

  get activeSshTarget(): SshTargetSummary | undefined {
    const locationId = this.activeTab?.locationId;
    return this.sshTargets.find((target) => target.locationId === locationId);
  }

  get activeSshLocationOffline(): boolean {
    const target = this.activeSshTarget;
    return Boolean(target && target.status !== "connected");
  }

  get activeSyncedFolderOffline(): boolean {
    return Boolean(
      this.activeLocation?.kind === "syncedFolder" &&
      this.activeLocation.status === "offline",
    );
  }

  get activeDirectory(): DirectoryRef | undefined {
    return this.activeTab?.directory;
  }

  get selectedEntry(): FileEntrySummary | undefined {
    return this.entries.find(
      ({ reference }) => reference.id === this.selectedEntryId,
    );
  }

  get editingSshTarget(): SshTargetSummary | undefined {
    const id = this.editingSshTargetId;
    return this.sshTargets.find((target) => target.id === id);
  }

  get canGoBack(): boolean {
    return (this.activeTab?.historyIndex ?? 0) > 0;
  }

  get canGoForward(): boolean {
    const tab = this.activeTab;
    return Boolean(tab && tab.historyIndex < tab.history.length - 1);
  }

  get canGoUp(): boolean {
    return this.parentDirectory !== null;
  }

  get visibleEntries(): FileEntrySummary[] {
    const query = this.searchQuery.trim().toLocaleLowerCase();
    const filtered = query
      ? this.entries.filter((entry) =>
          entry.name.toLocaleLowerCase().includes(query),
        )
      : [...this.entries];
    const direction = this.sort.direction === "ascending" ? 1 : -1;

    return filtered.sort((left, right) => {
      const leftIsDirectory = left.directory !== null;
      const rightIsDirectory = right.directory !== null;
      if (leftIsDirectory !== rightIsDirectory) {
        return leftIsDirectory ? -1 : 1;
      }

      switch (this.sort.column) {
        case "modifiedAt":
          return (
            ((left.modifiedAt ?? -1) - (right.modifiedAt ?? -1)) * direction
          );
        case "size":
          return compareFileSizes(left, right) * direction;
        default:
          return nameCollator.compare(left.name, right.name) * direction;
      }
    });
  }

  get availableFavoriteLocations(): LocationSummary[] {
    return this.locations.filter(
      ({ kind, role }) => kind === "local" && isFavoriteRole(role),
    );
  }

  get visibleFavoriteLocations(): LocationSummary[] {
    return this.availableFavoriteLocations.filter(({ role }) =>
      this.favoriteRoles.includes(role as FavoriteRole),
    );
  }

  get syncedFolderLocations(): LocationSummary[] {
    return this.locations.filter(({ kind }) => kind === "syncedFolder");
  }

  get visibleSyncedFolderLocations(): LocationSummary[] {
    return this.syncedFolderLocations.filter(
      ({ id }) => !this.hiddenSyncedFolderIds.includes(id),
    );
  }

  get visibleSshTargets(): SshTargetSummary[] {
    return this.sshTargets.filter(
      ({ id }) => !this.hiddenSshTargetIds.includes(id),
    );
  }

  async initialize(): Promise<void> {
    await this.initializePreferences();
    const controller = new AbortController();

    try {
      const [locations, sshTargets] = await Promise.all([
        this.dataSource.listLocations(controller.signal),
        this.dataSource.listSshTargets(controller.signal).catch((error) => {
          this.sshErrorMessage =
            error instanceof Error
              ? error.message
              : "Explora could not load SSH targets.";
          return [];
        }),
      ]);
      this.locations = [...locations];
      this.sshTargets = [...sshTargets];
      this.startVolumeWatch();
      this.startSyncedFolderWatch();
      const initialLocation = this.locations[0];

      if (!initialLocation) {
        this.errorMessage = "Explora could not find an available location.";
        return;
      }

      const tab = this.createTab(initialLocation.id, initialLocation.root);
      this.tabs = [tab];
      this.activeTabId = tab.id;
      await this.loadDirectory(initialLocation.root, (directory) => {
        tab.directory = directory;
        tab.history = [directory];
      });
    } catch (error) {
      if (!isAbortError(error)) {
        this.errorMessage =
          error instanceof Error
            ? error.message
            : "Explora could not load its locations.";
      }
    }
  }

  dispose(): void {
    this.directoryController?.abort();
    this.previewController?.abort();
    this.sshConnectionController?.abort();
    this.volumeWatchController?.abort();
    this.syncedFolderWatchController?.abort();
    this.syncedFolderMutationController?.abort();
  }

  private startVolumeWatch(): void {
    this.volumeWatchController?.abort();
    const controller = new AbortController();
    this.volumeWatchController = controller;
    void this.dataSource
      .watchVolumes({
        signal: controller.signal,
        onSnapshot: (snapshot) => void this.applyVolumeSnapshot(snapshot),
      })
      .catch((error: unknown) => {
        if (isAbortError(error) || this.volumeWatchController !== controller)
          return;
        this.volumeWarningMessage =
          error instanceof Error
            ? error.message
            : "Explora could not watch mounted volumes.";
      });
  }

  private async applyVolumeSnapshot(snapshot: VolumeSnapshot): Promise<void> {
    if (snapshot.revision <= this.volumeRevision) return;
    this.volumeRevision = snapshot.revision;
    this.volumeWarningMessage = snapshot.warning;

    const previousVolumes = this.locations.filter(
      ({ kind }) => kind === "volume",
    );
    const offlineVolumes = previousVolumes
      .filter(
        ({ id }) =>
          !snapshot.volumes.some((volume) => volume.id === id) &&
          this.tabs.some(({ locationId }) => locationId === id),
      )
      .map((location) => ({
        ...location,
        status: "offline" as const,
        detail: "Volume unavailable",
      }));
    this.locations = [
      ...this.locations.filter(({ kind }) => kind !== "volume"),
      ...snapshot.volumes,
      ...offlineVolumes,
    ];

    const activeTab = this.activeTab;
    const removedActiveVolume = activeTab
      ? offlineVolumes.find(({ id }) => id === activeTab.locationId)
      : undefined;
    if (removedActiveVolume) {
      this.directoryController?.abort();
      this.previewController?.abort();
      this.loading = false;
      this.warningMessage = `“${removedActiveVolume.name}” is no longer available. Reconnect the volume to continue.`;
    }

    const restoredIds = snapshot.volumes
      .filter(
        ({ id }) =>
          previousVolumes.find((location) => location.id === id)?.status ===
          "offline",
      )
      .map(({ id }) => id);
    if (restoredIds.length === 0) return;

    for (const tab of this.tabs) {
      if (!restoredIds.includes(tab.locationId)) continue;
      const volume = snapshot.volumes.find(({ id }) => id === tab.locationId);
      if (!volume) continue;
      tab.directory = volume.root;
      tab.history = [volume.root];
      tab.historyIndex = 0;
      tab.title = volume.name;
    }
    if (activeTab && restoredIds.includes(activeTab.locationId)) {
      const volume = snapshot.volumes.find(
        ({ id }) => id === activeTab.locationId,
      );
      if (volume) {
        this.warningMessage = null;
        await this.loadDirectory(volume.root, (directory) => {
          activeTab.directory = directory;
          activeTab.history = [directory];
          activeTab.historyIndex = 0;
          activeTab.title = directory.name;
        });
      }
    }
  }

  private startSyncedFolderWatch(): void {
    this.syncedFolderWatchController?.abort();
    const controller = new AbortController();
    this.syncedFolderWatchController = controller;
    void this.dataSource
      .watchSyncedFolders({
        signal: controller.signal,
        onSnapshot: (snapshot) => void this.applySyncedFolderSnapshot(snapshot),
      })
      .catch((error: unknown) => {
        if (
          isAbortError(error) ||
          this.syncedFolderWatchController !== controller
        )
          return;
        this.syncedFolderWarningMessage =
          error instanceof Error
            ? error.message
            : "Explora could not watch synced folders.";
      });
  }

  private async applySyncedFolderSnapshot(
    snapshot: SyncedFolderSnapshot,
  ): Promise<void> {
    if (snapshot.revision <= this.syncedFolderRevision) return;
    this.syncedFolderRevision = snapshot.revision;
    this.syncedFolderWarningMessage = snapshot.warning;
    this.canAddSyncedFolder = snapshot.canAddFolder;

    const previousFolders = this.locations.filter(
      ({ kind }) => kind === "syncedFolder",
    );
    const offlineFolders = previousFolders
      .filter(
        ({ id }) =>
          !snapshot.folders.some((folder) => folder.id === id) &&
          this.tabs.some(({ locationId }) => locationId === id),
      )
      .map((location) => ({
        ...location,
        status: "offline" as const,
        detail: "Synced folder unavailable",
        syncedFolder: location.syncedFolder
          ? { ...location.syncedFolder, status: "offline" as const }
          : null,
      }));
    this.locations = [
      ...this.locations.filter(({ kind }) => kind !== "syncedFolder"),
      ...snapshot.folders,
      ...offlineFolders,
    ];

    const activeTab = this.activeTab;
    const removedActiveFolder = activeTab
      ? offlineFolders.find(({ id }) => id === activeTab.locationId)
      : undefined;
    const transitionedOfflineFolder = activeTab
      ? snapshot.folders.find(
          ({ id, status }) =>
            id === activeTab.locationId &&
            status === "offline" &&
            previousFolders.find((folder) => folder.id === id)?.status !==
              "offline",
        )
      : undefined;
    const unavailableActiveFolder =
      removedActiveFolder ?? transitionedOfflineFolder;
    if (unavailableActiveFolder) {
      this.directoryController?.abort();
      this.previewController?.abort();
      this.loading = false;
      this.warningMessage =
        unavailableActiveFolder.syncedFolder?.source === "manual"
          ? `“${unavailableActiveFolder.name}” is no longer available. Restore the folder or add it again to continue.`
          : `“${unavailableActiveFolder.name}” is no longer available. Restart its sync provider to continue.`;
    }

    const restoredIds = snapshot.folders
      .filter(
        ({ id, status }) =>
          status === "available" &&
          previousFolders.find((location) => location.id === id)?.status ===
            "offline",
      )
      .map(({ id }) => id);
    if (restoredIds.length === 0) return;

    for (const tab of this.tabs) {
      if (!restoredIds.includes(tab.locationId)) continue;
      const folder = snapshot.folders.find(({ id }) => id === tab.locationId);
      if (!folder) continue;
      tab.directory = folder.root;
      tab.history = [folder.root];
      tab.historyIndex = 0;
      tab.title = folder.name;
    }
    if (activeTab && restoredIds.includes(activeTab.locationId)) {
      const folder = snapshot.folders.find(
        ({ id }) => id === activeTab.locationId,
      );
      if (folder) {
        this.warningMessage = null;
        await this.loadDirectory(folder.root, (directory) => {
          activeTab.directory = directory;
          activeTab.history = [directory];
          activeTab.historyIndex = 0;
          activeTab.title = directory.name;
        });
      }
    }
  }

  initializePreferences(): Promise<void> {
    this.preferencesInitialization ??= this.loadPreferences();
    return this.preferencesInitialization;
  }

  private async loadPreferences(): Promise<void> {
    try {
      const snapshot = await withTimeout(
        this.preferencesDataSource.getPreferences(),
        PREFERENCES_LOAD_TIMEOUT_MS,
      );
      this.sidebarCollapsed = snapshot.preferences.layout.sidebarCollapsed;
      this.viewMode = snapshot.preferences.layout.viewMode;
      this.sort = { ...snapshot.preferences.layout.sort };
      this.favoriteRoles = [...snapshot.preferences.layout.favoriteRoles];
      this.hiddenSyncedFolderIds = [
        ...snapshot.preferences.layout.hiddenSyncedFolderIds,
      ];
      this.hiddenSshTargetIds = [
        ...snapshot.preferences.layout.hiddenSshTargetIds,
      ];
      this.preferencesWarningMessage = snapshot.warning?.message ?? null;
    } catch (error) {
      this.preferencesWarningMessage =
        error instanceof Error
          ? error.message
          : "Explora could not load saved preferences and used defaults instead.";
    }
  }

  openNewSshTarget(): void {
    this.editingSshTargetId = null;
    this.sshTargetDialogOpen = true;
  }

  openEditSshTarget(targetId: string): void {
    const target = this.sshTargets.find(({ id }) => id === targetId);
    if (!target?.editable || !target.configuration) return;
    this.editingSshTargetId = targetId;
    this.sshTargetDialogOpen = true;
  }

  closeSshTargetDialog(): void {
    if (this.sshTargetSaving) return;
    this.sshTargetDialogOpen = false;
    this.editingSshTargetId = null;
  }

  async saveSshTarget(input: ManualSshTargetInput): Promise<boolean> {
    const controller = new AbortController();
    this.sshTargetSaving = true;
    this.sshErrorMessage = null;
    try {
      const saved = this.editingSshTargetId
        ? await this.dataSource.updateSshTarget(
            this.editingSshTargetId,
            input,
            controller.signal,
          )
        : await this.dataSource.createSshTarget(input, controller.signal);
      if (this.editingSshTargetId) {
        const previous = this.sshTargets.find(
          ({ id }) => id === this.editingSshTargetId,
        );
        await this.removeConnectedLocation(previous?.locationId);
        this.sshTargets = this.sshTargets.map((target) =>
          target.id === saved.id ? saved : target,
        );
      } else {
        this.sshTargets = [...this.sshTargets, saved];
      }
      this.sshTargetDialogOpen = false;
      this.editingSshTargetId = null;
      return true;
    } catch (error) {
      if (!isAbortError(error)) {
        this.sshErrorMessage =
          error instanceof Error
            ? error.message
            : "The SSH target was not saved.";
      }
      return false;
    } finally {
      this.sshTargetSaving = false;
    }
  }

  async selectSshTarget(targetId: string): Promise<void> {
    const target = this.sshTargets.find(({ id }) => id === targetId);
    if (!target) return;
    const location = target.connectedLocationId
      ? this.locations.find(({ id }) => id === target.connectedLocationId)
      : undefined;
    if (location) {
      await this.selectLocation(location.id);
      return;
    }
    await this.connectSshTarget(targetId);
  }

  async connectSshTarget(targetId: string): Promise<void> {
    if (this.connectingTargetId) return;
    const target = this.sshTargets.find(({ id }) => id === targetId);
    if (!target) return;

    const controller = new AbortController();
    this.sshConnectionController = controller;
    this.connectingTargetId = targetId;
    this.sshConnectionMessage = `Connecting to ${target.name}…`;
    this.sshErrorMessage = null;
    this.setSshTargetStatus(targetId, "connecting", null);
    try {
      const location = await this.dataSource.connectSshTarget(targetId, {
        signal: controller.signal,
        onEvent: (event, respond) => {
          if (event.event === "disconnected") {
            this.handleUnexpectedSshDisconnect(event.targetId, event.message);
            return;
          }
          if (this.sshConnectionController !== controller) return;
          if (event.event === "state") {
            this.sshConnectionMessage =
              event.state === "authenticating"
                ? `Authenticating with ${target.name}…`
                : event.state === "openingSftp"
                  ? `Opening ${target.name} in Explora…`
                  : event.state === "connected"
                    ? `Connected to ${target.name}.`
                    : `Connecting to ${target.name}…`;
          } else {
            this.pendingSshPrompt = { targetId, event, respond };
          }
        },
      });
      if (this.sshConnectionController !== controller) return;
      this.locations = [
        ...this.locations.filter(({ id }) => id !== location.id),
        location,
      ];
      this.setSshTargetStatus(targetId, "connected", location.id);
      this.pendingSshPrompt = null;
      const preservedTab =
        this.activeTab?.locationId === location.id
          ? this.activeTab
          : this.tabs.find((tab) => tab.locationId === location.id);
      if (preservedTab) {
        this.activeTabId = preservedTab.id;
        const restored = await this.loadDirectory(
          preservedTab.directory,
          (directory) => {
            preservedTab.directory = directory;
            preservedTab.history[preservedTab.historyIndex] = directory;
            preservedTab.title = directory.name;
          },
        );
        if (!restored) {
          await this.loadDirectory(location.root, (directory) => {
            preservedTab.directory = directory;
            preservedTab.history = [directory];
            preservedTab.historyIndex = 0;
            preservedTab.title = directory.name;
          });
        }
      } else {
        await this.selectLocation(location.id);
      }
    } catch (error) {
      if (!isAbortError(error) && this.sshConnectionController === controller) {
        this.setSshTargetStatus(targetId, "error", null);
        this.sshErrorMessage =
          error instanceof Error
            ? error.message
            : "Explora could not connect to the SSH target.";
      } else if (this.sshConnectionController === controller) {
        this.setSshTargetStatus(targetId, "disconnected", null);
      }
    } finally {
      if (this.sshConnectionController === controller) {
        this.sshConnectionController = null;
        this.connectingTargetId = null;
        this.sshConnectionMessage = null;
        this.pendingSshPrompt = null;
      }
    }
  }

  async answerSshPrompt(response: SshPromptResponse): Promise<void> {
    const pending = this.pendingSshPrompt;
    if (!pending) return;
    this.pendingSshPrompt = null;
    try {
      await pending.respond(response);
    } catch (error) {
      this.sshErrorMessage =
        error instanceof Error
          ? error.message
          : "The SSH response was not accepted.";
      this.sshConnectionController?.abort();
    }
  }

  cancelSshConnection(): void {
    this.pendingSshPrompt = null;
    this.sshConnectionController?.abort();
  }

  async disconnectSshTarget(targetId: string): Promise<void> {
    const target = this.sshTargets.find(({ id }) => id === targetId);
    if (!target) return;
    try {
      await this.dataSource.disconnectSshTarget(
        targetId,
        new AbortController().signal,
      );
      await this.removeConnectedLocation(target.locationId);
      this.setSshTargetStatus(targetId, "disconnected", null);
    } catch (error) {
      this.sshErrorMessage =
        error instanceof Error
          ? error.message
          : "The SSH target did not disconnect.";
    }
  }

  async deleteSshTarget(targetId: string): Promise<void> {
    const target = this.sshTargets.find(({ id }) => id === targetId);
    if (!target?.editable) return;
    try {
      await this.dataSource.deleteSshTarget(
        targetId,
        new AbortController().signal,
      );
      await this.removeConnectedLocation(target.locationId);
      this.sshTargets = this.sshTargets.filter(({ id }) => id !== targetId);
    } catch (error) {
      this.sshErrorMessage =
        error instanceof Error
          ? error.message
          : "The SSH target was not removed.";
    }
  }

  private setSshTargetStatus(
    targetId: string,
    status: SshTargetSummary["status"],
    connectedLocationId: string | null,
  ): void {
    this.sshTargets = this.sshTargets.map((target) =>
      target.id === targetId
        ? { ...target, status, connectedLocationId }
        : target,
    );
  }

  private handleUnexpectedSshDisconnect(
    targetId: string,
    message: string,
  ): void {
    const target = this.sshTargets.find(({ id }) => id === targetId);
    if (!target) return;

    this.setSshTargetStatus(targetId, "disconnected", null);
    this.locations = this.locations.map((location) =>
      location.id === target.locationId
        ? {
            ...location,
            status: "offline",
            detail: "SSH · Connection lost",
          }
        : location,
    );
    this.sshErrorMessage = message;
    if (this.activeTab?.locationId === target.locationId) {
      this.directoryController?.abort();
      this.loading = false;
      this.warningMessage = message;
    }
  }

  private async removeConnectedLocation(
    locationId: string | null | undefined,
  ): Promise<void> {
    if (!locationId) return;
    if (this.tabs.some((tab) => tab.locationId === locationId)) {
      this.locations = this.locations.map((location) =>
        location.id === locationId
          ? {
              ...location,
              status: "offline",
              detail: "SSH · Disconnected",
            }
          : location,
      );
      return;
    }
    this.locations = this.locations.filter(({ id }) => id !== locationId);
  }

  async selectLocation(locationId: string): Promise<void> {
    const tab = this.activeTab;
    const location = this.locations.find(({ id }) => id === locationId);
    if (!tab || !location) return;

    await this.loadDirectory(location.root, (directory) => {
      if (tab.directory.id !== directory.id) {
        tab.history = [
          ...tab.history.slice(0, tab.historyIndex + 1),
          directory,
        ];
        tab.historyIndex = tab.history.length - 1;
      }
      tab.locationId = location.id;
      tab.directory = directory;
      tab.title = directory.name;
      this.mobileSidebarOpen = false;
    });
  }

  async openDirectory(directory: DirectoryRef): Promise<void> {
    const tab = this.activeTab;
    if (!tab || tab.directory.id === directory.id) return;

    await this.loadDirectory(directory, (openedDirectory) => {
      tab.history = [
        ...tab.history.slice(0, tab.historyIndex + 1),
        openedDirectory,
      ];
      tab.historyIndex = tab.history.length - 1;
      tab.directory = openedDirectory;
      tab.title = openedDirectory.name;
    });
  }

  async openTab(locationId?: string): Promise<void> {
    const requestedLocation = locationId
      ? this.locations.find(({ id }) => id === locationId)
      : undefined;
    const sourceTab = this.activeTab;
    const directory =
      requestedLocation?.root ??
      sourceTab?.directory ??
      this.locations[0]?.root;
    const tabLocationId =
      requestedLocation?.id ?? sourceTab?.locationId ?? this.locations[0]?.id;
    if (!directory || !tabLocationId) return;

    const tab = this.createTab(tabLocationId, directory);
    this.tabs = [...this.tabs, tab];
    this.activeTabId = tab.id;
    this.clearDirectoryPresentation();
    await this.loadDirectory(directory, (openedDirectory) => {
      tab.directory = openedDirectory;
      tab.history = [openedDirectory];
      tab.title = openedDirectory.name;
    });
  }

  async activateTab(tabId: string): Promise<void> {
    if (tabId === this.activeTabId) return;
    const tab = this.tabs.find(({ id }) => id === tabId);
    if (!tab) return;

    this.activeTabId = tabId;
    this.clearDirectoryPresentation();
    await this.loadDirectory(tab.directory, (openedDirectory) => {
      tab.directory = openedDirectory;
      tab.history[tab.historyIndex] = openedDirectory;
      tab.title = openedDirectory.name;
    });
  }

  async closeTab(tabId: string): Promise<void> {
    if (this.tabs.length === 1) return;

    const closingIndex = this.tabs.findIndex(({ id }) => id === tabId);
    if (closingIndex < 0) return;

    const wasActive = tabId === this.activeTabId;
    this.tabs = this.tabs.filter(({ id }) => id !== tabId);

    if (wasActive) {
      const tab = this.tabs[Math.min(closingIndex, this.tabs.length - 1)];
      this.activeTabId = tab.id;
      this.clearDirectoryPresentation();
      await this.loadDirectory(tab.directory, (openedDirectory) => {
        tab.directory = openedDirectory;
        tab.history[tab.historyIndex] = openedDirectory;
        tab.title = openedDirectory.name;
      });
    }
  }

  async goBack(): Promise<void> {
    const tab = this.activeTab;
    if (!tab || tab.historyIndex <= 0) return;

    const nextIndex = tab.historyIndex - 1;
    await this.loadDirectory(tab.history[nextIndex], (directory) => {
      tab.historyIndex = nextIndex;
      tab.directory = directory;
      tab.history[nextIndex] = directory;
      tab.title = directory.name;
    });
  }

  async goForward(): Promise<void> {
    const tab = this.activeTab;
    if (!tab || tab.historyIndex >= tab.history.length - 1) return;

    const nextIndex = tab.historyIndex + 1;
    await this.loadDirectory(tab.history[nextIndex], (directory) => {
      tab.historyIndex = nextIndex;
      tab.directory = directory;
      tab.history[nextIndex] = directory;
      tab.title = directory.name;
    });
  }

  async goUp(): Promise<void> {
    if (this.parentDirectory) await this.openDirectory(this.parentDirectory);
  }

  async refreshDirectory(): Promise<void> {
    const tab = this.activeTab;
    if (!tab || this.activeSshLocationOffline) return;

    await this.loadDirectory(tab.directory, (directory) => {
      tab.directory = directory;
      tab.history[tab.historyIndex] = directory;
      tab.title = directory.name;
    });
  }

  async reconnectActiveSshLocation(): Promise<void> {
    const target = this.activeSshTarget;
    if (!target || target.status === "connected") return;
    await this.connectSshTarget(target.id);
  }

  async openEntry(entryId: string): Promise<void> {
    const entry = this.entries.find(
      ({ reference }) => reference.id === entryId,
    );
    if (!entry) return;

    this.selectedEntryId = entry.reference.id;
    if (entry.directory) {
      await this.openDirectory(entry.directory);
    } else {
      await this.openPreview(entry.reference.id);
    }
  }

  toggleSort(column: SortColumn): void {
    const sort: SortDescriptor = {
      column,
      direction:
        this.sort.column === column && this.sort.direction === "ascending"
          ? "descending"
          : "ascending",
    };
    this.sort = sort;
    this.persistLayoutPreferences({ sort });
  }

  setViewMode(viewMode: ViewMode): void {
    this.viewMode = viewMode;
    this.persistLayoutPreferences({ viewMode });
  }

  setSidebarCollapsed(sidebarCollapsed: boolean): void {
    this.sidebarCollapsed = sidebarCollapsed;
    this.persistLayoutPreferences({ sidebarCollapsed });
  }

  setFavoriteVisible(role: FavoriteRole, visible: boolean): void {
    const favoriteRoles = DEFAULT_FAVORITE_ROLES.filter((candidate) =>
      candidate === role ? visible : this.favoriteRoles.includes(candidate),
    );
    this.favoriteRoles = [...favoriteRoles];
    this.persistLayoutPreferences({ favoriteRoles: [...favoriteRoles] });
  }

  setSshTargetVisible(targetId: string, visible: boolean): void {
    const hiddenSshTargetIds = visible
      ? this.hiddenSshTargetIds.filter((id) => id !== targetId)
      : this.hiddenSshTargetIds.includes(targetId)
        ? [...this.hiddenSshTargetIds]
        : [...this.hiddenSshTargetIds, targetId].sort();
    this.hiddenSshTargetIds = hiddenSshTargetIds;
    this.persistLayoutPreferences({ hiddenSshTargetIds });
  }

  setSyncedFolderVisible(folderId: string, visible: boolean): void {
    const hiddenSyncedFolderIds = visible
      ? this.hiddenSyncedFolderIds.filter((id) => id !== folderId)
      : this.hiddenSyncedFolderIds.includes(folderId)
        ? [...this.hiddenSyncedFolderIds]
        : [...this.hiddenSyncedFolderIds, folderId].sort();
    this.hiddenSyncedFolderIds = hiddenSyncedFolderIds;
    this.persistLayoutPreferences({ hiddenSyncedFolderIds });
  }

  async addSyncedFolder(): Promise<void> {
    if (!this.canAddSyncedFolder || this.syncedFolderSaving) return;
    this.syncedFolderMutationController?.abort();
    const controller = new AbortController();
    this.syncedFolderMutationController = controller;
    this.syncedFolderSaving = true;
    this.syncedFolderConfigurationError = null;
    try {
      const folderId = await this.dataSource.addSyncedFolder(controller.signal);
      if (folderId && this.hiddenSyncedFolderIds.includes(folderId)) {
        this.setSyncedFolderVisible(folderId, true);
      }
    } catch (error) {
      if (!isAbortError(error)) {
        this.syncedFolderConfigurationError =
          error instanceof Error
            ? error.message
            : "Explora could not add the selected synced folder.";
      }
    } finally {
      if (this.syncedFolderMutationController === controller) {
        this.syncedFolderSaving = false;
        this.syncedFolderMutationController = null;
      }
    }
  }

  async removeSyncedFolder(folderId: string): Promise<void> {
    const folder = this.syncedFolderLocations.find(({ id }) => id === folderId);
    if (
      !folder ||
      folder.syncedFolder?.source !== "manual" ||
      this.syncedFolderSaving
    )
      return;
    this.syncedFolderMutationController?.abort();
    const controller = new AbortController();
    this.syncedFolderMutationController = controller;
    this.syncedFolderSaving = true;
    this.syncedFolderConfigurationError = null;
    try {
      await this.dataSource.removeSyncedFolder(folderId, controller.signal);
      this.editingSyncedFolders = false;
      if (this.hiddenSyncedFolderIds.includes(folderId)) {
        this.setSyncedFolderVisible(folderId, true);
      }
    } catch (error) {
      if (!isAbortError(error)) {
        this.syncedFolderConfigurationError =
          error instanceof Error
            ? error.message
            : "Explora could not remove the synced folder.";
      }
    } finally {
      if (this.syncedFolderMutationController === controller) {
        this.syncedFolderSaving = false;
        this.syncedFolderMutationController = null;
      }
    }
  }

  selectEntry(entryId: string): void {
    this.selectedEntryId = entryId;
  }

  private persistLayoutPreferences(patch: LayoutPreferencesPatch): void {
    this.preferenceWriteQueue = this.preferenceWriteQueue.then(async () => {
      try {
        await this.preferencesDataSource.updatePreferences({ layout: patch });
      } catch (error) {
        this.preferencesWarningMessage =
          error instanceof Error
            ? error.message
            : "Explora could not save the latest preference change.";
      }
    });
  }

  async openPreview(entryId = this.selectedEntryId): Promise<void> {
    const entry = this.entries.find(
      ({ reference }) => reference.id === entryId,
    );
    if (!entry) return;

    this.selectedEntryId = entry.reference.id;
    this.previewOpen = true;
    this.previewLoading = true;
    this.preview = null;
    this.previewContentRequestController?.abort();
    this.previewContentRequestController = null;
    this.previewContentRequest = null;
    this.previewContentRequestMessage = null;
    this.previewController?.abort();
    this.disposePreview();
    const controller = new AbortController();
    this.previewController = controller;

    try {
      const prepared = await this.dataSource.getPreview(entry, {
        signal: controller.signal,
        imageMode: this.imagePreviewMode,
      });
      if (this.previewController === controller) {
        this.preview = prepared.preview;
        this.previewDisposer = prepared.dispose;
      } else {
        prepared.dispose();
      }
    } catch (error) {
      if (!isAbortError(error) && this.previewController === controller) {
        this.preview = {
          entryId: entry.reference.id,
          kind: entry.contentKind,
          title: entry.name,
          accessibilityDescription: entry.displayPath,
          content: {
            type: "metadata",
            reason: "unsupported",
            message:
              error instanceof Error
                ? error.message
                : "Explora could not prepare this preview.",
            requestContent: null,
          },
          details: [],
        };
      }
    } finally {
      if (this.previewController === controller) this.previewLoading = false;
    }
  }

  closePreview(): void {
    this.previewController?.abort();
    this.previewController = null;
    this.previewContentRequestController?.abort();
    this.previewContentRequestController = null;
    this.disposePreview();
    this.previewOpen = false;
    this.previewLoading = false;
    this.preview = null;
    this.previewContentRequest = null;
    this.previewContentRequestMessage = null;
  }

  async requestPreviewContent(): Promise<void> {
    const preview = this.preview;
    if (
      !preview ||
      preview.content.type !== "metadata" ||
      preview.content.reason !== "downloadRequired" ||
      !preview.content.requestContent
    ) {
      return;
    }
    const entry = this.entries.find(
      ({ reference }) => reference.id === preview.entryId,
    );
    if (!entry) return;

    this.previewContentRequestController?.abort();
    const controller = new AbortController();
    this.previewContentRequestController = controller;
    this.previewContentRequestMessage = null;
    this.previewContentRequest = {
      availability: entry.availability,
      providerWorkCancellable:
        preview.content.requestContent.providerWorkCancellable,
    };
    let providerWorkStarted = false;

    try {
      await this.dataSource.requestContent(entry, {
        signal: controller.signal,
        onEvent: (event) => {
          if (this.previewContentRequestController !== controller) return;
          if (event.event === "started") {
            providerWorkStarted = true;
            this.previewContentRequest = {
              availability:
                this.previewContentRequest?.availability ?? entry.availability,
              providerWorkCancellable: event.providerWorkCancellable,
            };
          } else {
            this.previewContentRequest = {
              availability: event.availability,
              providerWorkCancellable:
                this.previewContentRequest?.providerWorkCancellable ?? false,
            };
          }
        },
      });
      if (this.previewContentRequestController !== controller) return;
      this.previewContentRequestController = null;
      this.previewContentRequest = null;
      await this.openPreview(entry.reference.id);
    } catch (error) {
      if (this.previewContentRequestController !== controller) return;
      this.previewContentRequestController = null;
      const providerWorkCancellable =
        this.previewContentRequest?.providerWorkCancellable ?? false;
      this.previewContentRequest = null;
      if (!isAbortError(error)) {
        const message =
          error instanceof Error
            ? error.message
            : "Explora could not request this file's content.";
        this.previewContentRequestMessage =
          providerWorkStarted && !providerWorkCancellable
            ? `${message} The operating system may continue downloading it.`
            : message;
      }
    }
  }

  stopWaitingForPreviewContent(): void {
    const controller = this.previewContentRequestController;
    if (!controller) return;
    const providerWorkCancellable =
      this.previewContentRequest?.providerWorkCancellable ?? false;
    this.previewContentRequestController = null;
    this.previewContentRequest = null;
    this.previewContentRequestMessage = providerWorkCancellable
      ? "Explora requested cancellation. The provider may still finish the download."
      : "Explora stopped waiting. The operating system may continue downloading this file.";
    controller.abort();
  }

  async setImagePreviewMode(mode: ImagePreviewMode): Promise<void> {
    if (this.imagePreviewMode === mode) return;
    this.imagePreviewMode = mode;
    if (this.previewOpen && this.selectedEntry?.contentKind === "image") {
      await this.openPreview(this.selectedEntryId);
    }
  }

  handlePreviewImageFailure(entryId: string): void {
    const preview = this.preview;
    if (preview?.entryId !== entryId || preview.content.type !== "image") {
      return;
    }
    this.disposePreview();
    this.preview = {
      ...preview,
      content: {
        type: "metadata",
        reason: "malformed",
        message: "This image could not be rendered by the system WebView.",
        requestContent: null,
      },
    };
  }

  private disposePreview(): void {
    this.previewDisposer?.();
    this.previewDisposer = null;
  }

  moveSelection(delta: number): void {
    const entries = this.visibleEntries;
    if (entries.length === 0) return;

    const currentIndex = entries.findIndex(
      ({ reference }) => reference.id === this.selectedEntryId,
    );
    const nextIndex =
      currentIndex < 0
        ? 0
        : Math.min(Math.max(currentIndex + delta, 0), entries.length - 1);
    this.selectedEntryId = entries[nextIndex].reference.id;

    if (this.previewOpen) void this.openPreview(this.selectedEntryId);
  }

  private createTab(locationId: string, directory: DirectoryRef): ExplorerTab {
    this.tabSequence += 1;
    return {
      id: `tab-${this.tabSequence}`,
      title: directory.name,
      locationId,
      directory,
      history: [directory],
      historyIndex: 0,
    };
  }

  private clearDirectoryPresentation(): void {
    this.entries = [];
    this.breadcrumbs = [];
    this.parentDirectory = null;
    this.resetTransientState();
  }

  private resetTransientState(): void {
    this.searchQuery = "";
    this.selectedEntryId = null;
    this.closePreview();
  }

  private async loadDirectory(
    target: DirectoryRef,
    commit: (directory: DirectoryRef) => void,
  ): Promise<boolean> {
    const location = this.locations.find(({ id }) => id === target.locationId);
    if (location?.kind === "volume" && location.status === "offline") {
      this.warningMessage = `“${location.name}” is no longer available. Reconnect the volume to continue.`;
      return false;
    }
    if (location?.kind === "syncedFolder" && location.status === "offline") {
      this.warningMessage = `“${location.name}” is no longer available. Restart its sync provider to continue.`;
      return false;
    }
    this.directoryController?.abort();
    const controller = new AbortController();
    this.directoryController = controller;
    this.loading = true;
    this.errorMessage = null;
    this.warningMessage = null;
    let started = false;

    try {
      await this.dataSource.listDirectory(target, {
        signal: controller.signal,
        onStart: ({ directory, parent, breadcrumbs }) => {
          if (this.directoryController !== controller) return;
          started = true;
          commit(directory);
          this.parentDirectory = parent;
          this.breadcrumbs = [...breadcrumbs];
          this.entries = [];
          this.resetTransientState();
        },
        onBatch: ({ entries, replace }) => {
          if (this.directoryController !== controller) return;
          this.entries = replace ? [...entries] : [...this.entries, ...entries];
        },
        onComplete: ({ skippedEntries }) => {
          if (this.directoryController !== controller || skippedEntries === 0)
            return;
          this.warningMessage = `${skippedEntries} ${
            skippedEntries === 1 ? "item was" : "items were"
          } skipped because metadata could not be read.`;
        },
      });
    } catch (error) {
      if (!isAbortError(error) && this.directoryController === controller) {
        this.errorMessage =
          error instanceof Error
            ? error.message
            : "This directory could not be loaded.";
      }
    } finally {
      if (this.directoryController === controller) this.loading = false;
    }

    return started;
  }
}
