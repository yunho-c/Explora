import { describe, expect, it, vi } from "vitest";

import type {
  DirectoryRef,
  FileEntrySummary,
  FileOperationBatchResult,
  LocationSummary,
  SshConnectionEvent,
  VolumeSnapshot,
} from "$lib/contracts/explorer";
import type {
  PreferencesSnapshot,
  UserPreferences,
  UserPreferencesPatch,
} from "$lib/contracts/preferences";
import { DemoExplorerDataSource } from "$lib/data/demo-explorer-data-source";
import type {
  ConnectSshOptions,
  FileOperationOptions,
  ListDirectoryOptions,
  OpenEntryOptions,
  PreparePreviewOptions,
  PreparedPreview,
  WatchVolumesOptions,
} from "$lib/data/explorer-data-source";
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
        capabilities: {
          createFolder: true,
          acceptCopy: true,
          acceptMove: true,
          atomicReplace: false,
        },
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

class RecordingNativeOpenDataSource extends DemoExplorerDataSource {
  readonly opened: string[] = [];

  override async openEntry(entry: FileEntrySummary, options: OpenEntryOptions) {
    this.opened.push(entry.name);
    return super.openEntry(entry, options);
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
        terminal: {
          visible: false,
          paneHeightPercent: 32,
          fontSize: 13,
          scrollback: 5_000,
          screenReaderMode: true,
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

class ControllableVolumeDataSource extends DemoExplorerDataSource {
  private onVolumeSnapshot: ((snapshot: VolumeSnapshot) => void) | null = null;

  override async watchVolumes({
    signal,
    onSnapshot,
  }: WatchVolumesOptions): Promise<void> {
    this.onVolumeSnapshot = onSnapshot;
    await new Promise<void>((resolve) => {
      signal.addEventListener("abort", () => resolve(), { once: true });
    });
  }

  emitVolumes(revision: number, volumes: readonly LocationSummary[]): void {
    this.onVolumeSnapshot?.({ revision, volumes, warning: null });
  }
}

class PartialBatchMoveDataSource extends DemoExplorerDataSource {
  override async moveEntries(
    entries: readonly FileEntrySummary[],
    destination: DirectoryRef,
    options: FileOperationOptions,
  ): Promise<FileOperationBatchResult> {
    const moved = await super.moveEntry(entries[0], destination, options);
    return {
      kind: "batch",
      status: "partial",
      items: [
        {
          status: "completed",
          source: entries[0].reference,
          outcome: moved,
        },
        {
          status: "failed",
          source: entries[1].reference,
          error: {
            code: "permissionDenied",
            message: "The second item is read-only.",
          },
        },
      ],
    };
  }
}

describe("ExplorerState", () => {
  it("creates and selects folders, preserves conflicting drafts, and navigates into the new folder", async () => {
    const state = await initializedState();
    state.startNewFolder();
    state.newFolderName = "Projects";
    await state.createFolder();
    expect(state.newFolderErrorMessage).toMatch(/already exists/);
    expect(state.newFolderName).toBe("Projects");
    state.newFolderName = "Created folder";
    await state.createFolder();
    expect(state.newFolderParent).toBeNull();
    expect(state.selectedEntry?.name).toBe("Created folder");
    const id = state.selectedEntryId!;
    await state.openEntry(id);
    expect(state.activeDirectory?.name).toBe("Created folder");
    expect(state.entries).toHaveLength(0);
    state.startNewFolder();
    state.newFolderName = "Nested";
    await state.createFolder();
    expect(state.selectedEntry?.name).toBe("Nested");
    state.dispose();
  });

  it("keeps empty and invalid folder names editable and cancels without mutation", async () => {
    const state = await initializedState();
    const count = state.entries.length;
    state.startNewFolder();
    await state.createFolder();
    expect(state.newFolderErrorMessage).toBe("Enter a folder name.");
    state.newFolderName = "../escape";
    await state.createFolder();
    expect(state.newFolderErrorMessage).toMatch(/valid folder name/);
    state.cancelNewFolder();
    expect(state.newFolderParent).toBeNull();
    state.startNewFolder();
    state.newFolderName = "Cancelled";
    const creation = state.createFolder();
    state.cancelNewFolder();
    await creation;
    expect(state.entries).toHaveLength(count);
    expect(state.newFolderParent).toBeNull();
    state.dispose();
  });

  it("gates New Folder using directory capabilities and connection state", async () => {
    const state = await initializedState();
    expect(state.canCreateFolder).toBe(true);
    state.activeTab!.directory = {
      ...state.activeDirectory!,
      capabilities: {
        ...state.activeDirectory!.capabilities,
        createFolder: false,
      },
    };
    expect(state.canCreateFolder).toBe(false);
    state.startNewFolder();
    expect(state.newFolderParent).toBeNull();
    await state.connectSshTarget(state.sshTargets[0].id);
    expect(state.canCreateFolder).toBe(true);
    state.startNewFolder();
    state.newFolderName = "Remote folder";
    await state.createFolder();
    expect(state.selectedEntry?.name).toBe("Remote folder");
    await state.disconnectSshTarget(state.activeSshTarget!.id);
    expect(state.canCreateFolder).toBe(false);
    state.dispose();
  });

  it("does not insert a late folder creation into another location", async () => {
    const source = new DemoExplorerDataSource();
    const original = source.createFolder.bind(source);
    let complete: (() => void) | undefined;
    vi.spyOn(source, "createFolder").mockImplementation(
      async (parent, name, signal) => {
        await new Promise<void>((resolve) => {
          complete = resolve;
        });
        return original(parent, name, signal);
      },
    );
    const state = new ExplorerState(source);
    await state.initialize();
    state.startNewFolder();
    state.newFolderName = "Home folder";
    const creation = state.createFolder();
    await state.selectLocation("downloads");
    complete?.();
    await creation;
    expect(state.activeDirectory?.locationId).toBe("downloads");
    expect(state.entries.some((entry) => entry.name === "Home folder")).toBe(
      false,
    );
    await state.selectLocation("home");
    expect(state.entries.some((entry) => entry.name === "Home folder")).toBe(
      true,
    );
    state.dispose();
  });
  it("renames an entry while preserving selection and opaque identity", async () => {
    const state = await initializedState();
    const entry = state.entries.find(({ name }) => name === "explora-notes.md");
    expect(entry?.capabilities.rename).toBe(true);
    state.selectEntry(entry!.reference.id);
    state.startRename();
    state.renameDraft = "renamed-notes.md";

    await state.commitRename();

    const renamed = state.entries.find(
      ({ reference }) => reference.id === entry!.reference.id,
    );
    expect(renamed).toMatchObject({
      name: "renamed-notes.md",
      reference: entry!.reference,
    });
    expect(state.selectedEntryId).toBe(entry!.reference.id);
    expect(state.renamingEntryId).toBeNull();
    expect(state.renameErrorMessage).toBeNull();
  });

  it("keeps rename active after a conflict so the user can correct it", async () => {
    const state = await initializedState();
    const projects = state.entries.find(({ name }) => name === "Projects");
    state.startRename(projects!.reference.id);
    state.renameDraft = "Photos";

    await state.commitRename();

    expect(state.renamingEntryId).toBe(projects!.reference.id);
    expect(state.renameErrorMessage).toBe(
      "An item with that name already exists.",
    );
    expect(state.entries.some(({ name }) => name === "Projects")).toBe(true);
    state.cancelRename();
    expect(state.renamingEntryId).toBeNull();
  });

  it("moves a local entry to Trash and advances selection without prompting", async () => {
    const state = await initializedState();
    const entry = state.entries.find(({ name }) => name === "explora-notes.md");
    expect(entry).toBeDefined();
    state.selectEntry(entry!.reference.id);
    await state.openPreview();

    await state.moveSelectedToTrash();

    expect(
      state.entries.some(
        ({ reference }) => reference.id === entry!.reference.id,
      ),
    ).toBe(false);
    expect(state.selectedEntryId).not.toBe(entry!.reference.id);
    expect(state.previewOpen).toBe(false);
    expect(state.fileOperations.pendingPrompt).toBeNull();
    expect(state.fileOperations.errorMessage).toBeNull();
  });

  it("moves a local entry through the destination chooser and refreshes both folders", async () => {
    const state = await initializedState();
    const entry = state.entries.find(({ name }) => name === "explora-notes.md");
    const projects = state.entries.find(({ name }) => name === "Projects");
    expect(entry).toBeDefined();
    expect(projects?.directory).toBeDefined();
    state.selectEntry(entry!.reference.id);

    await state.openMoveSelected();
    expect(state.fileOperations.moveChooser?.directory.name).toBe("Home");
    expect(state.fileOperations.canConfirmMove).toBe(false);
    await state.fileOperations.browseMoveDestination(projects!.directory!);
    expect(state.fileOperations.moveChooser?.directory.name).toBe("Projects");
    expect(state.fileOperations.canConfirmMove).toBe(true);

    await state.confirmMoveSelected();
    expect(state.entries.some(({ name }) => name === "explora-notes.md")).toBe(
      false,
    );
    expect(state.selectedEntryId).not.toBe(entry!.reference.id);
    expect(state.fileOperations.errorMessage).toBeNull();

    await state.openDirectory(projects!.directory!);
    expect(state.entries.some(({ name }) => name === "explora-notes.md")).toBe(
      true,
    );
  });

  it("supports replace, toggle, range, and select-all selection semantics", async () => {
    const state = await initializedState();
    const entries = state.visibleEntries;
    expect(entries.length).toBeGreaterThan(3);

    state.selectEntry(entries[0].reference.id);
    state.selectEntry(entries[2].reference.id, { range: true });
    expect(state.selectedEntryIds).toEqual(
      entries.slice(0, 3).map(({ reference }) => reference.id),
    );
    expect(state.selectedEntryId).toBe(entries[2].reference.id);

    state.selectEntry(entries[1].reference.id, { toggle: true });
    expect(state.selectedEntryIds).toEqual([
      entries[0].reference.id,
      entries[2].reference.id,
    ]);

    state.selectAllEntries();
    expect(state.selectedEntryIds).toEqual(
      entries.map(({ reference }) => reference.id),
    );
    state.clearSelection();
    expect(state.selectedEntryIds).toEqual([]);
    expect(state.selectedEntryId).toBeNull();
  });

  it("intersects capabilities across every selected entry", async () => {
    const state = await initializedState();
    const [first, second] = state.visibleEntries;
    state.entries = state.entries.map((entry) =>
      entry.reference.id === second.reference.id
        ? {
            ...entry,
            capabilities: { ...entry.capabilities, trash: false },
          }
        : entry,
    );

    state.selectEntry(first.reference.id);
    state.selectEntry(second.reference.id, { toggle: true });

    expect(state.canMoveSelection).toBe(true);
    expect(state.canTrashSelection).toBe(false);
    expect(state.canRenameSelection).toBe(false);
    expect(state.canDeleteSelectionPermanently).toBe(true);
  });

  it("moves multiple selected entries through one batch operation", async () => {
    const state = await initializedState();
    const notes = state.entries.find(
      ({ name }) => name === "explora-notes.md",
    )!;
    const image = state.entries.find(
      ({ name }) => name === "summer-light.jpg",
    )!;
    const projects = state.entries.find(({ name }) => name === "Projects")!;
    state.selectEntry(notes.reference.id);
    state.selectEntry(image.reference.id, { toggle: true });

    await state.openMoveSelected();
    expect(state.fileOperations.moveChooser?.entries).toHaveLength(2);
    await state.fileOperations.browseMoveDestination(projects.directory!);
    await state.confirmMoveSelected();

    expect(state.entries.some(({ name }) => name === notes.name)).toBe(false);
    expect(state.entries.some(({ name }) => name === image.name)).toBe(false);
    expect(state.fileOperations.errorMessage).toBeNull();

    await state.openDirectory(projects.directory!);
    expect(state.entries.some(({ name }) => name === notes.name)).toBe(true);
    expect(state.entries.some(({ name }) => name === image.name)).toBe(true);
  });

  it.each(["partial", "cancelled"])(
    "retains copied sources and completed items after a %s batch",
    async (status) => {
      const data = new DemoExplorerDataSource();
      const original = data.copyEntry.bind(data);
      const started = vi.fn();
      vi.spyOn(data, "copyEntry").mockImplementation(
        async (entry, destination, options) => {
          started();
          if (started.mock.calls.length === 2) {
            if (status === "partial")
              throw new Error("The second source could not be read.");
            await new Promise<void>((_resolve, reject) => {
              options.signal.addEventListener(
                "abort",
                () => {
                  const error = new Error("Cancelled");
                  error.name = "AbortError";
                  reject(error);
                },
                { once: true },
              );
            });
          }
          return original(entry, destination, options);
        },
      );
      const state = new ExplorerState(data);
      await state.initialize();
      const selected = state.entries.filter(
        ({ name }) =>
          name === "explora-notes.md" || name === "summer-light.jpg",
      );
      const projects = state.entries.find(({ name }) => name === "Projects")!;
      state.selectEntry(selected[0].reference.id);
      state.selectEntry(selected[1].reference.id, { toggle: true });
      state.copySelected();
      await state.openDirectory(projects.directory!);
      const paste = state.pasteEntries();
      if (status === "cancelled") {
        await vi.waitFor(() => expect(started).toHaveBeenCalledTimes(2));
        state.fileOperations.cancelActive();
      }
      await paste;
      expect(state.entries).toHaveLength(1);
      expect(state.selectedEntries).toHaveLength(1);
      expect(state.clipboardEntries).toHaveLength(2);
      if (status === "partial")
        expect(state.fileOperations.errorMessage).toContain(
          "1 of 2 items could not be completed",
        );
      state.dispose();
    },
  );

  it("copies a multi-selection without cutting it and retains sources for repeated paste", async () => {
    const state = await initializedState();
    const notes = state.entries.find(
      ({ name }) => name === "explora-notes.md",
    )!;
    const image = state.entries.find(
      ({ name }) => name === "summer-light.jpg",
    )!;
    const projects = state.entries.find(({ name }) => name === "Projects")!;
    const home = state.activeDirectory!;
    state.selectEntry(notes.reference.id);
    state.selectEntry(image.reference.id, { toggle: true });
    state.copySelected();
    expect(state.clipboardMode).toBe("copy");
    expect(state.isEntryCut(notes.reference.id)).toBe(false);
    expect(state.canPasteEntries).toBe(true);
    await state.openDirectory(projects.directory!);
    await state.pasteEntries();
    expect(state.selectedEntries.map(({ name }) => name)).toEqual(
      expect.arrayContaining([notes.name, image.name]),
    );
    expect(state.clipboardEntries).toHaveLength(2);
    expect(
      state.entries.find(({ name }) => name === notes.name)?.reference.id,
    ).not.toBe(notes.reference.id);
    await state.openDirectory(home);
    expect(
      state.entries.find(({ name }) => name === notes.name)?.reference,
    ).toEqual(notes.reference);
    expect(
      state.entries.find(({ name }) => name === image.name)?.reference,
    ).toEqual(image.reference);
    await state.openDirectory(projects.directory!);
    const repeat = state.pasteEntries();
    await vi.waitFor(() =>
      expect(state.fileOperations.pendingPrompt).not.toBeNull(),
    );
    await state.fileOperations.answerPrompt("keepBoth");
    await vi.waitFor(() =>
      expect(state.fileOperations.pendingPrompt).not.toBeNull(),
    );
    await state.fileOperations.answerPrompt("skip");
    await repeat;
    expect(state.entries.some(({ name }) => name.includes(" copy."))).toBe(
      true,
    );
    expect(state.clipboardEntries).toHaveLength(2);
    state.dispose();
  });

  it("copies into the source folder with explicit keep-both, skip and cancellation", async () => {
    const state = await initializedState();
    const notes = state.entries.find(
      ({ name }) => name === "explora-notes.md",
    )!;
    state.selectEntry(notes.reference.id);
    state.cutSelected();
    expect(state.canPasteEntries).toBe(false);
    state.copySelected();
    expect(state.canPasteEntries).toBe(true);
    for (const decision of ["skip", "cancel", "keepBoth"] as const) {
      const paste = state.pasteEntries();
      await vi.waitFor(() =>
        expect(state.fileOperations.pendingPrompt).not.toBeNull(),
      );
      await state.fileOperations.answerPrompt(decision);
      await paste;
      expect(
        state.entries.find(({ name }) => name === notes.name)?.reference,
      ).toEqual(notes.reference);
      expect(state.clipboardEntries[0].reference).toEqual(notes.reference);
    }
    expect(
      state.entries.some(({ name }) => name === "explora-notes copy.md"),
    ).toBe(true);
    state.cutSelected();
    expect(state.clipboardMode).toBe("cut");
    state.clearClipboard();
    expect(state.canPasteEntries).toBe(false);
    state.dispose();
  });

  it("retains a committed copy when an older directory listing arrives afterward", async () => {
    const data = new DemoExplorerDataSource();
    const state = new ExplorerState(data);
    await state.initialize();
    const notes = state.entries.find(
      ({ name }) => name === "explora-notes.md",
    )!;
    const projects = state.entries.find(({ name }) => name === "Projects")!;
    state.selectEntry(notes.reference.id);
    state.copySelected();
    await state.openDirectory(projects.directory!);
    let releaseListing: (() => void) | undefined;
    vi.spyOn(data, "listDirectory").mockImplementation(
      async (directory, options) => {
        options.onStart({ directory, parent: null, breadcrumbs: [] });
        await new Promise<void>((resolve) => {
          releaseListing = resolve;
        });
        options.onBatch({ entries: [], replace: true });
        options.onComplete({ skippedEntries: 0 });
      },
    );
    const paste = state.pasteEntries();
    const refresh = state.refreshDirectory();
    await paste;
    releaseListing!();
    await refresh;
    expect(state.entries.map(({ name }) => name)).toContain(notes.name);
    expect(state.selectedEntries.map(({ name }) => name)).toContain(notes.name);
    state.dispose();
  });

  it("leaves navigation alone when a copy completes in a different folder", async () => {
    let finish: (() => void) | undefined;
    const data = new DemoExplorerDataSource();
    const copy = data.copyEntry.bind(data);
    vi.spyOn(data, "copyEntry").mockImplementation(async (...args) => {
      await new Promise<void>((resolve) => {
        finish = resolve;
      });
      return copy(...args);
    });
    const state = new ExplorerState(data);
    await state.initialize();
    const home = state.activeDirectory!;
    const notes = state.entries.find(
      ({ name }) => name === "explora-notes.md",
    )!;
    const projects = state.entries.find(({ name }) => name === "Projects")!;
    state.selectEntry(notes.reference.id);
    state.copySelected();
    await state.openDirectory(projects.directory!);
    const paste = state.pasteEntries();
    await state.openDirectory(home);
    state.selectEntry(notes.reference.id);
    finish!();
    await paste;
    expect(state.activeDirectory?.id).toBe(home.id);
    expect(state.selectedEntryId).toBe(notes.reference.id);
    expect(state.clipboardEntries).toHaveLength(1);
    await state.openDirectory(projects.directory!);
    expect(state.entries.some(({ name }) => name === notes.name)).toBe(true);
    state.dispose();
  });

  it("uses copy capabilities independently of move capabilities and rejects offline or self destinations", async () => {
    const state = await initializedState();
    const projects = state.entries.find(({ name }) => name === "Projects")!;
    projects.capabilities.move = false;
    state.selectEntry(projects.reference.id);
    expect(state.canCopySelection).toBe(true);
    expect(state.canCutSelection).toBe(false);
    state.copySelected();
    await state.openDirectory(projects.directory!);
    expect(state.canPasteEntries).toBe(false);
    state.dispose();
  });

  it("cuts a multi-selection and pastes it through the batch move API", async () => {
    const state = await initializedState();
    const notes = state.entries.find(
      ({ name }) => name === "explora-notes.md",
    )!;
    const image = state.entries.find(
      ({ name }) => name === "summer-light.jpg",
    )!;
    const projects = state.entries.find(({ name }) => name === "Projects")!;
    state.selectEntry(notes.reference.id);
    state.selectEntry(image.reference.id, { toggle: true });

    state.cutSelected();
    expect(state.clipboardEntries.map(({ name }) => name)).toEqual(
      expect.arrayContaining([notes.name, image.name]),
    );
    expect(state.clipboardEntries).toHaveLength(2);
    expect(state.canPasteEntries).toBe(false);

    await state.openDirectory(projects.directory!);
    expect(state.canPasteEntries).toBe(true);
    await state.pasteEntries();

    expect(state.clipboardEntries).toEqual([]);
    expect(state.entries.some(({ name }) => name === notes.name)).toBe(true);
    expect(state.entries.some(({ name }) => name === image.name)).toBe(true);
    expect(state.fileOperations.errorMessage).toBeNull();
  });

  it("keeps failed cut items available after a partial paste", async () => {
    const state = new ExplorerState(new PartialBatchMoveDataSource());
    await state.initialize();
    const notes = state.entries.find(
      ({ name }) => name === "explora-notes.md",
    )!;
    const image = state.entries.find(
      ({ name }) => name === "summer-light.jpg",
    )!;
    const projects = state.entries.find(({ name }) => name === "Projects")!;
    state.selectEntry(notes.reference.id);
    state.selectEntry(image.reference.id, { toggle: true });
    state.cutSelected();
    const attempted = [...state.clipboardEntries];

    await state.openDirectory(projects.directory!);
    await state.pasteEntries();

    expect(state.clipboardEntries).toHaveLength(1);
    expect(state.clipboardEntries[0].reference).toEqual(attempted[1].reference);
    expect(state.fileOperations.errorMessage).toContain(
      "1 of 2 items could not be completed",
    );
    expect(state.fileOperations.errorMessage).toContain(
      "The second item is read-only.",
    );
  });

  it("drops an internal opaque-reference drag onto a capable directory", async () => {
    const state = await initializedState();
    const notes = state.entries.find(
      ({ name }) => name === "explora-notes.md",
    )!;
    const projects = state.entries.find(({ name }) => name === "Projects")!;
    state.selectEntry(notes.reference.id);
    const dataTransfer = {
      effectAllowed: "none",
      dropEffect: "none",
      setData: vi.fn(),
    };
    const preventDefault = vi.fn();
    const event = { dataTransfer, preventDefault } as unknown as DragEvent;

    state.startEntryDrag(notes.reference.id, event);
    expect(dataTransfer.effectAllowed).toBe("move");
    expect(dataTransfer.setData).toHaveBeenCalledWith(
      "application/x-explora-entry",
      "internal",
    );
    expect(state.canDropOnDirectory(projects.directory!)).toBe(true);
    state.dragOverDirectory(projects.directory!, event);
    expect(state.isDirectoryDropTarget(projects.directory!)).toBe(true);
    await state.dropDraggedEntries(projects.directory!, event);

    expect(preventDefault).toHaveBeenCalled();
    expect(state.isDirectoryDropTarget(projects.directory!)).toBe(false);
    expect(state.entries.some(({ name }) => name === notes.name)).toBe(false);
    await state.openDirectory(projects.directory!);
    expect(state.entries.some(({ name }) => name === notes.name)).toBe(true);
  });

  it("moves an entry to another capable location through the transfer path", async () => {
    const state = await initializedState();
    const entry = state.entries.find(({ name }) => name === "explora-notes.md");
    const workspace = state.locations.find(({ id }) => id === "workspace");
    expect(entry).toBeDefined();
    expect(workspace).toBeDefined();
    state.selectEntry(entry!.reference.id);

    await state.openMoveSelected();
    await state.fileOperations.browseMoveDestination(workspace!.root);
    expect(state.fileOperations.moveChooser?.directory.name).toBe("Workspace");
    expect(state.fileOperations.canConfirmMove).toBe(true);
    await state.confirmMoveSelected();

    expect(state.entries.some(({ name }) => name === "explora-notes.md")).toBe(
      false,
    );
    expect(state.fileOperations.errorMessage).toBeNull();
    await state.openDirectory(workspace!.root);
    const moved = state.entries.find(({ name }) => name === "explora-notes.md");
    expect(moved?.reference.locationId).toBe("workspace");
  });

  it("does not allow a directory to be chosen as its own move destination", async () => {
    const state = await initializedState();
    const projects = state.entries.find(({ name }) => name === "Projects");
    expect(projects?.directory).toBeDefined();
    state.selectEntry(projects!.reference.id);

    await state.openMoveSelected();
    expect(
      state.fileOperations.isCompatibleDestination(projects!.directory!),
    ).toBe(false);
    await state.fileOperations.browseMoveDestination(projects!.directory!);
    expect(state.fileOperations.moveChooser?.directory.name).toBe("Home");
    expect(state.fileOperations.canConfirmMove).toBe(false);
    state.fileOperations.closeMoveChooser();
  });

  it("cancels permanent deletion without changing the source", async () => {
    const state = await initializedState();
    const entry = state.entries.find(({ name }) => name === "explora-notes.md");
    expect(entry).toBeDefined();
    state.selectEntry(entry!.reference.id);

    const deletion = state.deleteSelectedPermanently();
    await vi.waitFor(() =>
      expect(state.fileOperations.pendingPrompt?.prompt).toMatchObject({
        targetName: "explora-notes.md",
        locationName: "Home",
      }),
    );
    await state.fileOperations.answerPrompt("cancel");
    await deletion;

    expect(
      state.entries.some(
        ({ reference }) => reference.id === entry!.reference.id,
      ),
    ).toBe(true);
    expect(state.fileOperations.pendingPrompt).toBeNull();
    expect(state.fileOperations.errorMessage).toBeNull();
  });

  it("repairs open tabs and histories after confirmed permanent deletion", async () => {
    const state = await initializedState();
    const projects = state.entries.find(({ name }) => name === "Projects");
    expect(projects?.directory).toBeDefined();
    const homeTabId = state.activeTabId;
    await state.openTab();
    const projectsTabId = state.activeTabId;
    await state.openDirectory(projects!.directory!);
    await state.activateTab(homeTabId);
    state.selectEntry(projects!.reference.id);

    const deletion = state.deleteSelectedPermanently();
    await vi.waitFor(() =>
      expect(state.fileOperations.pendingPrompt).not.toBeNull(),
    );
    await state.fileOperations.answerPrompt("confirm");
    await deletion;

    expect(state.entries.some(({ name }) => name === "Projects")).toBe(false);
    const repairedTab = state.tabs.find(({ id }) => id === projectsTabId);
    expect(repairedTab?.directory.name).toBe("Home");
    expect(
      repairedTab?.history.some(({ id }) => id === projects!.reference.id),
    ).toBe(false);
  });

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

  it("preserves an active volume tab across removal and restores it at the root", async () => {
    const dataSource = new ControllableVolumeDataSource();
    const state = new ExplorerState(dataSource);
    await state.initialize();
    const workspace = state.locations.find(({ id }) => id === "workspace");
    expect(workspace).toBeDefined();
    await state.selectLocation("workspace");

    dataSource.emitVolumes(1, []);
    await vi.waitFor(() =>
      expect(state.activeLocation?.status).toBe("offline"),
    );
    expect(state.activeTab?.locationId).toBe("workspace");
    expect(state.warningMessage).toContain("no longer available");

    dataSource.emitVolumes(2, [workspace as LocationSummary]);
    await vi.waitFor(() =>
      expect(state.activeLocation?.status).toBe("available"),
    );
    expect(state.activeDirectory?.id).toBe(workspace?.root.id);
    expect(state.warningMessage).toBeNull();
    state.dispose();
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
      terminal: {
        visible: false,
        paneHeightPercent: 32,
        fontSize: 13,
        scrollback: 5_000,
        screenReaderMode: true,
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
      capabilities: {
        createFolder: true,
        acceptCopy: true,
        acceptMove: true,
        atomicReplace: false,
      },
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

  it("opens files natively while keeping Space-based preview separate", async () => {
    const dataSource = new RecordingNativeOpenDataSource();
    const state = new ExplorerState(dataSource);
    await state.initialize();
    const file = state.entries.find(({ name }) => name === "explora-notes.md")!;

    await state.openEntry(file.reference.id);
    expect(dataSource.opened).toEqual(["explora-notes.md"]);
    expect(state.previewOpen).toBe(false);

    state.selectEntry(file.reference.id);
    await state.openPreview();
    expect(state.preview?.entryId).toBe(file.reference.id);
  });

  it("requires confirmation before downloading a large remote file", async () => {
    const dataSource = new RecordingNativeOpenDataSource();
    const state = new ExplorerState(dataSource);
    await state.initialize();
    await state.selectLocation("staging-box");
    const file = state.entries.find(({ name }) => name === "README.md")!;
    file.size = (300 * 1024 * 1024).toString();

    await state.openEntry(file.reference.id);
    expect(state.pendingNativeOpenConfirmation?.entry.name).toBe("README.md");
    expect(state.nativeOpenOperations).toHaveLength(0);

    await state.confirmNativeOpen();
    expect(state.pendingNativeOpenConfirmation).toBeNull();
    expect(dataSource.opened).toEqual(["README.md", "README.md"]);
  });

  it("requires confirmation when a remote file's size is unknown", async () => {
    const dataSource = new RecordingNativeOpenDataSource();
    const state = new ExplorerState(dataSource);
    await state.initialize();
    await state.selectLocation("staging-box");
    const file = state.entries.find(({ name }) => name === "README.md")!;
    file.size = null;

    await state.openEntry(file.reference.id);

    expect(state.pendingNativeOpenConfirmation).toMatchObject({
      entry: { name: "README.md" },
      size: null,
    });
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
