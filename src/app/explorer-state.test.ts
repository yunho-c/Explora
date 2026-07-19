import { describe, expect, it } from "vitest";

import type { DirectoryRef } from "$lib/contracts/explorer";
import { DemoExplorerDataSource } from "$lib/data/demo-explorer-data-source";
import type { ListDirectoryOptions } from "$lib/data/explorer-data-source";

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

describe("ExplorerState", () => {
  it("loads locations and directory batches through the data-source boundary", async () => {
    const state = await initializedState();

    expect(state.locations).toHaveLength(6);
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
});
