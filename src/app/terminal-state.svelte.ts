import type {
  TerminalEvent,
  TerminalLaunchContext,
  TerminalSessionSummary,
  TerminalSessionState,
  TerminalSize,
} from "$lib/contracts/terminal";
import type {
  TerminalPreferences,
  TerminalPreferencesPatch,
} from "$lib/contracts/preferences";
import { MemoryPreferencesDataSource } from "$lib/data/memory-preferences-data-source";
import type { PreferencesDataSource } from "$lib/data/preferences-data-source";
import type { TerminalDataSource } from "$lib/data/terminal-data-source";
import { SvelteMap, SvelteSet } from "svelte/reactivity";

const DEFAULT_SIZE: TerminalSize = {
  columns: 80,
  rows: 24,
  pixelWidth: null,
  pixelHeight: null,
};
const INPUT_BATCH_BYTES = 16 * 1024;
const INPUT_BATCH_DELAY_MS = 8;
const PREFERENCE_WRITE_DELAY_MS = 250;
export const MAX_TERMINAL_SESSIONS_PER_WINDOW = 6;

export interface TerminalSessionView extends TerminalSessionSummary {
  exitCode: number | null;
  exitReason: string | null;
  errorMessage: string | null;
}

export interface PendingTerminalPaste {
  sessionId: string;
  targetLabel: string;
  text: string;
  preview: string;
  lineCount: number;
}

type OutputEvent = Extract<TerminalEvent, { event: "output" }>;
type OutputSubscriber = (event: OutputEvent) => void;

interface InputQueue {
  pending: Uint8Array[];
  pendingBytes: number;
  nextSequence: number;
  timer: ReturnType<typeof setTimeout> | null;
  chain: Promise<void>;
  failed: boolean;
}

export class TerminalState {
  sessions = $state<TerminalSessionView[]>([]);
  activeSessionId = $state<string | null>(null);
  visible = $state(false);
  creating = $state(false);
  errorMessage = $state<string | null>(null);
  statusAnnouncement = $state("");
  pendingPaste = $state<PendingTerminalPaste | null>(null);
  focusRequest = $state(0);
  paneHeightPercent = $state(32);
  fontSize = $state(13);
  scrollback = $state(5_000);
  screenReaderMode = $state(true);
  preferencesWarningMessage = $state<string | null>(null);

  private readonly launchContexts = new SvelteMap<
    string,
    TerminalLaunchContext
  >();
  private readonly outputSubscribers = new SvelteMap<
    string,
    OutputSubscriber
  >();
  private readonly pendingOutput = new SvelteMap<string, OutputEvent[]>();
  private readonly inputQueues = new SvelteMap<string, InputQueue>();
  private readonly lastSizes = new SvelteMap<string, TerminalSize>();
  private readonly closedSessionIds = new SvelteSet<string>();
  private readonly createControllers = new SvelteSet<AbortController>();
  private preferenceWriteQueue = Promise.resolve();
  private paneHeightWriteTimer: ReturnType<typeof setTimeout> | null = null;
  private preferencesInitialization: Promise<void> | null = null;
  private disposed = false;

  constructor(
    private readonly dataSource: TerminalDataSource,
    private readonly currentLaunchContext: () => TerminalLaunchContext | null,
    private readonly preferencesDataSource: PreferencesDataSource = new MemoryPreferencesDataSource(),
  ) {}

  get activeSession(): TerminalSessionView | undefined {
    return this.sessions.find(({ id }) => id === this.activeSessionId);
  }

  initializePreferences(): Promise<void> {
    this.preferencesInitialization ??= this.loadPreferences();
    return this.preferencesInitialization;
  }

  async newTerminal(context = this.currentLaunchContext()): Promise<void> {
    if (this.disposed || this.creating) return;
    if (this.sessions.length >= MAX_TERMINAL_SESSIONS_PER_WINDOW) {
      this.errorMessage = `A window can have at most ${MAX_TERMINAL_SESSIONS_PER_WINDOW} terminal sessions.`;
      return;
    }
    if (!context) {
      this.errorMessage = "Open a location before creating a terminal.";
      return;
    }

    this.creating = true;
    this.setVisible(true);
    this.errorMessage = null;
    const controller = new AbortController();
    this.createControllers.add(controller);
    let sessionId: string | null = null;
    const earlyEvents: TerminalEvent[] = [];
    const receive = (event: TerminalEvent) => {
      if (event.event === "started") {
        sessionId = event.session.id;
        this.applyEvent(sessionId, event);
        for (const earlyEvent of earlyEvents.splice(0)) {
          this.applyEvent(sessionId, earlyEvent);
        }
      } else if (sessionId) {
        this.applyEvent(sessionId, event);
      } else {
        earlyEvents.push(event);
      }
    };

    try {
      const summary = await this.dataSource.createTerminal(
        context,
        DEFAULT_SIZE,
        {
          signal: controller.signal,
          onEvent: receive,
        },
      );
      sessionId = summary.id;
      this.upsertSession(summary);
      this.launchContexts.set(summary.id, context);
      this.ensureInputQueue(summary.id);
      for (const earlyEvent of earlyEvents.splice(0)) {
        this.applyEvent(summary.id, earlyEvent);
      }
      this.activeSessionId = summary.id;
      this.statusAnnouncement = `Terminal started in ${summary.contextLabel}.`;
      this.requestFocus();
    } catch (error) {
      if (!controller.signal.aborted) {
        this.errorMessage = terminalErrorMessage(error);
        this.statusAnnouncement = `Terminal failed to start. ${this.errorMessage}`;
      }
      if (this.sessions.length === 0) this.setVisible(false);
    } finally {
      this.createControllers.delete(controller);
      this.creating = false;
    }
  }

  async restartSession(sessionId: string): Promise<void> {
    const context = this.launchContexts.get(sessionId);
    if (!context) return;
    await this.closeSession(sessionId, "restart");
    await this.newTerminal(context);
  }

  toggleVisibility(): void {
    if (this.sessions.length === 0 && !this.creating) {
      void this.newTerminal();
      return;
    }
    this.setVisible(!this.visible);
    if (this.visible) this.requestFocus();
  }

  showAndFocus(): void {
    if (this.sessions.length === 0) {
      void this.newTerminal();
      return;
    }
    this.setVisible(true);
    this.requestFocus();
  }

  hide(): void {
    this.setVisible(false);
  }

  selectSession(sessionId: string): void {
    if (!this.sessions.some(({ id }) => id === sessionId)) return;
    this.activeSessionId = sessionId;
    this.setVisible(true);
    this.requestFocus();
  }

  selectRelativeSession(delta: number): void {
    if (this.sessions.length < 2) return;
    const current = this.sessions.findIndex(
      ({ id }) => id === this.activeSessionId,
    );
    const next =
      (Math.max(current, 0) + delta + this.sessions.length) %
      this.sessions.length;
    this.selectSession(this.sessions[next].id);
  }

  renameSession(sessionId: string, requestedTitle: string): void {
    const session = this.sessions.find(({ id }) => id === sessionId);
    if (!session) return;
    const title = Array.from(requestedTitle)
      .filter((character) => {
        const codePoint = character.codePointAt(0) ?? 0;
        return codePoint >= 32 && codePoint !== 127;
      })
      .join("")
      .trim();
    const boundedTitle = Array.from(title).slice(0, 64).join("");
    if (boundedTitle) session.title = boundedTitle;
  }

  setPaneHeightPercent(value: number): void {
    const paneHeightPercent = Math.min(Math.max(Math.round(value), 20), 70);
    if (paneHeightPercent === this.paneHeightPercent) return;
    this.paneHeightPercent = paneHeightPercent;
    if (this.paneHeightWriteTimer !== null) {
      clearTimeout(this.paneHeightWriteTimer);
    }
    this.paneHeightWriteTimer = setTimeout(() => {
      this.paneHeightWriteTimer = null;
      this.persistTerminalPreferences({ paneHeightPercent });
    }, PREFERENCE_WRITE_DELAY_MS);
  }

  setFontSize(value: number): void {
    const fontSize = Math.min(Math.max(Math.round(value), 10), 24);
    if (fontSize === this.fontSize) return;
    this.fontSize = fontSize;
    this.persistTerminalPreferences({ fontSize });
  }

  setScrollback(value: number): void {
    const scrollback = Math.min(Math.max(Math.round(value), 1_000), 50_000);
    if (scrollback === this.scrollback) return;
    this.scrollback = scrollback;
    this.persistTerminalPreferences({ scrollback });
  }

  setScreenReaderMode(screenReaderMode: boolean): void {
    if (screenReaderMode === this.screenReaderMode) return;
    this.screenReaderMode = screenReaderMode;
    this.persistTerminalPreferences({ screenReaderMode });
  }

  requestFocus(): void {
    this.focusRequest += 1;
  }

  subscribeOutput(sessionId: string, subscriber: OutputSubscriber): () => void {
    this.outputSubscribers.set(sessionId, subscriber);
    const pending = this.pendingOutput.get(sessionId);
    if (pending) {
      this.pendingOutput.delete(sessionId);
      for (const event of pending) subscriber(event);
    }
    return () => {
      if (this.outputSubscribers.get(sessionId) === subscriber) {
        this.outputSubscribers.delete(sessionId);
      }
    };
  }

  sendText(sessionId: string, text: string): void {
    if (!text) return;
    this.enqueueInput(sessionId, new TextEncoder().encode(text));
  }

  sendBinaryString(sessionId: string, value: string): void {
    if (!value) return;
    const bytes = Uint8Array.from(value, (character) =>
      character.charCodeAt(0),
    );
    this.enqueueInput(sessionId, bytes);
  }

  requestPaste(sessionId: string, text: string): void {
    if (!text) return;
    const lines = text.split(/\r\n|\r|\n/);
    if (lines.length === 1) {
      this.sendText(sessionId, text);
      return;
    }
    const session = this.sessions.find(({ id }) => id === sessionId);
    if (!session) return;
    const previewLines = lines.slice(0, 4);
    const preview = previewLines
      .map((line) => line.slice(0, 160))
      .join("\n")
      .concat(lines.length > previewLines.length ? "\n…" : "");
    this.pendingPaste = {
      sessionId,
      targetLabel: session.contextLabel,
      text,
      preview,
      lineCount: lines.length,
    };
  }

  confirmPaste(): void {
    const pending = this.pendingPaste;
    this.pendingPaste = null;
    if (pending) this.sendText(pending.sessionId, pending.text);
  }

  cancelPaste(): void {
    this.pendingPaste = null;
  }

  async resize(sessionId: string, size: TerminalSize): Promise<void> {
    const session = this.sessions.find(({ id }) => id === sessionId);
    if (!session || session.state !== "running") return;
    const previous = this.lastSizes.get(sessionId);
    if (
      previous &&
      previous.columns === size.columns &&
      previous.rows === size.rows &&
      previous.pixelWidth === size.pixelWidth &&
      previous.pixelHeight === size.pixelHeight
    ) {
      return;
    }
    this.lastSizes.set(sessionId, size);
    try {
      await this.dataSource.resizeTerminal(sessionId, size);
    } catch (error) {
      this.errorMessage = terminalErrorMessage(error);
    }
  }

  acknowledgeOutput(sessionId: string, sequence: number): void {
    void this.dataSource
      .acknowledgeOutput(sessionId, sequence)
      .catch((error) => {
        if (!this.closedSessionIds.has(sessionId)) {
          this.errorMessage = terminalErrorMessage(error);
        }
      });
  }

  async closeSession(
    sessionId: string,
    reason: "user" | "restart" | "applicationExit" = "user",
  ): Promise<void> {
    const session = this.sessions.find(({ id }) => id === sessionId);
    if (!session) return;
    const previousState = session.state;
    session.state = "closing";
    this.clearInputQueue(sessionId);
    try {
      await this.dataSource.closeTerminal(sessionId, reason);
      this.closedSessionIds.add(sessionId);
      this.sessions = this.sessions.filter(({ id }) => id !== sessionId);
      this.launchContexts.delete(sessionId);
      this.pendingOutput.delete(sessionId);
      this.outputSubscribers.delete(sessionId);
      this.lastSizes.delete(sessionId);
      if (this.activeSessionId === sessionId) {
        this.activeSessionId = this.sessions.at(-1)?.id ?? null;
      }
      if (this.sessions.length === 0 && reason !== "applicationExit") {
        this.setVisible(false);
      }
      this.statusAnnouncement = "Terminal closed.";
    } catch (error) {
      session.state = previousState;
      session.errorMessage = terminalErrorMessage(error);
      this.errorMessage = session.errorMessage;
    }
  }

  closeAll(): void {
    for (const session of [...this.sessions]) {
      void this.closeSession(session.id);
    }
  }

  dispose(): void {
    this.disposed = true;
    if (this.paneHeightWriteTimer !== null) {
      clearTimeout(this.paneHeightWriteTimer);
      this.paneHeightWriteTimer = null;
      this.persistTerminalPreferences({
        paneHeightPercent: this.paneHeightPercent,
      });
    }
    for (const controller of this.createControllers) controller.abort();
    this.createControllers.clear();
    for (const session of [...this.sessions]) {
      void this.closeSession(session.id, "applicationExit");
    }
  }

  private applyEvent(sessionId: string, event: TerminalEvent): void {
    if (this.disposed || this.closedSessionIds.has(sessionId)) return;
    if (event.event === "started") {
      this.upsertSession(event.session);
      return;
    }
    const session = this.sessions.find(({ id }) => id === sessionId);
    if (!session) return;
    if (event.event === "output") {
      const subscriber = this.outputSubscribers.get(sessionId);
      if (subscriber) subscriber(event);
      else {
        const pending = this.pendingOutput.get(sessionId) ?? [];
        pending.push(event);
        this.pendingOutput.set(sessionId, pending);
      }
      return;
    }
    if (event.event === "exited") {
      session.state = "exited";
      session.exitCode = event.exitCode;
      session.exitReason = event.reason;
      this.clearInputQueue(sessionId);
      const detail =
        event.exitCode === null ? "" : ` with code ${event.exitCode}`;
      this.statusAnnouncement = `Terminal exited${detail}.`;
      return;
    }
    session.state = "failed";
    session.errorMessage = event.error.message;
    this.clearInputQueue(sessionId);
    this.statusAnnouncement = `Terminal failed. ${event.error.message}`;
  }

  private upsertSession(summary: TerminalSessionSummary): void {
    const existing = this.sessions.find(({ id }) => id === summary.id);
    if (existing) {
      existing.state = summary.state;
      existing.kind = summary.kind;
      existing.locationId = summary.locationId;
      existing.title = summary.title;
      existing.contextLabel = summary.contextLabel;
    } else {
      this.sessions.push({
        ...summary,
        exitCode: null,
        exitReason: null,
        errorMessage: null,
      });
    }
    this.activeSessionId ??= summary.id;
  }

  private ensureInputQueue(sessionId: string): InputQueue {
    let queue = this.inputQueues.get(sessionId);
    if (!queue) {
      queue = {
        pending: [],
        pendingBytes: 0,
        nextSequence: 0,
        timer: null,
        chain: Promise.resolve(),
        failed: false,
      };
      this.inputQueues.set(sessionId, queue);
    }
    return queue;
  }

  private enqueueInput(sessionId: string, bytes: Uint8Array): void {
    const session = this.sessions.find(({ id }) => id === sessionId);
    if (!session || session.state !== "running") return;
    for (
      let offset = 0;
      offset < bytes.byteLength;
      offset += INPUT_BATCH_BYTES
    ) {
      const chunk = bytes.slice(offset, offset + INPUT_BATCH_BYTES);
      const queue = this.ensureInputQueue(sessionId);
      if (queue.failed) return;
      if (queue.pendingBytes + chunk.byteLength > INPUT_BATCH_BYTES) {
        this.flushInput(sessionId, queue);
      }
      queue.pending.push(chunk);
      queue.pendingBytes += chunk.byteLength;
      if (queue.pendingBytes >= INPUT_BATCH_BYTES) {
        this.flushInput(sessionId, queue);
      } else if (queue.timer === null) {
        queue.timer = setTimeout(() => {
          queue.timer = null;
          this.flushInput(sessionId, queue);
        }, INPUT_BATCH_DELAY_MS);
      }
    }
  }

  private flushInput(sessionId: string, queue: InputQueue): void {
    if (queue.pendingBytes === 0 || queue.failed) return;
    if (queue.timer !== null) {
      clearTimeout(queue.timer);
      queue.timer = null;
    }
    const bytes = new Uint8Array(queue.pendingBytes);
    let offset = 0;
    for (const chunk of queue.pending) {
      bytes.set(chunk, offset);
      offset += chunk.byteLength;
    }
    queue.pending = [];
    queue.pendingBytes = 0;
    const sequence = queue.nextSequence++;
    queue.chain = queue.chain
      .then(async () => {
        if (!queue.failed && !this.closedSessionIds.has(sessionId)) {
          await this.dataSource.writeTerminal(sessionId, sequence, bytes);
        }
      })
      .catch((error) => {
        queue.failed = true;
        const session = this.sessions.find(({ id }) => id === sessionId);
        if (session) session.errorMessage = terminalErrorMessage(error);
        this.errorMessage = terminalErrorMessage(error);
      });
  }

  private clearInputQueue(sessionId: string): void {
    const queue = this.inputQueues.get(sessionId);
    if (queue?.timer != null) clearTimeout(queue.timer);
    if (queue) queue.failed = true;
    this.inputQueues.delete(sessionId);
  }

  private async loadPreferences(): Promise<void> {
    try {
      const snapshot = await this.preferencesDataSource.getPreferences();
      this.applyPreferences(snapshot.preferences.terminal);
      this.preferencesWarningMessage = snapshot.warning?.message ?? null;
    } catch (error) {
      this.preferencesWarningMessage =
        error instanceof Error
          ? error.message
          : "Explora could not load saved terminal preferences and used defaults instead.";
    }
  }

  private applyPreferences(preferences: TerminalPreferences): void {
    this.visible = preferences.visible;
    this.paneHeightPercent = preferences.paneHeightPercent;
    this.fontSize = preferences.fontSize;
    this.scrollback = preferences.scrollback;
    this.screenReaderMode = preferences.screenReaderMode;
  }

  private setVisible(visible: boolean): void {
    if (visible === this.visible) return;
    this.visible = visible;
    this.persistTerminalPreferences({ visible });
  }

  private persistTerminalPreferences(patch: TerminalPreferencesPatch): void {
    this.preferenceWriteQueue = this.preferenceWriteQueue.then(async () => {
      try {
        await this.preferencesDataSource.updatePreferences({
          layout: {},
          terminal: patch,
        });
      } catch (error) {
        this.preferencesWarningMessage =
          error instanceof Error
            ? error.message
            : "Explora could not save the latest terminal preference change.";
      }
    });
  }
}

const terminalErrorMessage = (error: unknown): string => {
  if (
    typeof error === "object" &&
    error !== null &&
    "message" in error &&
    typeof error.message === "string"
  ) {
    return error.message;
  }
  return "The terminal operation failed unexpectedly.";
};

export const isTerminalSessionInteractive = (
  state: TerminalSessionState,
): boolean => state === "starting" || state === "running";
