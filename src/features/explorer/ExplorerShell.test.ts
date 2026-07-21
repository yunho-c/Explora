import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { describe, expect, it } from "vitest";

import { ExplorerState } from "../../app/explorer-state.svelte";
import {
  WindowChromeController,
  type WindowChromeAdapter,
} from "../../app/window-chrome.svelte";
import { DemoExplorerDataSource } from "$lib/data/demo-explorer-data-source";

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
    expect(titlebarLeft?.textContent?.trim()).toBe("");
    expect(titlebarLeft?.querySelector("svg")).toBeNull();

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
    }
  });

  it("opens Quick Preview with the Space key", async () => {
    const state = new ExplorerState(new DemoExplorerDataSource());
    await state.initialize();
    renderShell(state);

    await fireEvent.click(screen.getByText("explora-notes.md"));
    await fireEvent.keyDown(window, { key: " " });

    expect(await screen.findByRole("dialog")).toBeInTheDocument();
    expect(
      await screen.findByText(
        "Use ↑ and ↓ to move between items · Esc to close",
      ),
    ).toBeInTheDocument();
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
