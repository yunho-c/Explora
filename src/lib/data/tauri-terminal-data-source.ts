import { Channel, invoke } from "@tauri-apps/api/core";

import type {
  TerminalCloseReason,
  TerminalError,
  TerminalEvent,
  TerminalExitReason,
  TerminalLaunchContext,
  TerminalSessionKind,
  TerminalSessionState,
  TerminalSessionSummary,
  TerminalSize,
} from "$lib/contracts/terminal";
import type {
  CreateTerminalOptions,
  TerminalDataSource,
} from "$lib/data/terminal-data-source";

const OUTPUT_FRAME_VERSION = 1;
const OUTPUT_FRAME_TYPE = 1;
const OUTPUT_FRAME_HEADER_BYTES = 10;
const terminalStates = new Set<TerminalSessionState>([
  "starting",
  "running",
  "exited",
  "failed",
  "closing",
]);
const terminalKinds = new Set<TerminalSessionKind>(["local", "ssh"]);
const terminalExitReasons = new Set<TerminalExitReason>([
  "completed",
  "terminated",
  "transportClosed",
]);
const terminalErrorCodes = new Set<TerminalError["code"]>([
  "invalidReference",
  "notFound",
  "permissionDenied",
  "notDirectory",
  "cancelled",
  "offline",
  "authenticationFailed",
  "hostKeyFailure",
  "unsupported",
  "invalidConfiguration",
  "unexpected",
]);

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const requireString = (
  record: Record<string, unknown>,
  key: string,
): string => {
  const value = record[key];
  if (typeof value !== "string") {
    throw new Error(`Invalid terminal response: ${key} must be a string.`);
  }
  return value;
};

const parseSummary = (value: unknown): TerminalSessionSummary => {
  if (!isRecord(value)) {
    throw new Error("Invalid terminal response: session summary is malformed.");
  }
  const state = requireString(value, "state");
  const kind = requireString(value, "kind");
  if (!terminalStates.has(state as TerminalSessionState)) {
    throw new Error(
      `Invalid terminal response: unknown session state ${state}.`,
    );
  }
  if (!terminalKinds.has(kind as TerminalSessionKind)) {
    throw new Error(`Invalid terminal response: unknown session kind ${kind}.`);
  }
  return {
    id: requireString(value, "id"),
    state: state as TerminalSessionState,
    kind: kind as TerminalSessionKind,
    locationId: requireString(value, "locationId"),
    title: requireString(value, "title"),
    contextLabel: requireString(value, "contextLabel"),
  };
};

const parseError = (value: unknown): TerminalError => {
  if (!isRecord(value)) {
    throw new Error("Invalid terminal response: error is malformed.");
  }
  const code = requireString(value, "code");
  if (!terminalErrorCodes.has(code as TerminalError["code"])) {
    throw new Error(`Invalid terminal response: unknown error code ${code}.`);
  }
  return {
    code: code as TerminalError["code"],
    message: requireString(value, "message"),
  };
};

const parseControlEvent = (value: unknown): TerminalEvent => {
  if (!isRecord(value)) {
    throw new Error("Invalid terminal response: control event is malformed.");
  }
  if (value.event === "started") {
    return { event: "started", session: parseSummary(value.session) };
  }
  if (value.event === "failed") {
    return { event: "failed", error: parseError(value.error) };
  }
  if (value.event === "exited") {
    const reason = requireString(value, "reason");
    if (!terminalExitReasons.has(reason as TerminalExitReason)) {
      throw new Error(
        `Invalid terminal response: unknown exit reason ${reason}.`,
      );
    }
    if (
      value.exitCode !== null &&
      (!Number.isSafeInteger(value.exitCode) || Number(value.exitCode) < 0)
    ) {
      throw new Error("Invalid terminal response: exit code is malformed.");
    }
    if (value.signal !== null && typeof value.signal !== "string") {
      throw new Error("Invalid terminal response: exit signal is malformed.");
    }
    return {
      event: "exited",
      exitCode: value.exitCode as number | null,
      signal: value.signal,
      reason: reason as TerminalExitReason,
    };
  }
  throw new Error("Invalid terminal response: unknown control event.");
};

const parseOutputFrame = (value: ArrayBuffer | Uint8Array): TerminalEvent => {
  const bytes =
    value instanceof Uint8Array
      ? value
      : new Uint8Array(value, 0, value.byteLength);
  if (
    bytes.byteLength < OUTPUT_FRAME_HEADER_BYTES ||
    bytes[0] !== OUTPUT_FRAME_VERSION ||
    bytes[1] !== OUTPUT_FRAME_TYPE
  ) {
    throw new Error("Invalid terminal response: output frame is malformed.");
  }
  const sequenceView = new DataView(bytes.buffer, bytes.byteOffset + 2, 8);
  const sequenceHigh = sequenceView.getUint32(0);
  const sequenceLow = sequenceView.getUint32(4);
  if (sequenceHigh > 0x1f_ffff) {
    throw new Error("Invalid terminal response: output sequence is too large.");
  }
  const sequence = sequenceHigh * 0x1_0000_0000 + sequenceLow;
  return {
    event: "output",
    sequence,
    bytes: bytes.slice(OUTPUT_FRAME_HEADER_BYTES),
  };
};

export const parseTerminalWireEvent = (value: unknown): TerminalEvent => {
  if (value instanceof ArrayBuffer || value instanceof Uint8Array) {
    return parseOutputFrame(value);
  }
  return parseControlEvent(value);
};

const normalizedSize = (size: TerminalSize): TerminalSize => ({
  columns: size.columns,
  rows: size.rows,
  pixelWidth: size.pixelWidth,
  pixelHeight: size.pixelHeight,
});

const abortError = () =>
  new DOMException("The request was cancelled.", "AbortError");

export class TauriTerminalDataSource implements TerminalDataSource {
  private readonly channels = new Map<string, Channel<unknown>>();

  async createTerminal(
    context: TerminalLaunchContext,
    size: TerminalSize,
    options: CreateTerminalOptions,
  ): Promise<TerminalSessionSummary> {
    if (options.signal.aborted) throw abortError();
    const requestId = crypto.randomUUID();
    const channel = new Channel<unknown>();
    let sessionId: string | null = null;
    let protocolError: Error | null = null;
    channel.onmessage = (value) => {
      let event: TerminalEvent;
      try {
        event = parseTerminalWireEvent(value);
      } catch (error) {
        protocolError =
          error instanceof Error
            ? error
            : new Error("Invalid terminal response.");
        options.onEvent({
          event: "failed",
          error: { code: "unexpected", message: protocolError.message },
        });
        if (sessionId) void this.closeTerminal(sessionId, "channelClosed");
        return;
      }
      if (event.event === "started") sessionId = event.session.id;
      options.onEvent(event);
    };

    const summary = parseSummary(
      await invoke<unknown>("create_terminal", {
        requestId,
        locationId: context.locationId,
        directoryId: context.directoryId,
        size: normalizedSize(size),
        onEvent: channel,
      }),
    );
    sessionId = summary.id;
    this.channels.set(summary.id, channel);
    if (protocolError) {
      await this.closeTerminal(summary.id, "channelClosed");
      throw protocolError;
    }
    if (options.signal.aborted) {
      await this.closeTerminal(summary.id, "user");
      throw abortError();
    }
    return summary;
  }

  async writeTerminal(
    sessionId: string,
    inputSequence: number,
    bytes: Uint8Array,
  ): Promise<void> {
    await invoke("write_terminal", {
      sessionId,
      inputSequence,
      bytes: Array.from(bytes),
    });
  }

  async resizeTerminal(sessionId: string, size: TerminalSize): Promise<void> {
    await invoke("resize_terminal", {
      sessionId,
      size: normalizedSize(size),
    });
  }

  async acknowledgeOutput(
    sessionId: string,
    outputSequence: number,
  ): Promise<void> {
    await invoke("acknowledge_terminal_output", {
      sessionId,
      outputSequence,
    });
  }

  async closeTerminal(
    sessionId: string,
    reason: TerminalCloseReason,
  ): Promise<void> {
    try {
      await invoke("close_terminal", { sessionId, reason });
    } finally {
      this.channels.delete(sessionId);
    }
  }
}
