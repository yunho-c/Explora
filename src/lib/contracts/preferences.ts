import type { SortDescriptor, ViewMode } from "$lib/contracts/explorer";

export type FavoriteRole =
  | "home"
  | "desktop"
  | "documents"
  | "downloads"
  | "pictures"
  | "music"
  | "videos";

export const DEFAULT_FAVORITE_ROLES: readonly FavoriteRole[] = [
  "home",
  "desktop",
  "documents",
  "downloads",
  "pictures",
  "music",
  "videos",
];

const favoriteRoleSet = new Set<FavoriteRole>(DEFAULT_FAVORITE_ROLES);

export const isFavoriteRole = (role: string): role is FavoriteRole =>
  favoriteRoleSet.has(role as FavoriteRole);

export interface LayoutPreferences {
  sidebarCollapsed: boolean;
  viewMode: ViewMode;
  sort: SortDescriptor;
  favoriteRoles: FavoriteRole[];
  hiddenSyncedFolderIds: string[];
  hiddenSshTargetIds: string[];
}

export interface UserPreferences {
  layout: LayoutPreferences;
}

export interface LayoutPreferencesPatch {
  sidebarCollapsed?: boolean;
  viewMode?: ViewMode;
  sort?: SortDescriptor;
  favoriteRoles?: FavoriteRole[];
  hiddenSyncedFolderIds?: string[];
  hiddenSshTargetIds?: string[];
}

export interface UserPreferencesPatch {
  layout: LayoutPreferencesPatch;
}

export type PreferencesWarningCode =
  "readFailed" | "malformed" | "unsupportedVersion";

export interface PreferencesWarning {
  code: PreferencesWarningCode;
  message: string;
}

export interface PreferencesSnapshot {
  preferences: UserPreferences;
  warning: PreferencesWarning | null;
}

export const defaultUserPreferences = (): UserPreferences => ({
  layout: {
    sidebarCollapsed: false,
    viewMode: "list",
    sort: { column: "name", direction: "ascending" },
    favoriteRoles: [...DEFAULT_FAVORITE_ROLES],
    hiddenSyncedFolderIds: [],
    hiddenSshTargetIds: [],
  },
});
