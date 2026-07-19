import { describe, expect, it } from "vitest";

import { DemoExplorerDataSource } from "$lib/data/demo-explorer-data-source";

import { ExplorerState } from "./explorer-state.svelte";

const initializedState = async () => {
  const state = new ExplorerState(new DemoExplorerDataSource());
  await state.initialize();
  return state;
};

describe("ExplorerState", () => {
  it("loads locations and directory batches through the data-source boundary", async () => {
    const state = await initializedState();

    expect(state.locations).toHaveLength(6);
    expect(state.activeLocation?.name).toBe("Home");
    expect(state.entries).toHaveLength(10);
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

  it("loads and navigates previews from the keyboard selection model", async () => {
    const state = await initializedState();
    const firstEntry = state.visibleEntries[0];

    state.selectEntry(firstEntry.id);
    await state.openPreview();
    expect(state.preview?.entryId).toBe(firstEntry.id);

    state.moveSelection(1);
    expect(state.selectedEntryId).toBe(state.visibleEntries[1].id);

    state.closePreview();
    expect(state.previewOpen).toBe(false);
  });
});
