import type { SortDescriptor, ViewMode } from "$lib/contracts/explorer";

export interface LayoutPreferences {
  sidebarCollapsed: boolean;
  viewMode: ViewMode;
  sort: SortDescriptor;
}

export interface UserPreferences {
  layout: LayoutPreferences;
}

export interface LayoutPreferencesPatch {
  sidebarCollapsed?: boolean;
  viewMode?: ViewMode;
  sort?: SortDescriptor;
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
  },
});
