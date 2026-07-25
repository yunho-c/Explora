import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { TerminalLaunchContext } from "$lib/contracts/terminal";

import {
  parseTerminalWireEvent,
  TauriTerminalDataSource,
} from "./tauri-terminal-data-source";

const summary = {
  id: "session-token",
  state: "running",
  kind: "local",
  locationId: "home",
  title: "Projects",
  contextLabel: "/Users/test/Projects",
};
const context: TerminalLaunchContext = {
  locationId: "home",
  directoryId: "directory-token",
  kind: "local",
  locationLabel: "Home",
  directoryLabel: "/Users/test/Projects",
};

const sendChannelMessages = (
  channel: unknown,
  messages: readonly unknown[],
) => {
  const toJson =
    typeof channel === "object" && channel !== null
      ? Reflect.get(channel, "toJSON")
      : undefined;
  const serialized =
    typeof toJson === "function"
      ? Reflect.apply(toJson, channel, [])
      : String(channel);
  const match = /^__CHANNEL__:(\d+)$/.exec(String(serialized));
  if (!match) throw new Error("Expected a Tauri channel identifier.");
  const internals = (
    window as unknown as {
      __TAURI_INTERNALS__: {
        runCallback: (id: number, payload: unknown) => void;
      };
    }
  ).__TAURI_INTERNALS__;
  messages.forEach((message, index) => {
    internals.runCallback(Number(match[1]), { index, message });
  });
};

const outputFrame = (sequence: number, payload: readonly number[]) => {
  const frame = new Uint8Array(10 + payload.length);
  frame[0] = 1;
  frame[1] = 1;
  const view = new DataView(frame.buffer);
  view.setUint32(2, Math.floor(sequence / 0x1_0000_0000));
  view.setUint32(6, sequence >>> 0);
  frame.set(payload, 10);
  return frame.buffer;
};

afterEach(() => {
  clearMocks();
  vi.restoreAllMocks();
});

describe("TauriTerminalDataSource", () => {
  it("preserves binary output frames and sends the narrow typed command shape", async () => {
    const calls: Array<{ command: string; payload: unknown }> = [];
    mockIPC((command, payload) => {
      calls.push({ command, payload });
      if (command === "create_terminal") {
        if (
          !payload ||
          Array.isArray(payload) ||
          payload instanceof ArrayBuffer ||
          payload instanceof Uint8Array
        ) {
          throw new Error("missing payload");
        }
        sendChannelMessages(payload.onEvent, [
          { event: "started", session: summary },
          outputFrame(42, [0, 255, 27, 91, 65]),
        ]);
        return summary;
      }
      return null;
    });
    const events: unknown[] = [];
    const source = new TauriTerminalDataSource();
    const created = await source.createTerminal(
      context,
      { columns: 80, rows: 24, pixelWidth: null, pixelHeight: null },
      {
        signal: new AbortController().signal,
        onEvent: (event) => events.push(event),
      },
    );
    await source.writeTerminal(created.id, 0, new Uint8Array([0, 255]));
    await source.resizeTerminal(created.id, {
      columns: 120,
      rows: 40,
      pixelWidth: 900,
      pixelHeight: 600,
    });
    await source.acknowledgeOutput(created.id, 42);
    await source.closeTerminal(created.id, "user");

    expect(events).toEqual([
      { event: "started", session: summary },
      {
        event: "output",
        sequence: 42,
        bytes: new Uint8Array([0, 255, 27, 91, 65]),
      },
    ]);
    expect(calls.map(({ command }) => command)).toEqual([
      "create_terminal",
      "write_terminal",
      "resize_terminal",
      "acknowledge_terminal_output",
      "close_terminal",
    ]);
    expect(calls[1].payload).toEqual({
      sessionId: "session-token",
      inputSequence: 0,
      bytes: [0, 255],
    });
  });

  it("rejects malformed control and binary events", () => {
    expect(() =>
      parseTerminalWireEvent({
        event: "started",
        session: { ...summary, state: "teleporting" },
      }),
    ).toThrow("unknown session state");
    expect(() => parseTerminalWireEvent(new Uint8Array([1, 1]).buffer)).toThrow(
      "output frame is malformed",
    );
    expect(() =>
      parseTerminalWireEvent(outputFrame(Number.MAX_SAFE_INTEGER + 1, [1])),
    ).toThrow("output sequence is too large");
  });
});
