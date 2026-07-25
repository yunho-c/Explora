import type {
  TerminalCloseReason,
  TerminalEvent,
  TerminalLaunchContext,
  TerminalSessionSummary,
  TerminalSize,
} from "$lib/contracts/terminal";

export interface CreateTerminalOptions {
  signal: AbortSignal;
  onEvent: (event: TerminalEvent) => void;
}

export interface TerminalDataSource {
  createTerminal(
    context: TerminalLaunchContext,
    size: TerminalSize,
    options: CreateTerminalOptions,
  ): Promise<TerminalSessionSummary>;
  writeTerminal(
    sessionId: string,
    inputSequence: number,
    bytes: Uint8Array,
  ): Promise<void>;
  resizeTerminal(sessionId: string, size: TerminalSize): Promise<void>;
  acknowledgeOutput(sessionId: string, outputSequence: number): Promise<void>;
  closeTerminal(sessionId: string, reason: TerminalCloseReason): Promise<void>;
}
