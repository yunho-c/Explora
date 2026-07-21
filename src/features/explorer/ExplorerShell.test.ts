import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { describe, expect, it } from "vitest";

import { ExplorerState } from "../../app/explorer-state.svelte";
import { DemoExplorerDataSource } from "$lib/data/demo-explorer-data-source";

import ExplorerShell from "./ExplorerShell.svelte";

describe("ExplorerShell", () => {
  it("renders the loaded shell and switches between list and grid views", async () => {
    const state = new ExplorerState(new DemoExplorerDataSource());
    await state.initialize();
    render(ExplorerShell, { state });

    expect(
      screen.getByRole("main", { name: "File explorer" }),
    ).toBeInTheDocument();
    expect(screen.getByText("explora-notes.md")).toBeInTheDocument();

    await fireEvent.click(screen.getByRole("button", { name: "Grid view" }));
    expect(screen.getByRole("grid", { name: "Files" })).toBeInTheDocument();
  });

  it("uses distinct semantic icons for default favorite folders", async () => {
    const state = new ExplorerState(new DemoExplorerDataSource());
    await state.initialize();
    render(ExplorerShell, { state });

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
    render(ExplorerShell, { state });

    await fireEvent.click(screen.getByText("explora-notes.md"));
    await fireEvent.keyDown(window, { key: " " });

    expect(await screen.findByRole("dialog")).toBeInTheDocument();
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
    render(ExplorerShell, { state });

    await fireEvent.click(screen.getByText("summer-light.jpg"));
    await fireEvent.keyDown(window, { key: " " });

    expect(
      await screen.findByRole("img", {
        name: "Preview of summer-light.jpg",
      }),
    ).toBeInTheDocument();
  });

  it("opens folders on double-click and returns with the Up action", async () => {
    const state = new ExplorerState(new DemoExplorerDataSource());
    await state.initialize();
    render(ExplorerShell, { state });

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
});
