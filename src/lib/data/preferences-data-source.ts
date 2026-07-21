import type {
  PreferencesSnapshot,
  UserPreferences,
  UserPreferencesPatch,
} from "$lib/contracts/preferences";

export interface PreferencesDataSource {
  getPreferences(): Promise<PreferencesSnapshot>;
  updatePreferences(patch: UserPreferencesPatch): Promise<UserPreferences>;
}
