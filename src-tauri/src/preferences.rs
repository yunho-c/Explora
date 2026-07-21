use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::filesystem::ExplorerError;

const PREFERENCES_FILE_VERSION: u32 = 1;

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

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SortDescriptorDto {
    pub column: SortColumn,
    pub direction: SortDirection,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayoutPreferencesDto {
    pub sidebar_collapsed: bool,
    pub view_mode: ViewMode,
    pub sort: SortDescriptorDto,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserPreferencesDto {
    pub layout: LayoutPreferencesDto,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayoutPreferencesPatchDto {
    pub sidebar_collapsed: Option<bool>,
    pub view_mode: Option<ViewMode>,
    pub sort: Option<SortDescriptorDto>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserPreferencesPatchDto {
    pub layout: LayoutPreferencesPatchDto,
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
    if version != PREFERENCES_FILE_VERSION {
        return recovery(
            "unsupportedVersion",
            "Explora could not use preferences saved by a different application version and restored defaults.",
        );
    }

    let document = match serde_json::from_slice::<StoredPreferencesDocument>(&bytes) {
        Ok(document) => document,
        Err(_) => {
            return recovery(
                "malformed",
                "Explora's saved preferences were malformed and defaults were restored.",
            );
        }
    };

    (
        UserPreferencesDto {
            layout: document.layout,
        },
        None,
    )
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
            },
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
