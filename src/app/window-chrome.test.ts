import { waitFor } from "@testing-library/svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  WindowChromeController,
  type WindowChromeAdapter,
} from "./window-chrome.svelte";

const tauriAdapter = (
  overrides: Partial<WindowChromeAdapter> = {},
): WindowChromeAdapter => ({
  isTauri: true,
  windowLabel: "main",
  activate: vi.fn(async () => {}),
  restoreAndShowNative: vi.fn(async () => {}),
  show: vi.fn(async () => {}),
  ...overrides,
});

afterEach(() => {
  vi.useRealTimers();
  sessionStorage.clear();
  document.documentElement.removeAttribute(
    "data-tauri-plugin-decoration-active",
  );
});

describe("WindowChromeController", () => {
  it("does nothing outside Tauri", () => {
    const adapter = tauriAdapter({ isTauri: false });
    const controller = new WindowChromeController(adapter);
    const stop = controller.start();

    expect(controller.mode).toBe("browser");
    expect(adapter.activate).not.toHaveBeenCalled();
    stop();
  });

  it("reveals the window after custom decoration activation", async () => {
    const adapter = tauriAdapter();
    const controller = new WindowChromeController(adapter);
    const stop = controller.start();

    document.documentElement.setAttribute(
      "data-tauri-plugin-decoration-active",
      "",
    );

    await waitFor(() => expect(controller.mode).toBe("custom"));
    expect(adapter.activate).toHaveBeenCalledOnce();
    expect(adapter.show).toHaveBeenCalledOnce();
    expect(adapter.restoreAndShowNative).not.toHaveBeenCalled();
    expect(sessionStorage.getItem("explora:window-chrome:main")).toBe("custom");
    stop();
  });

  it("restores native decorations when activation rejects", async () => {
    const adapter = tauriAdapter({
      activate: vi.fn(async () => {
        throw new Error("unsupported compositor");
      }),
    });
    const controller = new WindowChromeController(adapter);
    const stop = controller.start();

    await waitFor(() => expect(controller.mode).toBe("native"));
    expect(adapter.restoreAndShowNative).toHaveBeenCalledOnce();
    expect(sessionStorage.getItem("explora:window-chrome:main")).toBe("native");
    stop();
  });

  it("falls back after the activation deadline", async () => {
    vi.useFakeTimers();
    const adapter = tauriAdapter();
    const controller = new WindowChromeController(adapter, { fallbackMs: 25 });
    const stop = controller.start();

    await vi.advanceTimersByTimeAsync(25);

    expect(controller.mode).toBe("native");
    expect(adapter.restoreAndShowNative).toHaveBeenCalledOnce();
    stop();
  });

  it("directly reveals the window if native restoration fails", async () => {
    const adapter = tauriAdapter({
      activate: vi.fn(async () => {
        throw new Error("activation failed");
      }),
      restoreAndShowNative: vi.fn(async () => {
        throw new Error("restore failed");
      }),
    });
    const controller = new WindowChromeController(adapter);
    const stop = controller.start();

    await waitFor(() => expect(controller.mode).toBe("native"));
    expect(adapter.show).toHaveBeenCalledOnce();
    expect(controller.recoveryMessage).toContain(
      "Native decorations could not be restored",
    );
    stop();
  });

  it("keeps a session on its known native fallback", async () => {
    sessionStorage.setItem("explora:window-chrome:main", "native");
    const adapter = tauriAdapter();
    const controller = new WindowChromeController(adapter);
    const stop = controller.start();

    await waitFor(() => expect(controller.mode).toBe("native"));
    expect(adapter.activate).not.toHaveBeenCalled();
    expect(adapter.restoreAndShowNative).toHaveBeenCalledOnce();
    stop();
  });
});
