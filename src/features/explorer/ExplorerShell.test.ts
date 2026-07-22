import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/svelte";
import { describe, expect, it } from "vitest";

import { ExplorerState } from "../../app/explorer-state.svelte";
import {
  WindowChromeController,
  type WindowChromeAdapter,
} from "../../app/window-chrome.svelte";
import { DemoExplorerDataSource } from "$lib/data/demo-explorer-data-source";
import type { WatchSyncedFoldersOptions } from "$lib/data/explorer-data-source";
import { MemoryPreferencesDataSource } from "$lib/data/memory-preferences-data-source";
import type { LocationSummary } from "$lib/contracts/explorer";

import ExplorerShell from "./ExplorerShell.svelte";

const browserWindowChrome = () =>
  new WindowChromeController({
    isTauri: false,
    windowLabel: "browser",
    activate: async () => {},
    restoreAndShowNative: async () => {},
    show: async () => {},
  });

const renderShell = (state: ExplorerState) => {
  const windowChrome = browserWindowChrome();
  return {
    windowChrome,
    ...render(ExplorerShell, { state, windowChrome }),
  };
};

class ManualSyncedFolderDataSource extends DemoExplorerDataSource {
  private revision = 1;
  private onSnapshot: WatchSyncedFoldersOptions["onSnapshot"] | null = null;
  readonly folder: LocationSummary = {
    id: "synced:manual:5f4c234c-bc60-41f4-86e7-f43082f7d331",
    name: "Synced Folder 1",
    backend: "local",
    kind: "syncedFolder",
    role: "syncedFolder",
    status: "available",
    displayPath: "/home/test/Sync",
    detail: "Manually added · Synced folder",
    root: {
      id: "manual-root-token",
      locationId: "synced:manual:5f4c234c-bc60-41f4-86e7-f43082f7d331",
      name: "Synced Folder 1",
      displayPath: "/home/test/Sync",
    },
    syncedFolder: {
      provider: "other",
      status: "available",
      source: "manual",
    },
  };

  override async listLocations(
    signal: AbortSignal,
  ): Promise<readonly LocationSummary[]> {
    return (await super.listLocations(signal)).filter(
      ({ kind }) => kind !== "syncedFolder",
    );
  }

  override async watchSyncedFolders({
    signal,
    onSnapshot,
  }: WatchSyncedFoldersOptions): Promise<void> {
    this.onSnapshot = onSnapshot;
    onSnapshot({
      revision: this.revision,
      folders: [],
      warning: null,
      canAddFolder: true,
    });
    await new Promise<void>((resolve) => {
      signal.addEventListener("abort", () => resolve(), { once: true });
    });
  }

  override async addSyncedFolder(): Promise<string | null> {
    this.revision += 1;
    this.onSnapshot?.({
      revision: this.revision,
      folders: [this.folder],
      warning: null,
      canAddFolder: true,
    });
    return this.folder.id;
  }

  override async removeSyncedFolder(folderId: string): Promise<void> {
    expect(folderId).toBe(this.folder.id);
    this.revision += 1;
    this.onSnapshot?.({
      revision: this.revision,
      folders: [],
      warning: null,
      canAddFolder: true,
    });
  }
}

describe("ExplorerShell", () => {
  it("renders the loaded shell and switches between list and grid views", async () => {
    const state = new ExplorerState(new DemoExplorerDataSource());
    await state.initialize();
    renderShell(state);

    expect(
      screen.getByRole("main", { name: "File explorer" }),
    ).toBeInTheDocument();
    expect(screen.getByText("explora-notes.md")).toBeInTheDocument();
    const titlebarLeft = document.querySelector(".explora-titlebar-left");
    expect(titlebarLeft).not.toBeNull();
    expect(titlebarLeft).not.toHaveClass("border-b");
    expect(titlebarLeft?.textContent?.trim()).toBe("");
    expect(titlebarLeft?.querySelector("svg")).toBeNull();
    expect(document.querySelector(".explora-sidebar-scroll")).toHaveClass(
      "overflow-y-auto",
    );
    for (const header of ["Favorites", "Cloud Storage", "SSH"]) {
      expect(
        screen.getByText(header, { exact: true }).parentElement,
      ).toHaveClass("pl-2");
      expect(
        screen.getByText(header, { exact: true }).parentElement,
      ).not.toHaveClass("pr-2");
    }

    await fireEvent.click(screen.getByRole("button", { name: "Grid view" }));
    expect(screen.getByRole("grid", { name: "Files" })).toBeInTheDocument();
  });

  it("uses distinct semantic icons for default favorite folders", async () => {
    const state = new ExplorerState(new DemoExplorerDataSource());
    await state.initialize();
    renderShell(state);

    for (const [name, iconClass] of [
      ["Home", "lucide-house"],
      ["Desktop", "lucide-monitor"],
      ["Documents", "lucide-file-text"],
      ["Downloads", "lucide-download"],
      ["Pictures", "lucide-images"],
      ["Music", "lucide-music-2"],
      ["Movies", "lucide-film"],
    ] as const) {
      const button = screen.getByRole("button", { name });
      expect(button.querySelector(`.${iconClass}`)).toBeInTheDocument();
      expect(button).toHaveClass("gap-2");
    }
  });

  it("does not render an empty Locations section", async () => {
    const state = new ExplorerState(new DemoExplorerDataSource());
    await state.initialize();
    state.locations = state.locations.filter(({ kind }) => kind !== "volume");
    renderShell(state);

    expect(
      screen.queryByText("Locations", { exact: true }),
    ).not.toBeInTheDocument();
  });

  it("configures visible standard favorites from the section header", async () => {
    const preferences = new MemoryPreferencesDataSource();
    const state = new ExplorerState(new DemoExplorerDataSource(), preferences);
    await state.initialize();
    renderShell(state);
    const favorites = within(
      screen.getByRole("navigation", { name: "Favorites" }),
    );
    const configure = screen.getByRole("button", {
      name: "Configure favorites",
    });

    expect(configure).toHaveClass("md:opacity-0");
    expect(favorites.getByRole("button", { name: "Home" })).toBeInTheDocument();
    await fireEvent.click(configure);
    const finishEditing = screen.getByRole("button", {
      name: "Finish editing favorites",
    });
    expect(finishEditing).toHaveAttribute("aria-pressed", "true");
    expect(finishEditing.querySelector(".lucide-check")).toBeInTheDocument();
    await fireEvent.click(
      favorites.getByRole("button", { name: "Remove Home from Favorites" }),
    );

    await waitFor(() =>
      expect(
        favorites.queryByRole("button", { name: "Home" }),
      ).not.toBeInTheDocument(),
    );
    await waitFor(async () =>
      expect(
        (await preferences.getPreferences()).preferences.layout.favoriteRoles,
      ).not.toContain("home"),
    );
    const ghostHome = favorites.getByLabelText("Home, not in Favorites");
    expect(ghostHome).toHaveClass("border-dashed");
    expect(ghostHome.querySelector("svg")).toHaveClass("size-4");
    expect(
      favorites.getByRole("button", { name: "Add Home to Favorites" }),
    ).toHaveClass("text-emerald-600");

    await fireEvent.click(
      favorites.getByRole("button", { name: "Add Home to Favorites" }),
    );
    await waitFor(() =>
      expect(
        favorites.getByRole("button", { name: "Home" }),
      ).toBeInTheDocument(),
    );
    await fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Configure favorites" }),
      ).toHaveFocus(),
    );
    expect(
      favorites.queryByRole("button", { name: "Remove Home from Favorites" }),
    ).not.toBeInTheDocument();
  });

  it("configures visible SSH targets without deleting or disconnecting them", async () => {
    const preferences = new MemoryPreferencesDataSource();
    const state = new ExplorerState(new DemoExplorerDataSource(), preferences);
    await state.initialize();
    renderShell(state);
    const sshTargets = within(
      screen.getByRole("navigation", { name: "SSH targets" }),
    );
    const configure = screen.getByRole("button", {
      name: "Configure SSH targets",
    });

    expect(configure).toHaveClass("md:opacity-0");
    await fireEvent.click(configure);
    const finishEditing = screen.getByRole("button", {
      name: "Finish editing SSH targets",
    });
    expect(finishEditing.querySelector(".lucide-check")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Add SSH target" }),
    ).not.toBeInTheDocument();
    await fireEvent.click(
      sshTargets.getByRole("button", {
        name: "Hide staging-box from SSH",
      }),
    );

    await waitFor(() =>
      expect(
        sshTargets.queryByRole("button", {
          name: "staging-box connected",
        }),
      ).not.toBeInTheDocument(),
    );
    const ghostTarget = sshTargets.getByLabelText(
      "staging-box, hidden from SSH",
    );
    expect(ghostTarget).toHaveClass("border-dashed");
    expect(ghostTarget.querySelector("svg")).toHaveClass("size-4");
    expect(
      sshTargets.getByRole("button", { name: "Show staging-box in SSH" }),
    ).toHaveClass("text-emerald-600");
    await waitFor(async () =>
      expect(
        (await preferences.getPreferences()).preferences.layout
          .hiddenSshTargetIds,
      ).toEqual(["demo:staging-box"]),
    );
    expect(
      state.sshTargets.find(({ id }) => id === "demo:staging-box")?.status,
    ).toBe("connected");

    await fireEvent.click(
      sshTargets.getByRole("button", { name: "Show staging-box in SSH" }),
    );
    await fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Configure SSH targets" }),
      ).toHaveFocus(),
    );
    expect(
      screen.getByRole("button", { name: "Add SSH target" }),
    ).toBeInTheDocument();
    expect(
      sshTargets.queryByRole("button", { name: /Manage / }),
    ).not.toBeInTheDocument();
    expect(sshTargets.queryByText("Config")).not.toBeInTheDocument();

    await fireEvent.contextMenu(
      sshTargets.getByRole("button", { name: "staging-box connected" }),
    );
    expect(
      await screen.findByRole("menuitem", { name: "Edit" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("menuitem", { name: "Disconnect" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("menuitem", { name: "Remove" }),
    ).toBeInTheDocument();
  });

  it("shows discovered cloud storage and persists per-root visibility", async () => {
    const preferences = new MemoryPreferencesDataSource();
    const state = new ExplorerState(new DemoExplorerDataSource(), preferences);
    await state.initialize();
    renderShell(state);
    const cloudStorage = within(
      screen.getByRole("navigation", { name: "Cloud storage" }),
    );

    expect(
      cloudStorage.getByRole("button", { name: "iCloud Drive available" }),
    ).toBeInTheDocument();
    expect(
      cloudStorage.getByRole("button", { name: "OneDrive available" }),
    ).toBeInTheDocument();
    await fireEvent.click(
      screen.getByRole("button", { name: "Configure cloud storage" }),
    );
    await fireEvent.click(
      cloudStorage.getByRole("button", {
        name: "Hide OneDrive from Cloud Storage",
      }),
    );

    await waitFor(async () =>
      expect(
        (await preferences.getPreferences()).preferences.layout
          .hiddenSyncedFolderIds,
      ).toEqual(["synced:onedrive"]),
    );
    expect(
      cloudStorage.getByLabelText("OneDrive, hidden from Cloud Storage"),
    ).toHaveClass("border-dashed");
    expect(
      state.syncedFolderLocations.find(({ id }) => id === "synced:onedrive")
        ?.status,
    ).toBe("available");

    await fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Configure cloud storage" }),
      ).toHaveFocus(),
    );
  });

  it("adds and removes an explicit local synced folder without exposing its path as authority", async () => {
    const dataSource = new ManualSyncedFolderDataSource();
    const state = new ExplorerState(dataSource);
    await state.initialize();
    renderShell(state);

    expect(
      screen.getByText("Add a local folder managed by your sync client."),
    ).toBeInTheDocument();
    await fireEvent.click(
      screen.getByRole("button", { name: "Add synced folder" }),
    );
    const cloudStorage = within(
      screen.getByRole("navigation", { name: "Cloud storage" }),
    );
    await waitFor(() =>
      expect(
        cloudStorage.getByRole("button", {
          name: "Synced Folder 1 available",
        }),
      ).toBeInTheDocument(),
    );

    await fireEvent.click(
      screen.getByRole("button", { name: "Configure cloud storage" }),
    );
    await fireEvent.click(
      cloudStorage.getByRole("button", {
        name: "Remove Synced Folder 1 from Explora",
      }),
    );
    await waitFor(() =>
      expect(
        screen.getByText("Add a local folder managed by your sync client."),
      ).toBeInTheDocument(),
    );
    expect(screen.queryByText("/home/test/Sync")).not.toBeInTheDocument();
  });

  it("labels online-only entries and downloads only after explicit preview intent", async () => {
    const state = new ExplorerState(new DemoExplorerDataSource());
    await state.initialize();
    await state.selectLocation("synced:icloud");
    renderShell(state);

    const entry = screen.getByText("Reference library.pdf");
    expect(
      entry.parentElement?.querySelector("[aria-label='Online only']"),
    ).toBeInTheDocument();
    await fireEvent.click(entry);
    await fireEvent.keyDown(window, { key: " " });

    const previewDialog = await screen.findByRole("dialog");
    expect(
      await within(previewDialog).findByText(
        "Download this file before opening Quick Preview.",
      ),
    ).toBeInTheDocument();
    await fireEvent.click(
      within(previewDialog).getByRole("button", {
        name: "Download to Preview",
      }),
    );
    expect(
      within(previewDialog).getByRole("button", { name: "Stop waiting" }),
    ).toBeInTheDocument();
    expect(
      within(previewDialog).getByText(
        "Stopping here will not stop the operating system download.",
      ),
    ).toBeInTheDocument();
    await waitFor(() => expect(state.preview?.content.type).toBe("pdf"));
    expect(
      within(previewDialog).queryByRole("button", {
        name: "Download to Preview",
      }),
    ).not.toBeInTheDocument();
  });

  it("opens Quick Preview with the Space key", async () => {
    const state = new ExplorerState(new DemoExplorerDataSource());
    await state.initialize();
    renderShell(state);

    await fireEvent.click(screen.getByText("explora-notes.md"));
    await fireEvent.keyDown(window, { key: " " });

    const previewDialog = await screen.findByRole("dialog");
    expect(previewDialog).toBeInTheDocument();
    expect(within(previewDialog).queryByText("Path")).not.toBeInTheDocument();
    expect(
      within(previewDialog).queryByText("Location"),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("Preparing preview")).not.toBeInTheDocument();
    expect(
      screen.queryByText("Reading a safe, bounded view of this file…"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText("Use ↑ and ↓ to move between items · Esc to close"),
    ).not.toBeInTheDocument();
    const textPreview = await screen.findByRole<HTMLTextAreaElement>(
      "textbox",
      {
        name: "Text preview of explora-notes.md",
      },
    );
    expect(textPreview.value).toContain("local and remote files");
    expect(screen.queryByText("UTF-8 text")).not.toBeInTheDocument();

    const selectedEntryId = state.selectedEntryId;
    textPreview.focus();
    await fireEvent.keyDown(textPreview, { key: "ArrowDown" });
    await waitFor(() =>
      expect(state.selectedEntryId).not.toBe(selectedEntryId),
    );
  });

  it("renders raster preview content with an accessible file name", async () => {
    const state = new ExplorerState(new DemoExplorerDataSource());
    await state.initialize();
    renderShell(state);

    await fireEvent.click(screen.getByText("summer-light.jpg"));
    await fireEvent.keyDown(window, { key: " " });

    expect(
      await screen.findByRole("img", {
        name: "Preview of summer-light.jpg",
      }),
    ).toBeInTheDocument();
    const sanitizeToggle = screen.getByRole("button", {
      name: "Use sanitized image preview",
    });
    expect(sanitizeToggle).toHaveAttribute("aria-pressed", "false");

    await fireEvent.click(sanitizeToggle);
    await waitFor(() => expect(state.imagePreviewMode).toBe("sanitized"));
    expect(
      await screen.findByRole("button", { name: "Use direct image preview" }),
    ).toHaveAttribute("aria-pressed", "true");
    expect(screen.queryByText("Image")).not.toBeInTheDocument();
  });

  it("opens folders on double-click and returns with the Up action", async () => {
    const state = new ExplorerState(new DemoExplorerDataSource());
    await state.initialize();
    renderShell(state);

    await fireEvent.dblClick(screen.getByText("Projects"));
    await waitFor(() => expect(state.activeDirectory?.name).toBe("Projects"));
    expect(screen.getByRole("button", { name: "Go back" })).toBeEnabled();
    expect(
      screen.getByRole("button", { name: "Go to parent folder" }),
    ).toBeEnabled();

    await fireEvent.click(
      screen.getByRole("button", { name: "Go to parent folder" }),
    );
    await waitFor(() => expect(state.activeDirectory?.name).toBe("Home"));
    expect(await screen.findByText("explora-notes.md")).toBeInTheDocument();
  });

  it("limits custom titlebar dragging to non-interactive chrome", async () => {
    const state = new ExplorerState(new DemoExplorerDataSource());
    await state.initialize();
    const adapter: WindowChromeAdapter = {
      isTauri: true,
      windowLabel: "main",
      activate: async () => {},
      restoreAndShowNative: async () => {},
      show: async () => {},
    };
    const windowChrome = new WindowChromeController(adapter);
    windowChrome.mode = "custom";
    render(ExplorerShell, { state, windowChrome });

    const shell = screen.getByRole("main", {
      name: "File explorer",
    }).parentElement;
    expect(shell).toHaveAttribute("data-window-chrome", "custom");
    expect(
      document.querySelectorAll("[data-tauri-drag-region]").length,
    ).toBeGreaterThan(0);
    expect(screen.getByRole("tab", { name: "Home" })).not.toHaveAttribute(
      "data-tauri-drag-region",
    );
    expect(
      screen.getByRole("button", { name: "Open a new tab" }),
    ).not.toHaveAttribute("data-tauri-drag-region");

    state.sidebarCollapsed = true;
    await waitFor(() =>
      expect(
        screen.getByRole("tablist", { name: "Open locations" }),
      ).toHaveClass("explora-titlebar-tabs-collapsed"),
    );
    expect(shell).toHaveAttribute("data-sidebar-collapsed", "true");

    windowChrome.mode = "native";
    await waitFor(() =>
      expect(shell).toHaveAttribute("data-window-chrome", "native"),
    );
    expect(document.querySelectorAll("[data-tauri-drag-region]")).toHaveLength(
      0,
    );
  });
});
