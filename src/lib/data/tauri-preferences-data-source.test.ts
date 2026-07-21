import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it } from "vitest";

import { TauriPreferencesDataSource } from "./tauri-preferences-data-source";

const preferencesPayload = {
  layout: {
    sidebarCollapsed: true,
    viewMode: "grid",
    sort: { column: "size", direction: "descending" },
    favoriteRoles: ["home", "documents", "music"],
    hiddenSshTargetIds: ["config:archived", "manual:target-1"],
  },
};

afterEach(() => clearMocks());

describe("TauriPreferencesDataSource", () => {
  it("validates preference snapshots and sends typed partial updates", async () => {
    const calls: Array<{ command: string; args: unknown }> = [];
    mockIPC((command, args) => {
      calls.push({ command, args });
      if (command === "get_user_preferences") {
        return {
          preferences: preferencesPayload,
          warning: {
            code: "malformed",
            message: "Saved preferences were restored.",
          },
        };
      }
      return preferencesPayload;
    });
    const source = new TauriPreferencesDataSource();

    const snapshot = await source.getPreferences();
    expect(snapshot.preferences.layout.viewMode).toBe("grid");
    expect(snapshot.warning?.code).toBe("malformed");
    await source.updatePreferences({
      layout: { sidebarCollapsed: false },
    });

    expect(calls).toEqual([
      { command: "get_user_preferences", args: {} },
      {
        command: "update_user_preferences",
        args: { patch: { layout: { sidebarCollapsed: false } } },
      },
    ]);
  });

  it("rejects unknown persisted enum values", async () => {
    mockIPC(() => ({
      preferences: {
        ...preferencesPayload,
        layout: { ...preferencesPayload.layout, viewMode: "columns" },
      },
      warning: null,
    }));

    await expect(
      new TauriPreferencesDataSource().getPreferences(),
    ).rejects.toThrow("viewMode is unknown");
  });

  it("rejects non-canonical favorite role responses", async () => {
    mockIPC(() => ({
      preferences: {
        ...preferencesPayload,
        layout: {
          ...preferencesPayload.layout,
          favoriteRoles: ["music", "home", "music"],
        },
      },
      warning: null,
    }));

    await expect(
      new TauriPreferencesDataSource().getPreferences(),
    ).rejects.toThrow("favorite roles are not canonical");
  });

  it("rejects malformed hidden SSH target IDs", async () => {
    mockIPC(() => ({
      preferences: {
        ...preferencesPayload,
        layout: {
          ...preferencesPayload.layout,
          hiddenSshTargetIds: ["unknown:target"],
        },
      },
      warning: null,
    }));

    await expect(
      new TauriPreferencesDataSource().getPreferences(),
    ).rejects.toThrow("hidden SSH target IDs are malformed");
  });

  it("rejects malformed recovery warnings", async () => {
    mockIPC(() => ({
      preferences: preferencesPayload,
      warning: { code: "futureWarning", message: "Unknown warning." },
    }));

    await expect(
      new TauriPreferencesDataSource().getPreferences(),
    ).rejects.toThrow("warning is malformed");
  });
});
