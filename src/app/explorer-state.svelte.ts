import type {
  ExplorerTab,
  FileEntrySummary,
  LocationSummary,
  PreviewSummary,
  SortColumn,
  SortDescriptor,
  ViewMode,
} from "$lib/contracts/explorer";
import type { ExplorerDataSource } from "$lib/data/explorer-data-source";

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
  selectedEntryId = $state<string | null>(null);
  searchQuery = $state("");
  viewMode = $state<ViewMode>("list");
  sort = $state<SortDescriptor>({ column: "name", direction: "ascending" });
  loading = $state(false);
  errorMessage = $state<string | null>(null);
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

  get selectedEntry(): FileEntrySummary | undefined {
    return this.entries.find(({ id }) => id === this.selectedEntryId);
  }

  get canGoBack(): boolean {
    return (this.activeTab?.historyIndex ?? 0) > 0;
  }

  get canGoForward(): boolean {
    const tab = this.activeTab;
    return Boolean(tab && tab.historyIndex < tab.history.length - 1);
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
      if (left.kind !== right.kind) {
        return left.kind === "directory" ? -1 : 1;
      }

      switch (this.sort.column) {
        case "modifiedAt":
          return left.modifiedAt.localeCompare(right.modifiedAt) * direction;
        case "size":
          return ((left.size ?? -1) - (right.size ?? -1)) * direction;
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

      if (initialLocation) {
        this.tabs = [this.createTab(initialLocation)];
        this.activeTabId = this.tabs[0].id;
        await this.loadActiveLocation();
      }
    } catch (error) {
      if (!isAbortError(error)) {
        this.errorMessage =
          error instanceof Error
            ? error.message
            : "Explora could not load its demo locations.";
      }
    }
  }

  async selectLocation(locationId: string): Promise<void> {
    const tab = this.activeTab;
    const location = this.locations.find(({ id }) => id === locationId);

    if (!tab || !location) return;

    if (tab.locationId !== locationId) {
      tab.history = [...tab.history.slice(0, tab.historyIndex + 1), locationId];
      tab.historyIndex = tab.history.length - 1;
      tab.locationId = locationId;
      tab.title = location.name;
    }

    this.mobileSidebarOpen = false;
    this.resetTransientState();
    await this.loadActiveLocation();
  }

  async openTab(locationId = this.activeLocation?.id): Promise<void> {
    const location = this.locations.find(({ id }) => id === locationId);
    if (!location) return;

    const tab = this.createTab(location);
    this.tabs = [...this.tabs, tab];
    this.activeTabId = tab.id;
    this.resetTransientState();
    await this.loadActiveLocation();
  }

  async activateTab(tabId: string): Promise<void> {
    if (tabId === this.activeTabId || !this.tabs.some(({ id }) => id === tabId))
      return;

    this.activeTabId = tabId;
    this.resetTransientState();
    await this.loadActiveLocation();
  }

  async closeTab(tabId: string): Promise<void> {
    if (this.tabs.length === 1) return;

    const closingIndex = this.tabs.findIndex(({ id }) => id === tabId);
    if (closingIndex < 0) return;

    const wasActive = tabId === this.activeTabId;
    this.tabs = this.tabs.filter(({ id }) => id !== tabId);

    if (wasActive) {
      this.activeTabId =
        this.tabs[Math.min(closingIndex, this.tabs.length - 1)].id;
      this.resetTransientState();
      await this.loadActiveLocation();
    }
  }

  async goBack(): Promise<void> {
    const tab = this.activeTab;
    if (!tab || tab.historyIndex <= 0) return;

    tab.historyIndex -= 1;
    this.syncTabToHistory(tab);
    await this.loadActiveLocation();
  }

  async goForward(): Promise<void> {
    const tab = this.activeTab;
    if (!tab || tab.historyIndex >= tab.history.length - 1) return;

    tab.historyIndex += 1;
    this.syncTabToHistory(tab);
    await this.loadActiveLocation();
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
    const entry = this.entries.find(({ id }) => id === entryId);
    if (!entry) return;

    this.selectedEntryId = entry.id;
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
          entryId: entry.id,
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
      ({ id }) => id === this.selectedEntryId,
    );
    const nextIndex =
      currentIndex < 0
        ? 0
        : Math.min(Math.max(currentIndex + delta, 0), entries.length - 1);
    this.selectedEntryId = entries[nextIndex].id;

    if (this.previewOpen) void this.openPreview(this.selectedEntryId);
  }

  private createTab(location: LocationSummary): ExplorerTab {
    this.tabSequence += 1;
    return {
      id: `tab-${this.tabSequence}`,
      title: location.name,
      locationId: location.id,
      history: [location.id],
      historyIndex: 0,
    };
  }

  private syncTabToHistory(tab: ExplorerTab): void {
    const locationId = tab.history[tab.historyIndex];
    const location = this.locations.find(({ id }) => id === locationId);
    if (!location) return;

    tab.locationId = location.id;
    tab.title = location.name;
    this.resetTransientState();
  }

  private resetTransientState(): void {
    this.searchQuery = "";
    this.selectedEntryId = null;
    this.closePreview();
  }

  private async loadActiveLocation(): Promise<void> {
    const locationId = this.activeTab?.locationId;
    if (!locationId) return;

    this.directoryController?.abort();
    const controller = new AbortController();
    this.directoryController = controller;
    this.entries = [];
    this.loading = true;
    this.errorMessage = null;

    try {
      await this.dataSource.listDirectory(locationId, {
        signal: controller.signal,
        onBatch: ({ entries, replace }) => {
          if (this.directoryController !== controller) return;
          this.entries = replace ? [...entries] : [...this.entries, ...entries];
        },
      });
    } catch (error) {
      if (!isAbortError(error)) {
        this.errorMessage =
          error instanceof Error
            ? error.message
            : "This location could not be loaded.";
      }
    } finally {
      if (this.directoryController === controller) this.loading = false;
    }
  }
}
