import {
  defaultUserPreferences,
  type PreferencesSnapshot,
  type UserPreferences,
  type UserPreferencesPatch,
} from "$lib/contracts/preferences";
import type { PreferencesDataSource } from "$lib/data/preferences-data-source";

const clonePreferences = (preferences: UserPreferences): UserPreferences => ({
  layout: {
    sidebarCollapsed: preferences.layout.sidebarCollapsed,
    viewMode: preferences.layout.viewMode,
    sort: { ...preferences.layout.sort },
    favoriteRoles: [...preferences.layout.favoriteRoles],
    hiddenSshTargetIds: [...preferences.layout.hiddenSshTargetIds],
  },
  terminal: { ...preferences.terminal },
});

export class MemoryPreferencesDataSource implements PreferencesDataSource {
  #preferences: UserPreferences;

  constructor(preferences: UserPreferences = defaultUserPreferences()) {
    this.#preferences = clonePreferences(preferences);
  }

  async getPreferences(): Promise<PreferencesSnapshot> {
    return {
      preferences: clonePreferences(this.#preferences),
      warning: null,
    };
  }

  async updatePreferences(
    patch: UserPreferencesPatch,
  ): Promise<UserPreferences> {
    this.#preferences = {
      layout: {
        sidebarCollapsed:
          patch.layout.sidebarCollapsed ??
          this.#preferences.layout.sidebarCollapsed,
        viewMode: patch.layout.viewMode ?? this.#preferences.layout.viewMode,
        sort: patch.layout.sort
          ? { ...patch.layout.sort }
          : { ...this.#preferences.layout.sort },
        favoriteRoles: patch.layout.favoriteRoles
          ? [...patch.layout.favoriteRoles]
          : [...this.#preferences.layout.favoriteRoles],
        hiddenSshTargetIds: patch.layout.hiddenSshTargetIds
          ? [...patch.layout.hiddenSshTargetIds]
          : [...this.#preferences.layout.hiddenSshTargetIds],
      },
      terminal: {
        ...this.#preferences.terminal,
        ...patch.terminal,
      },
    };
    return clonePreferences(this.#preferences);
  }
}
