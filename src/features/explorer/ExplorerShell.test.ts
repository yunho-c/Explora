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

  it("opens Quick Preview with the Space key", async () => {
    const state = new ExplorerState(new DemoExplorerDataSource());
    await state.initialize();
    render(ExplorerShell, { state });

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
