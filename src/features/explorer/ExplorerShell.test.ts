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
import { MemoryPreferencesDataSource } from "$lib/data/memory-preferences-data-source";

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
    for (const header of ["Favorites", "SSH"]) {
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
