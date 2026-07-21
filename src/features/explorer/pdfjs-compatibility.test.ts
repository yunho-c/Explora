import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mapDescriptor = Object.getOwnPropertyDescriptor(
  Map.prototype,
  "getOrInsertComputed",
);
const weakMapDescriptor = Object.getOwnPropertyDescriptor(
  WeakMap.prototype,
  "getOrInsertComputed",
);

const restoreProperty = (
  prototype: object,
  key: string,
  descriptor: PropertyDescriptor | undefined,
) => {
  if (descriptor) {
    Object.defineProperty(prototype, key, descriptor);
  } else {
    Reflect.deleteProperty(prototype, key);
  }
};

describe("PDF.js system WebView compatibility", () => {
  beforeEach(() => {
    Reflect.deleteProperty(Map.prototype, "getOrInsertComputed");
    Reflect.deleteProperty(WeakMap.prototype, "getOrInsertComputed");
    vi.resetModules();
  });

  afterEach(() => {
    restoreProperty(Map.prototype, "getOrInsertComputed", mapDescriptor);
    restoreProperty(
      WeakMap.prototype,
      "getOrInsertComputed",
      weakMapDescriptor,
    );
  });

  it("loads the maintained legacy runtime when proposal-stage map methods are absent", async () => {
    await import("pdfjs-dist/legacy/build/pdf.mjs");

    expect(Reflect.get(Map.prototype, "getOrInsertComputed")).toBeTypeOf(
      "function",
    );
    expect(Reflect.get(WeakMap.prototype, "getOrInsertComputed")).toBeTypeOf(
      "function",
    );
  });
});
