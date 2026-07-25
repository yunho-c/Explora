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
  hiddenSshTargetIds: string[];
}

export interface TerminalPreferences {
  visible: boolean;
  paneHeightPercent: number;
  fontSize: number;
  scrollback: number;
  screenReaderMode: boolean;
}

export interface UserPreferences {
  layout: LayoutPreferences;
  terminal: TerminalPreferences;
}

export interface LayoutPreferencesPatch {
  sidebarCollapsed?: boolean;
  viewMode?: ViewMode;
  sort?: SortDescriptor;
  favoriteRoles?: FavoriteRole[];
  hiddenSshTargetIds?: string[];
}

export type TerminalPreferencesPatch = Partial<TerminalPreferences>;

export interface UserPreferencesPatch {
  layout: LayoutPreferencesPatch;
  terminal?: TerminalPreferencesPatch;
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
    hiddenSshTargetIds: [],
  },
  terminal: {
    visible: false,
    paneHeightPercent: 32,
    fontSize: 13,
    scrollback: 5_000,
    screenReaderMode: true,
  },
});
