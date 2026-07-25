use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::filesystem::ExplorerError;

const PREFERENCES_FILE_VERSION: u32 = 4;
const MAX_HIDDEN_SSH_TARGETS: usize = 512;
const MAX_SSH_TARGET_ID_LENGTH: usize = 512;
const MIN_TERMINAL_PANE_HEIGHT_PERCENT: u16 = 20;
const MAX_TERMINAL_PANE_HEIGHT_PERCENT: u16 = 70;
const MIN_TERMINAL_FONT_SIZE: u16 = 10;
const MAX_TERMINAL_FONT_SIZE: u16 = 24;
const MIN_TERMINAL_SCROLLBACK: u32 = 1_000;
const MAX_TERMINAL_SCROLLBACK: u32 = 50_000;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ViewMode {
    #[default]
    List,
    Grid,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SortColumn {
    #[default]
    Name,
    ModifiedAt,
    Size,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SortDirection {
    #[default]
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FavoriteRole {
    Home,
    Desktop,
    Documents,
    Downloads,
    Pictures,
    Music,
    Videos,
}

const DEFAULT_FAVORITE_ROLES: [FavoriteRole; 7] = [
    FavoriteRole::Home,
    FavoriteRole::Desktop,
    FavoriteRole::Documents,
    FavoriteRole::Downloads,
    FavoriteRole::Pictures,
    FavoriteRole::Music,
    FavoriteRole::Videos,
];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SortDescriptorDto {
    pub column: SortColumn,
    pub direction: SortDirection,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayoutPreferencesDto {
    pub sidebar_collapsed: bool,
    pub view_mode: ViewMode,
    pub sort: SortDescriptorDto,
    pub favorite_roles: Vec<FavoriteRole>,
    pub hidden_ssh_target_ids: Vec<String>,
}

impl Default for LayoutPreferencesDto {
    fn default() -> Self {
        Self {
            sidebar_collapsed: false,
            view_mode: ViewMode::default(),
            sort: SortDescriptorDto::default(),
            favorite_roles: DEFAULT_FAVORITE_ROLES.to_vec(),
            hidden_ssh_target_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalPreferencesDto {
    pub visible: bool,
    pub pane_height_percent: u16,
    pub font_size: u16,
    pub scrollback: u32,
    pub screen_reader_mode: bool,
}

impl Default for TerminalPreferencesDto {
    fn default() -> Self {
        Self {
            visible: false,
            pane_height_percent: 32,
            font_size: 13,
            scrollback: 5_000,
            screen_reader_mode: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserPreferencesDto {
    pub layout: LayoutPreferencesDto,
    pub terminal: TerminalPreferencesDto,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayoutPreferencesPatchDto {
    pub sidebar_collapsed: Option<bool>,
    pub view_mode: Option<ViewMode>,
    pub sort: Option<SortDescriptorDto>,
    pub favorite_roles: Option<Vec<FavoriteRole>>,
    pub hidden_ssh_target_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalPreferencesPatchDto {
    pub visible: Option<bool>,
    pub pane_height_percent: Option<u16>,
    pub font_size: Option<u16>,
    pub scrollback: Option<u32>,
    pub screen_reader_mode: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserPreferencesPatchDto {
    pub layout: LayoutPreferencesPatchDto,
    pub terminal: Option<TerminalPreferencesPatchDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreferencesWarningDto {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreferencesSnapshotDto {
    pub preferences: UserPreferencesDto,
    pub warning: Option<PreferencesWarningDto>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredPreferencesDocument {
    version: u32,
    layout: LayoutPreferencesDto,
    terminal: TerminalPreferencesDto,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredPreferencesDocumentV1 {
    version: u32,
    layout: LayoutPreferencesV1,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LayoutPreferencesV1 {
    sidebar_collapsed: bool,
    view_mode: ViewMode,
    sort: SortDescriptorDto,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredPreferencesDocumentV2 {
    version: u32,
    layout: LayoutPreferencesV2,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LayoutPreferencesV2 {
    sidebar_collapsed: bool,
    view_mode: ViewMode,
    sort: SortDescriptorDto,
    favorite_roles: Vec<FavoriteRole>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredPreferencesDocumentV3 {
    version: u32,
    layout: LayoutPreferencesDto,
}

#[derive(Debug, Deserialize)]
struct VersionProbe {
    version: u32,
}

struct PreferencesState {
    preferences: UserPreferencesDto,
    load_warning: Option<PreferencesWarningDto>,
}

pub struct PreferencesStore {
    storage_path: PathBuf,
    state: Mutex<PreferencesState>,
}

impl PreferencesStore {
    pub fn new(storage_path: PathBuf) -> Self {
        let (preferences, load_warning) = load_preferences(&storage_path);
        Self {
            storage_path,
            state: Mutex::new(PreferencesState {
                preferences,
                load_warning,
            }),
        }
    }

    pub fn snapshot(&self) -> Result<PreferencesSnapshotDto, ExplorerError> {
        let state = self
            .state
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        Ok(PreferencesSnapshotDto {
            preferences: state.preferences.clone(),
            warning: state.load_warning.clone(),
        })
    }

    pub fn update(
        &self,
        patch: UserPreferencesPatchDto,
    ) -> Result<UserPreferencesDto, ExplorerError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let mut updated = state.preferences.clone();

        if let Some(sidebar_collapsed) = patch.layout.sidebar_collapsed {
            updated.layout.sidebar_collapsed = sidebar_collapsed;
        }
        if let Some(view_mode) = patch.layout.view_mode {
            updated.layout.view_mode = view_mode;
        }
        if let Some(sort) = patch.layout.sort {
            updated.layout.sort = sort;
        }
        if let Some(favorite_roles) = patch.layout.favorite_roles {
            updated.layout.favorite_roles = canonical_favorite_roles(&favorite_roles);
        }
        if let Some(hidden_ssh_target_ids) = patch.layout.hidden_ssh_target_ids {
            updated.layout.hidden_ssh_target_ids =
                canonical_hidden_ssh_target_ids(&hidden_ssh_target_ids)?;
        }
        if let Some(terminal) = patch.terminal {
            if let Some(visible) = terminal.visible {
                updated.terminal.visible = visible;
            }
            if let Some(pane_height_percent) = terminal.pane_height_percent {
                updated.terminal.pane_height_percent = pane_height_percent;
            }
            if let Some(font_size) = terminal.font_size {
                updated.terminal.font_size = font_size;
            }
            if let Some(scrollback) = terminal.scrollback {
                updated.terminal.scrollback = scrollback;
            }
            if let Some(screen_reader_mode) = terminal.screen_reader_mode {
                updated.terminal.screen_reader_mode = screen_reader_mode;
            }
            validate_terminal_preferences(&updated.terminal)?;
        }

        persist_preferences(&self.storage_path, &updated)?;
        state.preferences = updated.clone();
        state.load_warning = None;
        Ok(updated)
    }
}

fn load_preferences(path: &Path) -> (UserPreferencesDto, Option<PreferencesWarningDto>) {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (UserPreferencesDto::default(), None);
        }
        Err(_) => {
            return recovery(
                "readFailed",
                "Explora could not read saved preferences and used defaults instead.",
            );
        }
    };

    let version = match serde_json::from_slice::<VersionProbe>(&bytes) {
        Ok(probe) => probe.version,
        Err(_) => {
            return recovery(
                "malformed",
                "Explora's saved preferences were malformed and defaults were restored.",
            );
        }
    };
    match version {
        1 => match serde_json::from_slice::<StoredPreferencesDocumentV1>(&bytes) {
            Ok(document) if document.version == 1 => (
                UserPreferencesDto {
                    layout: LayoutPreferencesDto {
                        sidebar_collapsed: document.layout.sidebar_collapsed,
                        view_mode: document.layout.view_mode,
                        sort: document.layout.sort,
                        favorite_roles: DEFAULT_FAVORITE_ROLES.to_vec(),
                        hidden_ssh_target_ids: Vec::new(),
                    },
                    terminal: TerminalPreferencesDto::default(),
                },
                None,
            ),
            _ => malformed_recovery(),
        },
        2 => match serde_json::from_slice::<StoredPreferencesDocumentV2>(&bytes) {
            Ok(document) if document.version == 2 => (
                UserPreferencesDto {
                    layout: LayoutPreferencesDto {
                        sidebar_collapsed: document.layout.sidebar_collapsed,
                        view_mode: document.layout.view_mode,
                        sort: document.layout.sort,
                        favorite_roles: canonical_favorite_roles(
                            &document.layout.favorite_roles,
                        ),
                        hidden_ssh_target_ids: Vec::new(),
                    },
                    terminal: TerminalPreferencesDto::default(),
                },
                None,
            ),
            _ => malformed_recovery(),
        },
        3 => match serde_json::from_slice::<StoredPreferencesDocumentV3>(&bytes) {
            Ok(document) if document.version == 3 => match canonical_hidden_ssh_target_ids(
                &document.layout.hidden_ssh_target_ids,
            ) {
                Ok(hidden_ssh_target_ids) => (
                    UserPreferencesDto {
                        layout: LayoutPreferencesDto {
                            favorite_roles: canonical_favorite_roles(
                                &document.layout.favorite_roles,
                            ),
                            hidden_ssh_target_ids,
                            ..document.layout
                        },
                        terminal: TerminalPreferencesDto::default(),
                    },
                    None,
                ),
                Err(_) => malformed_recovery(),
            },
            _ => malformed_recovery(),
        },
        PREFERENCES_FILE_VERSION => {
            match serde_json::from_slice::<StoredPreferencesDocument>(&bytes) {
                Ok(document) => {
                    match (
                        canonical_hidden_ssh_target_ids(
                            &document.layout.hidden_ssh_target_ids,
                        ),
                        validate_terminal_preferences(&document.terminal),
                    ) {
                        (Ok(hidden_ssh_target_ids), Ok(())) => (
                            UserPreferencesDto {
                                layout: LayoutPreferencesDto {
                                    favorite_roles: canonical_favorite_roles(
                                        &document.layout.favorite_roles,
                                    ),
                                    hidden_ssh_target_ids,
                                    ..document.layout
                                },
                                terminal: document.terminal,
                            },
                            None,
                        ),
                        _ => malformed_recovery(),
                    }
                }
                Err(_) => malformed_recovery(),
            }
        }
        _ => recovery(
            "unsupportedVersion",
            "Explora could not use preferences saved by a different application version and restored defaults.",
        ),
    }
}

fn malformed_recovery() -> (UserPreferencesDto, Option<PreferencesWarningDto>) {
    recovery(
        "malformed",
        "Explora's saved preferences were malformed and defaults were restored.",
    )
}

fn canonical_favorite_roles(roles: &[FavoriteRole]) -> Vec<FavoriteRole> {
    DEFAULT_FAVORITE_ROLES
        .into_iter()
        .filter(|role| roles.contains(role))
        .collect()
}

fn canonical_hidden_ssh_target_ids(ids: &[String]) -> Result<Vec<String>, ExplorerError> {
    if ids.len() > MAX_HIDDEN_SSH_TARGETS
        || ids.iter().any(|id| {
            id.is_empty()
                || id.len() > MAX_SSH_TARGET_ID_LENGTH
                || id.chars().any(char::is_control)
                || !(id.starts_with("manual:") || id.starts_with("config:"))
        })
    {
        return Err(ExplorerError::InvalidConfiguration(
            "The hidden SSH target selection is invalid.".to_owned(),
        ));
    }

    let mut canonical = ids.to_vec();
    canonical.sort();
    canonical.dedup();
    Ok(canonical)
}

fn validate_terminal_preferences(
    preferences: &TerminalPreferencesDto,
) -> Result<(), ExplorerError> {
    if !(MIN_TERMINAL_PANE_HEIGHT_PERCENT..=MAX_TERMINAL_PANE_HEIGHT_PERCENT)
        .contains(&preferences.pane_height_percent)
        || !(MIN_TERMINAL_FONT_SIZE..=MAX_TERMINAL_FONT_SIZE).contains(&preferences.font_size)
        || !(MIN_TERMINAL_SCROLLBACK..=MAX_TERMINAL_SCROLLBACK).contains(&preferences.scrollback)
    {
        return Err(ExplorerError::InvalidConfiguration(
            "The terminal preferences are outside the supported range.".to_owned(),
        ));
    }
    Ok(())
}

fn recovery(
    code: &'static str,
    message: &str,
) -> (UserPreferencesDto, Option<PreferencesWarningDto>) {
    (
        UserPreferencesDto::default(),
        Some(PreferencesWarningDto {
            code,
            message: message.to_owned(),
        }),
    )
}

fn persist_preferences(path: &Path, preferences: &UserPreferencesDto) -> Result<(), ExplorerError> {
    let parent = path.parent().ok_or_else(|| {
        ExplorerError::InvalidConfiguration("The preference storage path is invalid.".to_owned())
    })?;
    fs::create_dir_all(parent)
        .map_err(|error| ExplorerError::io("create its preference directory", parent, error))?;
    let document = StoredPreferencesDocument {
        version: PREFERENCES_FILE_VERSION,
        layout: preferences.layout.clone(),
        terminal: preferences.terminal.clone(),
    };
    let bytes = serde_json::to_vec_pretty(&document).map_err(|_| {
        ExplorerError::Unexpected("Explora could not encode its preferences.".to_owned())
    })?;
    let mut temporary = NamedTempFile::new_in(parent)
        .map_err(|error| ExplorerError::io("create temporary preference storage", parent, error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| ExplorerError::io("secure preference storage", path, error))?;
    }
    temporary
        .write_all(&bytes)
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|error| ExplorerError::io("write preferences", path, error))?;
    temporary
        .persist(path)
        .map_err(|error| ExplorerError::io("replace preference storage", path, error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Arc, thread};

    use tempfile::TempDir;

    use super::*;

    fn patch(
        sidebar_collapsed: Option<bool>,
        view_mode: Option<ViewMode>,
        sort: Option<SortDescriptorDto>,
    ) -> UserPreferencesPatchDto {
        UserPreferencesPatchDto {
            layout: LayoutPreferencesPatchDto {
                sidebar_collapsed,
                view_mode,
                sort,
                favorite_roles: None,
                hidden_ssh_target_ids: None,
            },
            terminal: None,
        }
    }

    fn favorite_patch(favorite_roles: Vec<FavoriteRole>) -> UserPreferencesPatchDto {
        UserPreferencesPatchDto {
            layout: LayoutPreferencesPatchDto {
                sidebar_collapsed: None,
                view_mode: None,
                sort: None,
                favorite_roles: Some(favorite_roles),
                hidden_ssh_target_ids: None,
            },
            terminal: None,
        }
    }

    fn hidden_ssh_patch(hidden_ssh_target_ids: Vec<&str>) -> UserPreferencesPatchDto {
        UserPreferencesPatchDto {
            layout: LayoutPreferencesPatchDto {
                sidebar_collapsed: None,
                view_mode: None,
                sort: None,
                favorite_roles: None,
                hidden_ssh_target_ids: Some(
                    hidden_ssh_target_ids
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                ),
            },
            terminal: None,
        }
    }

    fn terminal_patch(
        visible: Option<bool>,
        pane_height_percent: Option<u16>,
        font_size: Option<u16>,
        scrollback: Option<u32>,
        screen_reader_mode: Option<bool>,
    ) -> UserPreferencesPatchDto {
        UserPreferencesPatchDto {
            layout: LayoutPreferencesPatchDto {
                sidebar_collapsed: None,
                view_mode: None,
                sort: None,
                favorite_roles: None,
                hidden_ssh_target_ids: None,
            },
            terminal: Some(TerminalPreferencesPatchDto {
                visible,
                pane_height_percent,
                font_size,
                scrollback,
                screen_reader_mode,
            }),
        }
    }

    #[test]
    fn missing_file_uses_layout_defaults() {
        let temp = TempDir::new().expect("temporary directory");
        let store = PreferencesStore::new(temp.path().join("preferences.json"));

        let snapshot = store.snapshot().expect("preference snapshot");
        assert_eq!(snapshot.preferences, UserPreferencesDto::default());
        assert_eq!(snapshot.warning, None);
    }

    #[test]
    fn partial_updates_round_trip_without_resetting_other_fields() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("config/preferences.json");
        let store = PreferencesStore::new(path.clone());

        store
            .update(patch(None, Some(ViewMode::Grid), None))
            .expect("update view mode");
        store
            .update(patch(
                Some(true),
                None,
                Some(SortDescriptorDto {
                    column: SortColumn::Size,
                    direction: SortDirection::Descending,
                }),
            ))
            .expect("update remaining layout");

        let reloaded = PreferencesStore::new(path)
            .snapshot()
            .expect("reloaded preferences");
        assert!(reloaded.preferences.layout.sidebar_collapsed);
        assert_eq!(reloaded.preferences.layout.view_mode, ViewMode::Grid);
        assert_eq!(reloaded.preferences.layout.sort.column, SortColumn::Size);
        assert_eq!(
            reloaded.preferences.layout.sort.direction,
            SortDirection::Descending
        );
        assert_eq!(
            reloaded.preferences.layout.favorite_roles,
            DEFAULT_FAVORITE_ROLES
        );
        assert!(reloaded.preferences.layout.hidden_ssh_target_ids.is_empty());
    }

    #[test]
    fn migrates_version_one_layout_preferences_with_default_favorites() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("preferences.json");
        fs::write(
            &path,
            br#"{"version":1,"layout":{"sidebarCollapsed":true,"viewMode":"grid","sort":{"column":"size","direction":"descending"}}}"#,
        )
        .expect("version one preferences");

        let snapshot = PreferencesStore::new(path)
            .snapshot()
            .expect("migrated preferences");
        assert!(snapshot.preferences.layout.sidebar_collapsed);
        assert_eq!(snapshot.preferences.layout.view_mode, ViewMode::Grid);
        assert_eq!(snapshot.preferences.layout.sort.column, SortColumn::Size);
        assert_eq!(
            snapshot.preferences.layout.favorite_roles,
            DEFAULT_FAVORITE_ROLES
        );
        assert!(snapshot.preferences.layout.hidden_ssh_target_ids.is_empty());
        assert_eq!(snapshot.warning, None);
    }

    #[test]
    fn migrates_version_two_favorites_with_no_hidden_ssh_targets() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("preferences.json");
        fs::write(
            &path,
            br#"{"version":2,"layout":{"sidebarCollapsed":false,"viewMode":"list","sort":{"column":"name","direction":"ascending"},"favoriteRoles":["home","documents"]}}"#,
        )
        .expect("version two preferences");

        let snapshot = PreferencesStore::new(path)
            .snapshot()
            .expect("migrated preferences");
        assert_eq!(
            snapshot.preferences.layout.favorite_roles,
            vec![FavoriteRole::Home, FavoriteRole::Documents]
        );
        assert!(snapshot.preferences.layout.hidden_ssh_target_ids.is_empty());
        assert_eq!(snapshot.warning, None);
    }

    #[test]
    fn migrates_version_three_layout_with_terminal_defaults() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("preferences.json");
        fs::write(
            &path,
            br#"{"version":3,"layout":{"sidebarCollapsed":false,"viewMode":"list","sort":{"column":"name","direction":"ascending"},"favoriteRoles":["home","documents"],"hiddenSshTargetIds":["manual:target-1"]}}"#,
        )
        .expect("version three preferences");

        let snapshot = PreferencesStore::new(path)
            .snapshot()
            .expect("migrated preferences");
        assert_eq!(
            snapshot.preferences.layout.hidden_ssh_target_ids,
            vec!["manual:target-1"]
        );
        assert_eq!(
            snapshot.preferences.terminal,
            TerminalPreferencesDto::default()
        );
        assert_eq!(snapshot.warning, None);
    }

    #[test]
    fn terminal_preferences_round_trip_and_reject_invalid_ranges() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("preferences.json");
        let store = PreferencesStore::new(path.clone());

        store
            .update(terminal_patch(
                Some(true),
                Some(40),
                Some(16),
                Some(10_000),
                Some(false),
            ))
            .expect("terminal preference update");
        let reloaded = PreferencesStore::new(path)
            .snapshot()
            .expect("reloaded preferences");
        assert_eq!(
            reloaded.preferences.terminal,
            TerminalPreferencesDto {
                visible: true,
                pane_height_percent: 40,
                font_size: 16,
                scrollback: 10_000,
                screen_reader_mode: false,
            }
        );

        let result = store.update(terminal_patch(None, Some(90), None, None, None));
        assert!(matches!(
            result,
            Err(ExplorerError::InvalidConfiguration(_))
        ));
        assert_eq!(
            store
                .snapshot()
                .expect("unchanged preferences")
                .preferences
                .terminal
                .pane_height_percent,
            40
        );
    }

    #[test]
    fn favorite_updates_are_deduplicated_in_canonical_order() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("preferences.json");
        let store = PreferencesStore::new(path.clone());

        store
            .update(favorite_patch(vec![
                FavoriteRole::Music,
                FavoriteRole::Home,
                FavoriteRole::Music,
            ]))
            .expect("favorite update");

        let reloaded = PreferencesStore::new(path)
            .snapshot()
            .expect("reloaded favorites");
        assert_eq!(
            reloaded.preferences.layout.favorite_roles,
            vec![FavoriteRole::Home, FavoriteRole::Music]
        );
    }

    #[test]
    fn hidden_ssh_targets_are_deduplicated_and_sorted() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("preferences.json");
        let store = PreferencesStore::new(path.clone());

        store
            .update(hidden_ssh_patch(vec![
                "manual:target-b",
                "config:staging",
                "manual:target-b",
            ]))
            .expect("hidden SSH update");

        let reloaded = PreferencesStore::new(path)
            .snapshot()
            .expect("reloaded hidden SSH targets");
        assert_eq!(
            reloaded.preferences.layout.hidden_ssh_target_ids,
            vec!["config:staging", "manual:target-b"]
        );
    }

    #[test]
    fn hidden_ssh_targets_reject_untrusted_identifiers() {
        let temp = TempDir::new().expect("temporary directory");
        let store = PreferencesStore::new(temp.path().join("preferences.json"));

        let result = store.update(hidden_ssh_patch(vec!["unknown:target"]));

        assert!(matches!(
            result,
            Err(ExplorerError::InvalidConfiguration(_))
        ));
        assert!(store
            .snapshot()
            .expect("unchanged preferences")
            .preferences
            .layout
            .hidden_ssh_target_ids
            .is_empty());
    }

    #[test]
    fn malformed_and_unsupported_documents_recover_non_fatally() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("preferences.json");
        fs::write(&path, b"not json").expect("malformed preferences");
        let malformed = PreferencesStore::new(path.clone())
            .snapshot()
            .expect("malformed snapshot");
        assert_eq!(
            malformed.warning.as_ref().map(|warning| warning.code),
            Some("malformed")
        );

        fs::write(&path, br#"{"version":99,"layout":{}}"#).expect("unsupported preferences");
        let unsupported = PreferencesStore::new(path)
            .snapshot()
            .expect("unsupported snapshot");
        assert_eq!(
            unsupported.warning.as_ref().map(|warning| warning.code),
            Some("unsupportedVersion")
        );
    }

    #[test]
    fn invalid_enum_values_recover_to_defaults() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("preferences.json");
        fs::write(
            &path,
            br#"{"version":1,"layout":{"sidebarCollapsed":false,"viewMode":"columns","sort":{"column":"name","direction":"ascending"}}}"#,
        )
        .expect("invalid preferences");

        let snapshot = PreferencesStore::new(path)
            .snapshot()
            .expect("preference snapshot");
        assert_eq!(snapshot.preferences, UserPreferencesDto::default());
        assert_eq!(
            snapshot.warning.as_ref().map(|warning| warning.code),
            Some("malformed")
        );
    }

    #[test]
    fn preference_patches_reject_unknown_fields() {
        let parsed = serde_json::from_str::<UserPreferencesPatchDto>(
            r#"{"layout":{"sidebarCollapsed":true,"futureField":42}}"#,
        );

        assert!(parsed.is_err());
    }

    #[test]
    fn concurrent_partial_updates_leave_a_valid_document() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("preferences.json");
        let store = Arc::new(PreferencesStore::new(path.clone()));
        let sidebar_store = store.clone();
        let sidebar = thread::spawn(move || sidebar_store.update(patch(Some(true), None, None)));
        let view_store = store.clone();
        let view =
            thread::spawn(move || view_store.update(patch(None, Some(ViewMode::Grid), None)));

        sidebar
            .join()
            .expect("sidebar thread")
            .expect("sidebar update");
        view.join().expect("view thread").expect("view update");
        let reloaded = PreferencesStore::new(path)
            .snapshot()
            .expect("reloaded preferences");
        assert!(reloaded.preferences.layout.sidebar_collapsed);
        assert_eq!(reloaded.preferences.layout.view_mode, ViewMode::Grid);
    }

    #[cfg(unix)]
    #[test]
    fn persisted_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("preferences.json");
        let store = PreferencesStore::new(path.clone());
        store
            .update(patch(Some(true), None, None))
            .expect("preference update");

        let mode = fs::metadata(path)
            .expect("preference metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
