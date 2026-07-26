import { afterEach, describe, expect, it, vi } from "vitest";

import type {
  TerminalCloseReason,
  TerminalEvent,
  TerminalLaunchContext,
  TerminalSessionSummary,
  TerminalSize,
} from "$lib/contracts/terminal";
import type {
  CreateTerminalOptions,
  TerminalDataSource,
} from "$lib/data/terminal-data-source";
import { defaultUserPreferences } from "$lib/contracts/preferences";
import { MemoryPreferencesDataSource } from "$lib/data/memory-preferences-data-source";

import { TerminalState } from "./terminal-state.svelte";

const context: TerminalLaunchContext = {
  locationId: "home",
  directoryId: "directory-token",
  kind: "local",
  locationLabel: "Home",
  directoryLabel: "/Users/test/Projects",
};
const summary: TerminalSessionSummary = {
  id: "terminal-1",
  state: "running",
  kind: "local",
  locationId: "home",
  title: "Projects",
  contextLabel: "/Users/test/Projects",
};

class FakeTerminalDataSource implements TerminalDataSource {
  onEvent: ((event: TerminalEvent) => void) | null = null;
  writes: Array<{ sequence: number; bytes: Uint8Array }> = [];
  resizes: TerminalSize[] = [];
  acknowledgements: number[] = [];
  closes: TerminalCloseReason[] = [];
  createCount = 0;

  async createTerminal(
    _context: TerminalLaunchContext,
    _size: TerminalSize,
    options: CreateTerminalOptions,
  ): Promise<TerminalSessionSummary> {
    this.onEvent = options.onEvent;
    const created = {
      ...summary,
      id: `terminal-${++this.createCount}`,
    };
    options.onEvent({ event: "started", session: created });
    return created;
  }

  async writeTerminal(
    _sessionId: string,
    inputSequence: number,
    bytes: Uint8Array,
  ): Promise<void> {
    this.writes.push({ sequence: inputSequence, bytes: bytes.slice() });
  }

  async resizeTerminal(_sessionId: string, size: TerminalSize): Promise<void> {
    this.resizes.push(size);
  }

  async acknowledgeOutput(
    _sessionId: string,
    outputSequence: number,
  ): Promise<void> {
    this.acknowledgements.push(outputSequence);
  }

  async closeTerminal(
    _sessionId: string,
    reason: TerminalCloseReason,
  ): Promise<void> {
    this.closes.push(reason);
  }
}

afterEach(() => {
  vi.useRealTimers();
});

describe("TerminalState", () => {
  it("enforces the shared per-window session limit before invoking the backend", async () => {
    const dataSource = new FakeTerminalDataSource();
    const state = new TerminalState(dataSource, () => context);
    for (let index = 0; index < 6; index += 1) {
      await state.newTerminal();
    }
    await state.newTerminal();

    expect(state.sessions).toHaveLength(6);
    expect(dataSource.createCount).toBe(6);
    expect(state.errorMessage).toContain("at most 6");
  });

  it("restores and persists bounded presentation preferences", async () => {
    vi.useFakeTimers();
    const preferences = defaultUserPreferences();
    preferences.terminal = {
      visible: true,
      paneHeightPercent: 40,
      fontSize: 15,
      scrollback: 10_000,
      screenReaderMode: false,
    };
    const preferencesDataSource = new MemoryPreferencesDataSource(preferences);
    const state = new TerminalState(
      new FakeTerminalDataSource(),
      () => context,
      preferencesDataSource,
    );

    await state.initializePreferences();
    expect(state.visible).toBe(true);
    expect(state.paneHeightPercent).toBe(40);
    expect(state.fontSize).toBe(15);
    expect(state.scrollback).toBe(10_000);
    expect(state.screenReaderMode).toBe(false);

    state.setPaneHeightPercent(99);
    state.setFontSize(8);
    state.setScrollback(80_000);
    state.setScreenReaderMode(true);
    await vi.advanceTimersByTimeAsync(250);
    await vi.waitFor(async () => {
      const snapshot = await preferencesDataSource.getPreferences();
      expect(snapshot.preferences.terminal).toEqual({
        visible: true,
        paneHeightPercent: 70,
        fontSize: 10,
        scrollback: 50_000,
        screenReaderMode: true,
      });
    });
  });

  it("keeps multiple sessions independent and sanitizes presentation-only names", async () => {
    const dataSource = new FakeTerminalDataSource();
    const state = new TerminalState(dataSource, () => context);
    await state.newTerminal();
    await state.newTerminal();
    expect(state.sessions.map(({ id }) => id)).toEqual([
      "terminal-1",
      "terminal-2",
    ]);
    expect(state.activeSessionId).toBe("terminal-2");

    state.renameSession("terminal-2", `\u001bBuild ${"x".repeat(80)}`);
    expect(state.activeSession?.title.startsWith("Build ")).toBe(true);
    expect(Array.from(state.activeSession!.title)).toHaveLength(64);
    state.selectRelativeSession(-1);
    expect(state.activeSessionId).toBe("terminal-1");
    expect(state.sessions[1].title.startsWith("Build ")).toBe(true);
  });

  it("creates on first toggle and hides without stopping the session", async () => {
    const dataSource = new FakeTerminalDataSource();
    const state = new TerminalState(dataSource, () => context);

    state.toggleVisibility();
    await vi.waitFor(() => expect(state.sessions).toHaveLength(1));
    expect(state.visible).toBe(true);
    state.toggleVisibility();
    expect(state.visible).toBe(false);
    expect(state.sessions[0].state).toBe("running");
    expect(dataSource.closes).toHaveLength(0);
  });

  it("keeps lifecycle separate, buffers output until mounted, and ignores stale events", async () => {
    const dataSource = new FakeTerminalDataSource();
    const state = new TerminalState(dataSource, () => context);
    await state.newTerminal();
    expect(state.sessions).toHaveLength(1);
    expect(state.visible).toBe(true);

    dataSource.onEvent?.({
      event: "output",
      sequence: 0,
      bytes: new Uint8Array([0, 255, 27]),
    });
    const output: TerminalEvent[] = [];
    state.subscribeOutput(summary.id, (event) => output.push(event));
    expect(output).toEqual([
      {
        event: "output",
        sequence: 0,
        bytes: new Uint8Array([0, 255, 27]),
      },
    ]);
    state.acknowledgeOutput(summary.id, 0);
    await vi.waitFor(() => expect(dataSource.acknowledgements).toEqual([0]));

    await state.closeSession(summary.id);
    expect(state.sessions).toHaveLength(0);
    dataSource.onEvent?.({
      event: "failed",
      error: { code: "unexpected", message: "late failure" },
    });
    expect(state.sessions).toHaveLength(0);
    expect(state.statusAnnouncement).toBe("Terminal closed.");
  });

  it("batches ordered text and binary input without lossy conversion", async () => {
    vi.useFakeTimers();
    const dataSource = new FakeTerminalDataSource();
    const state = new TerminalState(dataSource, () => context);
    await state.newTerminal();

    state.sendText(summary.id, "hé");
    state.sendBinaryString(summary.id, String.fromCharCode(0, 255));
    await vi.advanceTimersByTimeAsync(8);
    await vi.waitFor(() => expect(dataSource.writes).toHaveLength(1));
    expect(dataSource.writes[0].sequence).toBe(0);
    expect(Array.from(dataSource.writes[0].bytes)).toEqual([
      104, 195, 169, 0, 255,
    ]);

    state.sendText(summary.id, "x");
    await vi.advanceTimersByTimeAsync(8);
    await vi.waitFor(() => expect(dataSource.writes).toHaveLength(2));
    expect(dataSource.writes[1]).toMatchObject({ sequence: 1 });
  });

  it("requires confirmation before sending multiline paste", async () => {
    vi.useFakeTimers();
    const dataSource = new FakeTerminalDataSource();
    const state = new TerminalState(dataSource, () => context);
    await state.newTerminal();

    state.requestPaste(summary.id, "rm one\nrm two\n");
    expect(state.pendingPaste).toMatchObject({
      sessionId: summary.id,
      lineCount: 3,
      targetLabel: summary.contextLabel,
    });
    await vi.advanceTimersByTimeAsync(20);
    expect(dataSource.writes).toHaveLength(0);

    state.confirmPaste();
    await vi.advanceTimersByTimeAsync(8);
    await vi.waitFor(() => expect(dataSource.writes).toHaveLength(1));
    expect(new TextDecoder().decode(dataSource.writes[0].bytes)).toBe(
      "rm one\nrm two\n",
    );
  });

  it("preserves scrollback state after exit until explicit close", async () => {
    const dataSource = new FakeTerminalDataSource();
    const state = new TerminalState(dataSource, () => context);
    await state.newTerminal();
    dataSource.onEvent?.({
      event: "exited",
      exitCode: 7,
      signal: null,
      reason: "completed",
    });

    expect(state.activeSession).toMatchObject({
      state: "exited",
      exitCode: 7,
      exitReason: "completed",
    });
    expect(state.visible).toBe(true);
  });
});
