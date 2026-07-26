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

interface DemoSession {
  summary: TerminalSessionSummary;
  onEvent: (event: TerminalEvent) => void;
  nextOutputSequence: number;
}

export class DemoTerminalDataSource implements TerminalDataSource {
  private readonly sessions = new Map<string, DemoSession>();
  private sequence = 0;

  async createTerminal(
    context: TerminalLaunchContext,
    _size: TerminalSize,
    options: CreateTerminalOptions,
  ): Promise<TerminalSessionSummary> {
    if (options.signal.aborted) {
      throw new DOMException("The request was cancelled.", "AbortError");
    }
    const summary: TerminalSessionSummary = {
      id: `demo-terminal-${++this.sequence}`,
      state: "running",
      kind: context.kind,
      locationId: context.locationId,
      title: context.directoryLabel || "Terminal",
      contextLabel:
        context.kind === "ssh"
          ? `${context.locationLabel} · server home`
          : context.directoryLabel,
    };
    const session: DemoSession = {
      summary,
      onEvent: options.onEvent,
      nextOutputSequence: 0,
    };
    this.sessions.set(summary.id, session);
    queueMicrotask(() => {
      if (!this.sessions.has(summary.id)) return;
      options.onEvent({ event: "started", session: summary });
      this.emitOutput(
        session,
        new TextEncoder().encode(
          "\u001b[2mExplora browser demo — native PTY behavior requires the Tauri app.\u001b[0m\r\n$ ",
        ),
      );
    });
    return summary;
  }

  async writeTerminal(
    sessionId: string,
    _inputSequence: number,
    bytes: Uint8Array,
  ): Promise<void> {
    const session = this.sessions.get(sessionId);
    if (!session)
      throw new Error("This demo terminal session is no longer available.");
    this.emitOutput(session, bytes.slice());
    if (new TextDecoder().decode(bytes).trim() === "exit") {
      session.onEvent({
        event: "exited",
        exitCode: 0,
        signal: null,
        reason: "completed",
      });
    }
  }

  async resizeTerminal(sessionId: string, size: TerminalSize): Promise<void> {
    void sessionId;
    void size;
  }

  async acknowledgeOutput(
    sessionId: string,
    outputSequence: number,
  ): Promise<void> {
    void sessionId;
    void outputSequence;
  }

  async closeTerminal(
    sessionId: string,
    reason: TerminalCloseReason,
  ): Promise<void> {
    void reason;
    this.sessions.delete(sessionId);
  }

  private emitOutput(session: DemoSession, bytes: Uint8Array) {
    session.onEvent({
      event: "output",
      sequence: session.nextOutputSequence++,
      bytes,
    });
  }
}
