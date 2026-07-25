import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { describe, expect, it, vi } from "vitest";

import { TerminalState } from "../../app/terminal-state.svelte";
import { DemoTerminalDataSource } from "$lib/data/demo-terminal-data-source";

import TerminalPane from "./TerminalPane.svelte";

vi.mock("./xterm-adapter", () => ({
  XtermAdapter: class {
    constructor() {}
    write(_bytes: Uint8Array, consumed: () => void) {
      consumed();
    }
    focus() {}
    fit() {}
    setPreferences() {}
    dispose() {}
  },
}));

const createState = () =>
  new TerminalState(new DemoTerminalDataSource(), () => ({
    locationId: "home",
    directoryId: "directory-token",
    kind: "local",
    locationLabel: "Home",
    directoryLabel: "/Users/test/Projects",
  }));

describe("TerminalPane", () => {
  it("renders accessible session chrome and confirms paste and close actions", async () => {
    const state = createState();
    await state.newTerminal();
    render(TerminalPane, { state });

    expect(
      screen.getByRole("region", { name: "Integrated terminal" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("tablist", { name: "Terminal sessions" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("tab")).toHaveAttribute("aria-selected", "true");
    expect(
      screen.getByRole("button", { name: "New terminal" }),
    ).toBeInTheDocument();

    await fireEvent.click(
      screen.getByRole("button", { name: "Terminal actions" }),
    );
    await fireEvent.click(
      await screen.findByRole("menuitem", { name: "Rename Terminal" }),
    );
    const rename = await screen.findByRole("textbox", {
      name: /Rename .* terminal/,
    });
    await fireEvent.input(rename, { target: { value: "Build" } });
    await fireEvent.keyDown(rename, { key: "Enter" });
    expect(screen.getByRole("tab", { name: "Build" })).toBeInTheDocument();

    state.requestPaste(state.activeSessionId!, "echo one\necho two\n");
    expect(
      await screen.findByRole("dialog", { name: "Paste multiple lines?" }),
    ).toHaveTextContent("/Users/test/Projects");
    await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(state.pendingPaste).toBeNull());

    await fireEvent.click(
      screen.getByRole("button", { name: "Close Build terminal" }),
    );
    expect(
      await screen.findByRole("dialog", { name: "Close this terminal?" }),
    ).toHaveTextContent("/Users/test/Projects");
    await fireEvent.click(
      screen.getByRole("button", { name: "Close terminal" }),
    );
    await waitFor(() => expect(state.sessions).toHaveLength(0));
  });
});
