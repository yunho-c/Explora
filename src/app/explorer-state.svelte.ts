import type {
  BreadcrumbSegment,
  DirectoryRef,
  ExplorerTab,
  FileEntrySummary,
  LocationSummary,
  ManualSshTargetInput,
  PreviewSummary,
  SshConnectionEvent,
  SshPromptResponse,
  SshTargetSummary,
  SortColumn,
  SortDescriptor,
  ViewMode,
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

type SshPromptEvent = Exclude<SshConnectionEvent, { event: "state" }>;

export interface PendingSshPrompt {
  targetId: string;
  event: SshPromptEvent;
  respond: (response: SshPromptResponse) => Promise<void>;
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
  previewOpen = $state(false);
  previewLoading = $state(false);
  preview = $state<PreviewSummary | null>(null);
  sidebarCollapsed = $state(false);
  favoriteRoles = $state<FavoriteRole[]>([...DEFAULT_FAVORITE_ROLES]);
  editingFavorites = $state(false);
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
  private sshConnectionController: AbortController | null = null;
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
        await this.removeConnectedLocation(previous?.connectedLocationId);
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
      await this.selectLocation(location.id);
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
      await this.removeConnectedLocation(target.connectedLocationId);
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
      await this.removeConnectedLocation(target.connectedLocationId);
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
    this.previewController?.abort();
    const controller = new AbortController();
    this.previewController = controller;

    try {
      const preview = await this.dataSource.getPreview(
        entry,
        controller.signal,
      );
      if (this.previewController === controller) this.preview = preview;
    } catch (error) {
      if (!isAbortError(error)) {
        this.preview = {
          entryId: entry.reference.id,
          kind: entry.contentKind,
          title: entry.name,
          subtitle: "Preview unavailable",
          details: [],
        };
      }
    } finally {
      if (this.previewController === controller) this.previewLoading = false;
    }
  }

  closePreview(): void {
    this.previewController?.abort();
    this.previewOpen = false;
    this.previewLoading = false;
    this.preview = null;
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
