import { invoke } from "@tauri-apps/api/core";

import type {
  PreferencesSnapshot,
  PreferencesWarningCode,
  UserPreferences,
  UserPreferencesPatch,
} from "$lib/contracts/preferences";
import type {
  SortColumn,
  SortDirection,
  ViewMode,
} from "$lib/contracts/explorer";
import type { PreferencesDataSource } from "$lib/data/preferences-data-source";

const viewModes = new Set<ViewMode>(["list", "grid"]);
const sortColumns = new Set<SortColumn>(["name", "modifiedAt", "size"]);
const sortDirections = new Set<SortDirection>(["ascending", "descending"]);
const warningCodes = new Set<PreferencesWarningCode>([
  "readFailed",
  "malformed",
  "unsupportedVersion",
]);

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const parsePreferences = (value: unknown): UserPreferences => {
  if (!isRecord(value) || !isRecord(value.layout)) {
    throw new Error("Invalid preference response: layout must be an object.");
  }
  const { layout } = value;
  if (typeof layout.sidebarCollapsed !== "boolean") {
    throw new Error(
      "Invalid preference response: sidebarCollapsed must be a boolean.",
    );
  }
  if (
    typeof layout.viewMode !== "string" ||
    !viewModes.has(layout.viewMode as ViewMode)
  ) {
    throw new Error("Invalid preference response: viewMode is unknown.");
  }
  if (!isRecord(layout.sort)) {
    throw new Error("Invalid preference response: sort must be an object.");
  }
  if (
    typeof layout.sort.column !== "string" ||
    !sortColumns.has(layout.sort.column as SortColumn)
  ) {
    throw new Error("Invalid preference response: sort column is unknown.");
  }
  if (
    typeof layout.sort.direction !== "string" ||
    !sortDirections.has(layout.sort.direction as SortDirection)
  ) {
    throw new Error("Invalid preference response: sort direction is unknown.");
  }

  return {
    layout: {
      sidebarCollapsed: layout.sidebarCollapsed,
      viewMode: layout.viewMode as ViewMode,
      sort: {
        column: layout.sort.column as SortColumn,
        direction: layout.sort.direction as SortDirection,
      },
    },
  };
};

const parseSnapshot = (value: unknown): PreferencesSnapshot => {
  if (!isRecord(value)) {
    throw new Error("Invalid preference response: snapshot must be an object.");
  }
  let warning = null;
  if (value.warning !== null) {
    if (
      !isRecord(value.warning) ||
      typeof value.warning.code !== "string" ||
      !warningCodes.has(value.warning.code as PreferencesWarningCode) ||
      typeof value.warning.message !== "string"
    ) {
      throw new Error("Invalid preference response: warning is malformed.");
    }
    warning = {
      code: value.warning.code as PreferencesWarningCode,
      message: value.warning.message,
    };
  }
  return {
    preferences: parsePreferences(value.preferences),
    warning,
  };
};

export class TauriPreferencesDataSource implements PreferencesDataSource {
  async getPreferences(): Promise<PreferencesSnapshot> {
    return parseSnapshot(await invoke<unknown>("get_user_preferences"));
  }

  async updatePreferences(
    patch: UserPreferencesPatch,
  ): Promise<UserPreferences> {
    return parsePreferences(
      await invoke<unknown>("update_user_preferences", { patch }),
    );
  }
}
