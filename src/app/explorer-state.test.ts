import { describe, expect, it, vi } from "vitest";

import type { DirectoryRef } from "$lib/contracts/explorer";
import type {
  PreferencesSnapshot,
  UserPreferences,
  UserPreferencesPatch,
} from "$lib/contracts/preferences";
import { DemoExplorerDataSource } from "$lib/data/demo-explorer-data-source";
import type { ListDirectoryOptions } from "$lib/data/explorer-data-source";
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

  it("restores global layout preferences before loading explorer data", async () => {
    const preferences = new MemoryPreferencesDataSource({
      layout: {
        sidebarCollapsed: true,
        viewMode: "grid",
        sort: { column: "size", direction: "descending" },
        favoriteRoles: ["home", "music"],
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
});
