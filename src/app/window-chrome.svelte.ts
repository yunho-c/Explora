import { invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

export type WindowChromeMode = "browser" | "activating" | "custom" | "native";

export interface WindowChromeAdapter {
  readonly isTauri: boolean;
  readonly windowLabel: string;
  activate(): Promise<void>;
  restoreAndShowNative(): Promise<void>;
  show(): Promise<void>;
}

interface WindowChromeOptions {
  document?: Document;
  fallbackMs?: number;
  sessionStorage?: Storage;
}

const FALLBACK_MS = 5_000;

const browserAdapter: WindowChromeAdapter = {
  isTauri: false,
  windowLabel: "browser",
  activate: async () => {},
  restoreAndShowNative: async () => {},
  show: async () => {},
};

export const createWindowChromeAdapter = (): WindowChromeAdapter => {
  if (!isTauri()) return browserAdapter;

  const currentWindow = getCurrentWindow();
  return {
    isTauri: true,
    windowLabel: currentWindow.label,
    activate: () => invoke("activate_custom_titlebar"),
    restoreAndShowNative: () => invoke("show_native_titlebar_fallback"),
    show: () => currentWindow.show(),
  };
};

export class WindowChromeController {
  mode = $state<WindowChromeMode>("browser");
  recoveryMessage = $state<string | null>(null);

  readonly #adapter: WindowChromeAdapter;
  readonly #document: Document;
  readonly #fallbackMs: number;
  readonly #storage: Storage;

  constructor(
    adapter: WindowChromeAdapter = createWindowChromeAdapter(),
    options: WindowChromeOptions = {},
  ) {
    this.#adapter = adapter;
    this.#document = options.document ?? document;
    this.#fallbackMs = options.fallbackMs ?? FALLBACK_MS;
    this.#storage = options.sessionStorage ?? sessionStorage;
    this.mode = adapter.isTauri ? "activating" : "browser";
  }

  start(): () => void {
    if (!this.#adapter.isTauri) return () => {};

    const storageKey = `explora:window-chrome:${this.#adapter.windowLabel}`;
    let disposed = false;
    let settled = false;
    let fallbackStarted = false;
    let timeoutId: number | undefined;

    const clearTimer = () => {
      if (timeoutId !== undefined) window.clearTimeout(timeoutId);
      timeoutId = undefined;
    };

    const revealNativeFallback = async () => {
      if (disposed || settled || fallbackStarted) return;
      fallbackStarted = true;
      clearTimer();

      try {
        await this.#adapter.restoreAndShowNative();
      } catch {
        if (disposed) return;
        try {
          await this.#adapter.show();
          this.recoveryMessage =
            "Native decorations could not be restored. Window controls may have limited behavior until Explora restarts.";
        } catch {
          this.recoveryMessage =
            "The application window could not be revealed after window-decoration activation failed.";
          return;
        }
      }

      if (disposed) return;
      settled = true;
      this.mode = "native";
      this.#storage.setItem(storageKey, "native");
    };

    const revealCustomChrome = async () => {
      if (disposed || settled || fallbackStarted) return;
      settled = true;
      clearTimer();

      try {
        await this.#adapter.show();
        if (disposed) return;
        this.mode = "custom";
        this.#storage.setItem(storageKey, "custom");
      } catch {
        settled = false;
        await revealNativeFallback();
      }
    };

    const observeActivation = () => {
      if (
        this.#document.querySelector("[data-tauri-plugin-decoration-active]")
      ) {
        void revealCustomChrome();
      }
    };

    const Observer = this.#document.defaultView?.MutationObserver;
    if (!Observer) {
      void revealNativeFallback();
      return () => {
        disposed = true;
      };
    }

    const observer = new Observer(observeActivation);
    observer.observe(this.#document.documentElement, {
      attributes: true,
      childList: true,
      subtree: true,
    });

    timeoutId = window.setTimeout(() => {
      void revealNativeFallback();
    }, this.#fallbackMs);

    observeActivation();

    if (this.#storage.getItem(storageKey) === "native") {
      void revealNativeFallback();
    } else {
      void this.#adapter.activate().catch(() => {
        void revealNativeFallback();
      });
    }

    return () => {
      disposed = true;
      clearTimer();
      observer.disconnect();
    };
  }
}
