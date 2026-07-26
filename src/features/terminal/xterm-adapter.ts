import { FitAddon } from "@xterm/addon-fit";
import { Terminal, type IDisposable, type ITheme } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";

import type { TerminalSize } from "$lib/contracts/terminal";

export interface XtermAdapterCallbacks {
  onData: (value: string) => void;
  onBinary: (value: string) => void;
  onResize: (size: TerminalSize) => void;
  onPaste: (text: string) => void;
  onToggleVisibility: () => void;
  onNewTerminal: () => void;
  onNextSession: () => void;
  onPreviousSession: () => void;
}

export interface XtermPreferences {
  fontSize: number;
  scrollback: number;
  screenReaderMode: boolean;
}

export class XtermAdapter {
  private readonly terminal: Terminal;
  private readonly fitAddon = new FitAddon();
  private readonly subscriptions: IDisposable[] = [];
  private readonly resizeObserver: ResizeObserver;
  private readonly themeObserver: MutationObserver;
  private resizeFrame: number | null = null;
  private disposed = false;

  constructor(
    private readonly mount: HTMLElement,
    private readonly callbacks: XtermAdapterCallbacks,
    preferences: XtermPreferences,
  ) {
    this.terminal = new Terminal({
      allowProposedApi: false,
      allowTransparency: false,
      convertEol: false,
      cursorBlink: true,
      cursorStyle: "block",
      drawBoldTextInBrightColors: true,
      fontFamily:
        '"SFMono-Regular", "Cascadia Mono", "Liberation Mono", Menlo, Consolas, monospace',
      fontSize: preferences.fontSize,
      lineHeight: 1.2,
      macOptionIsMeta: true,
      minimumContrastRatio: 4.5,
      screenReaderMode: preferences.screenReaderMode,
      scrollback: preferences.scrollback,
      theme: terminalTheme(mount),
    });
    this.terminal.loadAddon(this.fitAddon);
    this.terminal.open(mount);
    this.subscriptions.push(
      this.terminal.onData(callbacks.onData),
      this.terminal.onBinary(callbacks.onBinary),
    );
    this.terminal.attachCustomKeyEventHandler((event) =>
      this.handleKeyEvent(event),
    );
    mount.addEventListener("paste", this.handlePaste, true);

    this.resizeObserver = new ResizeObserver(() => this.scheduleFit());
    this.resizeObserver.observe(mount);
    this.themeObserver = new MutationObserver(() => {
      this.terminal.options.theme = terminalTheme(this.mount);
    });
    this.themeObserver.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class", "style"],
    });
    this.scheduleFit();
  }

  setPreferences(preferences: XtermPreferences): void {
    if (this.disposed) return;
    this.terminal.options.fontSize = preferences.fontSize;
    this.terminal.options.scrollback = preferences.scrollback;
    this.terminal.options.screenReaderMode = preferences.screenReaderMode;
    this.scheduleFit();
  }

  write(bytes: Uint8Array, onConsumed: () => void): void {
    if (this.disposed) return;
    this.terminal.write(bytes, () => {
      if (!this.disposed) onConsumed();
    });
  }

  focus(): void {
    if (this.disposed) return;
    this.terminal.focus();
  }

  fit(): void {
    if (
      this.disposed ||
      this.mount.clientWidth < 2 ||
      this.mount.clientHeight < 2
    ) {
      return;
    }
    try {
      this.fitAddon.fit();
      this.callbacks.onResize({
        columns: this.terminal.cols,
        rows: this.terminal.rows,
        pixelWidth: clampPixelSize(this.mount.clientWidth),
        pixelHeight: clampPixelSize(this.mount.clientHeight),
      });
    } catch {
      // A native WebView can briefly report incomplete font metrics while a
      // hidden pane is becoming visible. The next observer tick retries.
    }
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    if (this.resizeFrame !== null) cancelAnimationFrame(this.resizeFrame);
    this.resizeObserver.disconnect();
    this.themeObserver.disconnect();
    this.mount.removeEventListener("paste", this.handlePaste, true);
    for (const subscription of this.subscriptions) subscription.dispose();
    this.fitAddon.dispose();
    this.terminal.dispose();
  }

  private scheduleFit(): void {
    if (this.resizeFrame !== null || this.disposed) return;
    this.resizeFrame = requestAnimationFrame(() => {
      this.resizeFrame = null;
      this.fit();
    });
  }

  private readonly handlePaste = (event: ClipboardEvent) => {
    const text = event.clipboardData?.getData("text/plain") ?? "";
    if (!text) return;
    event.preventDefault();
    event.stopPropagation();
    this.callbacks.onPaste(text);
  };

  private handleKeyEvent(event: KeyboardEvent): boolean {
    if (
      event.type === "keydown" &&
      event.ctrlKey &&
      !event.metaKey &&
      !event.altKey &&
      event.shiftKey &&
      event.key === "`"
    ) {
      event.preventDefault();
      event.stopPropagation();
      this.callbacks.onNewTerminal();
      return false;
    }
    if (
      event.type === "keydown" &&
      event.ctrlKey &&
      !event.metaKey &&
      !event.altKey &&
      (event.key === "PageDown" || event.key === "PageUp")
    ) {
      event.preventDefault();
      event.stopPropagation();
      if (event.key === "PageDown") this.callbacks.onNextSession();
      else this.callbacks.onPreviousSession();
      return false;
    }
    if (
      event.type === "keydown" &&
      event.ctrlKey &&
      !event.metaKey &&
      !event.altKey &&
      event.key === "`"
    ) {
      event.preventDefault();
      event.stopPropagation();
      this.callbacks.onToggleVisibility();
      return false;
    }

    const copyShortcut =
      event.type === "keydown" &&
      event.key.toLocaleLowerCase() === "c" &&
      (event.metaKey || event.ctrlKey) &&
      this.terminal.hasSelection();
    if (copyShortcut) {
      event.preventDefault();
      event.stopPropagation();
      const clipboard = navigator.clipboard;
      if (clipboard) {
        void clipboard.writeText(this.terminal.getSelection()).catch(() => {});
      }
      return false;
    }
    return true;
  }
}

const clampPixelSize = (value: number): number =>
  Math.min(Math.max(Math.round(value), 0), 32_768);

const terminalTheme = (element: HTMLElement): ITheme => {
  const style = getComputedStyle(element);
  const token = (name: string, fallback: string) =>
    style.getPropertyValue(name).trim() || fallback;
  return {
    background: token("--terminal-background", "#111318"),
    foreground: token("--terminal-foreground", "#eef1f5"),
    cursor: token("--terminal-cursor", "#f8fafc"),
    cursorAccent: token("--terminal-cursor-accent", "#111318"),
    selectionBackground: token("--terminal-selection-background", "#34435a"),
    selectionForeground: token("--terminal-selection-foreground", "#ffffff"),
    black: "#242932",
    red: "#ff7b72",
    green: "#7ee787",
    yellow: "#e3b341",
    blue: "#79c0ff",
    magenta: "#d2a8ff",
    cyan: "#56d4dd",
    white: "#d9dee7",
    brightBlack: "#8b949e",
    brightRed: "#ff7b72",
    brightGreen: "#56d364",
    brightYellow: "#e3b341",
    brightBlue: "#79c0ff",
    brightMagenta: "#d2a8ff",
    brightCyan: "#56d4dd",
    brightWhite: "#f0f6fc",
  };
};
