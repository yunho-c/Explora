import { invoke } from "@tauri-apps/api/core";

import {
  DEFAULT_FAVORITE_ROLES,
  isFavoriteRole,
  type FavoriteRole,
  type PreferencesSnapshot,
  type PreferencesWarningCode,
  type UserPreferences,
  type UserPreferencesPatch,
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
  if (
    !Array.isArray(layout.favoriteRoles) ||
    layout.favoriteRoles.some(
      (role) => typeof role !== "string" || !isFavoriteRole(role),
    )
  ) {
    throw new Error(
      "Invalid preference response: favorite roles are malformed.",
    );
  }
  const favoriteRoles = layout.favoriteRoles as FavoriteRole[];
  const favoriteRoleSet = new Set(favoriteRoles);
  const canonicalFavoriteRoles = DEFAULT_FAVORITE_ROLES.filter((role) =>
    favoriteRoleSet.has(role),
  );
  if (
    favoriteRoleSet.size !== favoriteRoles.length ||
    favoriteRoles.some((role, index) => role !== canonicalFavoriteRoles[index])
  ) {
    throw new Error(
      "Invalid preference response: favorite roles are not canonical.",
    );
  }

  return {
    layout: {
      sidebarCollapsed: layout.sidebarCollapsed,
      viewMode: layout.viewMode as ViewMode,
      sort: {
        column: layout.sort.column as SortColumn,
        direction: layout.sort.direction as SortDirection,
      },
      favoriteRoles: [...favoriteRoles],
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
