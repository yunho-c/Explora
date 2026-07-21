import { describe, expect, it } from "vitest";

import type { DirectoryRef, FileEntrySummary } from "$lib/contracts/explorer";
import type { SshConnectionEvent } from "$lib/contracts/explorer";
import { DemoExplorerDataSource } from "$lib/data/demo-explorer-data-source";
import type {
  ConnectSshOptions,
  ListDirectoryOptions,
  PreparePreviewOptions,
  PreparedPreview,
} from "$lib/data/explorer-data-source";

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
        },
        details: [],
      },
      dispose: () => this.disposedEntryIds.push(entry.reference.id),
    };
  }
}

describe("ExplorerState", () => {
  it("loads locations and directory batches through the data-source boundary", async () => {
    const state = await initializedState();

    expect(state.locations).toHaveLength(10);
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
    });
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
