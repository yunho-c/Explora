import type {
  BreadcrumbSegment,
  DirectoryRef,
  ExplorerTab,
  FileEntrySummary,
  LocationSummary,
  PreviewSummary,
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

export class ExplorerState {
  locations = $state<LocationSummary[]>([]);
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
  sidebarCollapsed = $state(false);
  mobileSidebarOpen = $state(false);

  private directoryController: AbortController | null = null;
  private previewController: AbortController | null = null;
  private tabSequence = 0;

  constructor(private readonly dataSource: ExplorerDataSource) {}

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
      this.locations = [
        ...(await this.dataSource.listLocations(controller.signal)),
      ];
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
