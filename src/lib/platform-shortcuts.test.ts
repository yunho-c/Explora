import { afterEach, describe, expect, it, vi } from "vitest";

import {
  deletionShortcut,
  isRenameShortcut,
  isNewFolderShortcut,
} from "./platform-shortcuts";

const platform = (value: string) => {
  vi.stubGlobal("navigator", { platform: value, userAgent: value });
};

describe("platform filesystem shortcuts", () => {
  afterEach(() => vi.unstubAllGlobals());

  it.each(["MacIntel", "Win32", "Linux x86_64"])(
    "uses the platform modifier for New Folder on %s",
    (value) => {
      platform(value);
      const mac = value === "MacIntel";
      expect(
        isNewFolderShortcut(
          new KeyboardEvent("keydown", {
            key: "N",
            shiftKey: true,
            metaKey: mac,
            ctrlKey: !mac,
          }),
        ),
      ).toBe(true);
      expect(
        isNewFolderShortcut(
          new KeyboardEvent("keydown", {
            key: "n",
            metaKey: mac,
            ctrlKey: !mac,
          }),
        ),
      ).toBe(false);
      expect(
        isNewFolderShortcut(
          new KeyboardEvent("keydown", {
            key: "n",
            shiftKey: true,
            metaKey: !mac,
            ctrlKey: mac,
          }),
        ),
      ).toBe(false);
    },
  );

  it("uses Finder-style rename and deletion shortcuts on macOS", () => {
    platform("MacIntel");

    expect(
      isRenameShortcut(new KeyboardEvent("keydown", { key: "Enter" })),
    ).toBe(true);
    expect(
      deletionShortcut(
        new KeyboardEvent("keydown", { key: "Backspace", metaKey: true }),
      ),
    ).toBe("trash");
    expect(
      deletionShortcut(
        new KeyboardEvent("keydown", {
          key: "Backspace",
          metaKey: true,
          altKey: true,
        }),
      ),
    ).toBe("deletePermanently");
    expect(
      deletionShortcut(new KeyboardEvent("keydown", { key: "Delete" })),
    ).toBeNull();
  });

  it("uses F2, Delete, and Shift+Delete on Windows and Linux", () => {
    platform("Win32");

    expect(isRenameShortcut(new KeyboardEvent("keydown", { key: "F2" }))).toBe(
      true,
    );
    expect(
      deletionShortcut(new KeyboardEvent("keydown", { key: "Delete" })),
    ).toBe("trash");
    expect(
      deletionShortcut(
        new KeyboardEvent("keydown", { key: "Delete", shiftKey: true }),
      ),
    ).toBe("deletePermanently");
    expect(
      isRenameShortcut(new KeyboardEvent("keydown", { key: "Enter" })),
    ).toBe(false);
  });
});
