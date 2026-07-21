import type {
  BreadcrumbSegment,
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
  ViewMode,
} from "$lib/contracts/explorer";
import type { ExplorerDataSource } from "$lib/data/explorer-data-source";
import { compareFileSizes } from "$lib/file-metadata";

const nameCollator = new Intl.Collator(undefined, {
  numeric: true,
  sensitivity: "base",
});

const isAbortError = (error: unknown) =>
  error instanceof Error && error.name === "AbortError";

type SshPromptEvent = Extract<
  SshConnectionEvent,
  { event: "hostKeyPrompt" | "authenticationPrompt" }
>;

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
  previewOpen = $state(false);
  previewLoading = $state(false);
  preview = $state<PreviewSummary | null>(null);
  imagePreviewMode = $state<ImagePreviewMode>("direct");
  sidebarCollapsed = $state(false);
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
  private previewDisposer: (() => void) | null = null;
  private sshConnectionController: AbortController | null = null;
  private tabSequence = 0;

  constructor(private readonly dataSource: ExplorerDataSource) {}

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

  async initialize(): Promise<void> {
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
    this.sort = {
      column,
      direction:
        this.sort.column === column && this.sort.direction === "ascending"
          ? "descending"
          : "ascending",
    };
  }

  setViewMode(viewMode: ViewMode): void {
    this.viewMode = viewMode;
  }

  selectEntry(entryId: string): void {
    this.selectedEntryId = entryId;
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
    this.disposePreview();
    this.previewOpen = false;
    this.previewLoading = false;
    this.preview = null;
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
