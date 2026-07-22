use std::{
    collections::HashSet,
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use uuid::Uuid;

use crate::{
    filesystem::{
        ExplorerError, SyncedFolderMetadataDto, SyncedFolderProvider, SyncedFolderSource,
        SyncedFolderStatus,
    },
    local_filesystem::SyncedFolderRoot,
    synced_availability::SyncedAvailabilityPolicy,
};

const ROOTS_FILE_VERSION: u32 = 1;
const MAX_MANUAL_ROOTS: usize = 128;
const MAX_PATH_UNITS: usize = 16 * 1024;
const MANUAL_ID_PREFIX: &str = "synced:manual:";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "encoding", content = "data", rename_all = "camelCase")]
enum StoredPath {
    UnixBytes(Vec<u8>),
    WindowsWide(Vec<u16>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredManualRoot {
    id: String,
    name: String,
    path: StoredPath,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredManualRootsDocument {
    version: u32,
    roots: Vec<StoredManualRoot>,
}

pub(crate) struct ManualSyncedFolderStore {
    storage_path: PathBuf,
    enabled: bool,
    roots: Mutex<Vec<StoredManualRoot>>,
}

impl ManualSyncedFolderStore {
    pub(crate) fn new(storage_path: PathBuf, enabled: bool) -> Result<Self, ExplorerError> {
        let roots = if enabled {
            load_roots(&storage_path)?
        } else {
            Vec::new()
        };
        Ok(Self {
            storage_path,
            enabled,
            roots: Mutex::new(roots),
        })
    }

    pub(crate) const fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn discover(&self) -> Result<Vec<SyncedFolderRoot>, ExplorerError> {
        let stored = self
            .roots
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .clone();
        let mut roots = Vec::new();
        for stored in stored {
            let Ok(path) = decode_path(&stored.path) else {
                continue;
            };
            let status = match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_dir() => SyncedFolderStatus::Available,
                Ok(_) => SyncedFolderStatus::Error,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    SyncedFolderStatus::Offline
                }
                Err(_) => SyncedFolderStatus::Error,
            };
            roots.push(SyncedFolderRoot {
                id: location_id(&stored.id),
                name: stored.name,
                path,
                detail: if status == SyncedFolderStatus::Available {
                    "Manually added · Synced folder".to_owned()
                } else {
                    "Manually added · Folder unavailable".to_owned()
                },
                metadata: SyncedFolderMetadataDto {
                    provider: SyncedFolderProvider::Other,
                    status,
                    source: SyncedFolderSource::Manual,
                },
                availability: SyncedAvailabilityPolicy::LocalMirror,
            });
        }
        Ok(roots)
    }

    pub(crate) fn add(&self, selected_path: PathBuf) -> Result<String, ExplorerError> {
        if !self.enabled {
            return Err(ExplorerError::Unsupported(
                "Adding synced folders manually is not supported on this platform.".to_owned(),
            ));
        }
        let path = validate_selected_path(selected_path)?;
        let encoded = encode_path(&path)?;
        let mut roots = self
            .roots
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        for root in roots.iter() {
            if decode_path(&root.path).ok().as_ref() == Some(&path) {
                return Ok(location_id(&root.id));
            }
        }
        if roots.len() >= MAX_MANUAL_ROOTS {
            return Err(ExplorerError::InvalidConfiguration(
                "Explora cannot save more manually added synced folders.".to_owned(),
            ));
        }

        let id = Uuid::new_v4().to_string();
        let name = next_manual_name(&roots);
        let mut updated = roots.clone();
        updated.push(StoredManualRoot {
            id: id.clone(),
            name,
            path: encoded,
        });
        persist_roots(&self.storage_path, &updated)?;
        *roots = updated;
        Ok(location_id(&id))
    }

    pub(crate) fn remove(&self, folder_id: &str) -> Result<(), ExplorerError> {
        if !self.enabled {
            return Err(ExplorerError::Unsupported(
                "Removing manually added synced folders is not supported on this platform."
                    .to_owned(),
            ));
        }
        let id = folder_id
            .strip_prefix(MANUAL_ID_PREFIX)
            .filter(|id| Uuid::parse_str(id).is_ok())
            .ok_or(ExplorerError::InvalidReference)?;
        let mut roots = self
            .roots
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let mut updated = roots.clone();
        let original_len = updated.len();
        updated.retain(|root| root.id != id);
        if updated.len() == original_len {
            return Err(ExplorerError::InvalidReference);
        }
        persist_roots(&self.storage_path, &updated)?;
        *roots = updated;
        Ok(())
    }
}

fn location_id(id: &str) -> String {
    format!("{MANUAL_ID_PREFIX}{id}")
}

fn validate_selected_path(path: PathBuf) -> Result<PathBuf, ExplorerError> {
    if !path.is_absolute() {
        return Err(ExplorerError::InvalidConfiguration(
            "The selected synced folder path is not absolute.".to_owned(),
        ));
    }
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| ExplorerError::io("inspect the selected synced folder", &path, error))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(ExplorerError::InvalidConfiguration(
            "Select a real directory rather than a file or symbolic link.".to_owned(),
        ));
    }
    fs::canonicalize(&path)
        .map_err(|error| ExplorerError::io("resolve the selected synced folder", &path, error))
}

fn next_manual_name(roots: &[StoredManualRoot]) -> String {
    (1..=MAX_MANUAL_ROOTS)
        .map(|index| format!("Synced Folder {index}"))
        .find(|name| roots.iter().all(|root| root.name != *name))
        .unwrap_or_else(|| "Synced Folder".to_owned())
}

#[cfg(unix)]
fn encode_path(path: &Path) -> Result<StoredPath, ExplorerError> {
    use std::os::unix::ffi::OsStrExt;

    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_PATH_UNITS || bytes.contains(&0) {
        return Err(invalid_stored_path());
    }
    Ok(StoredPath::UnixBytes(bytes.to_vec()))
}

#[cfg(windows)]
fn encode_path(path: &Path) -> Result<StoredPath, ExplorerError> {
    use std::os::windows::ffi::OsStrExt;

    let units = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if units.is_empty() || units.len() > MAX_PATH_UNITS || units.contains(&0) {
        return Err(invalid_stored_path());
    }
    Ok(StoredPath::WindowsWide(units))
}

#[cfg(unix)]
fn decode_path(path: &StoredPath) -> Result<PathBuf, ExplorerError> {
    use std::os::unix::ffi::OsStringExt;

    let StoredPath::UnixBytes(bytes) = path else {
        return Err(invalid_stored_path());
    };
    if bytes.is_empty() || bytes.len() > MAX_PATH_UNITS || bytes.contains(&0) {
        return Err(invalid_stored_path());
    }
    Ok(PathBuf::from(OsString::from_vec(bytes.clone())))
}

#[cfg(windows)]
fn decode_path(path: &StoredPath) -> Result<PathBuf, ExplorerError> {
    use std::os::windows::ffi::OsStringExt;

    let StoredPath::WindowsWide(units) = path else {
        return Err(invalid_stored_path());
    };
    if units.is_empty() || units.len() > MAX_PATH_UNITS || units.contains(&0) {
        return Err(invalid_stored_path());
    }
    Ok(PathBuf::from(OsString::from_wide(units)))
}

fn invalid_stored_path() -> ExplorerError {
    ExplorerError::InvalidConfiguration("Explora's saved synced-folder path is invalid.".to_owned())
}

fn load_roots(path: &Path) -> Result<Vec<StoredManualRoot>, ExplorerError> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let bytes = fs::read(path)
        .map_err(|error| ExplorerError::io("read saved synced folders", path, error))?;
    let document: StoredManualRootsDocument = serde_json::from_slice(&bytes).map_err(|_| {
        ExplorerError::InvalidConfiguration(
            "Explora's saved synced-folder file is malformed.".to_owned(),
        )
    })?;
    if document.version != ROOTS_FILE_VERSION || document.roots.len() > MAX_MANUAL_ROOTS {
        return Err(ExplorerError::InvalidConfiguration(
            "Explora's saved synced-folder file has an unsupported version or size.".to_owned(),
        ));
    }

    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    let mut paths = HashSet::new();
    for root in &document.roots {
        let decoded_path = decode_path(&root.path)?;
        if Uuid::parse_str(&root.id).is_err()
            || !ids.insert(root.id.clone())
            || !valid_manual_name(&root.name)
            || !names.insert(root.name.clone())
            || !decoded_path.is_absolute()
            || !paths.insert(decoded_path)
        {
            return Err(ExplorerError::InvalidConfiguration(
                "Explora's saved synced-folder file contains duplicate or invalid entries."
                    .to_owned(),
            ));
        }
    }
    Ok(document.roots)
}

fn valid_manual_name(name: &str) -> bool {
    name.strip_prefix("Synced Folder ")
        .and_then(|index| index.parse::<usize>().ok())
        .is_some_and(|index| (1..=MAX_MANUAL_ROOTS).contains(&index))
}

fn persist_roots(path: &Path, roots: &[StoredManualRoot]) -> Result<(), ExplorerError> {
    let parent = path.parent().ok_or_else(|| {
        ExplorerError::InvalidConfiguration("The synced-folder storage path is invalid.".to_owned())
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        ExplorerError::io(
            "create the synced-folder configuration directory",
            parent,
            error,
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(|error| {
            ExplorerError::io(
                "secure the synced-folder configuration directory",
                parent,
                error,
            )
        })?;
    }
    let bytes = serde_json::to_vec_pretty(&StoredManualRootsDocument {
        version: ROOTS_FILE_VERSION,
        roots: roots.to_vec(),
    })
    .map_err(|_| {
        ExplorerError::Unexpected("Explora could not encode its saved synced folders.".to_owned())
    })?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(|error| {
        ExplorerError::io("create temporary synced-folder storage", parent, error)
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| ExplorerError::io("secure synced-folder storage", path, error))?;
    }
    temporary
        .write_all(&bytes)
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|error| ExplorerError::io("write saved synced folders", path, error))?;
    temporary
        .persist(path)
        .map_err(|error| ExplorerError::io("replace saved synced folders", path, error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn persists_private_paths_and_exposes_only_opaque_ids() {
        let temp = TempDir::new().expect("temporary directory");
        let selected = temp.path().join("private-account-folder");
        fs::create_dir(&selected).expect("selected folder");
        let storage = temp.path().join("config/manual-synced-folders.json");
        let store = ManualSyncedFolderStore::new(storage.clone(), true).expect("store");

        let id = store.add(selected.clone()).expect("added folder");
        assert!(id.starts_with(MANUAL_ID_PREFIX));
        assert!(!id.contains("private-account"));
        let roots = store.discover().expect("discovered roots");
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].id, id);
        assert_eq!(roots[0].path, selected.canonicalize().expect("canonical"));
        assert_eq!(roots[0].availability, SyncedAvailabilityPolicy::LocalMirror);

        let reopened = ManualSyncedFolderStore::new(storage, true).expect("reopened store");
        assert_eq!(reopened.discover().expect("reloaded roots")[0].id, id);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(temp.path().join("config"))
                    .expect("configuration directory")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(temp.path().join("config/manual-synced-folders.json"))
                    .expect("configuration file")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn deduplicates_paths_and_removes_only_manual_ids() {
        let temp = TempDir::new().expect("temporary directory");
        let selected = temp.path().join("selected");
        fs::create_dir(&selected).expect("selected folder");
        let store = ManualSyncedFolderStore::new(
            temp.path().join("config/manual-synced-folders.json"),
            true,
        )
        .expect("store");

        let first = store.add(selected.clone()).expect("first add");
        assert_eq!(store.add(selected).expect("duplicate add"), first);
        assert_eq!(store.discover().expect("roots").len(), 1);
        let second_path = temp.path().join("selected-two");
        fs::create_dir(&second_path).expect("second selected folder");
        let second = store.add(second_path).expect("second add");
        assert!(matches!(
            store.remove("synced:system-root"),
            Err(ExplorerError::InvalidReference)
        ));
        store.remove(&first).expect("remove manual root");
        let remaining = store.discover().expect("remaining roots");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name, "Synced Folder 2");
        store.remove(&second).expect("remove second root");
        assert!(store.discover().expect("roots after removal").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_selected_symlinks_and_keeps_unavailable_roots_offline() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temporary directory");
        let real = temp.path().join("real");
        let link = temp.path().join("link");
        fs::create_dir(&real).expect("real folder");
        symlink(&real, &link).expect("folder symlink");
        let store = ManualSyncedFolderStore::new(
            temp.path().join("config/manual-synced-folders.json"),
            true,
        )
        .expect("store");

        assert!(matches!(
            store.add(link),
            Err(ExplorerError::InvalidConfiguration(_))
        ));
        store.add(real.clone()).expect("real root");
        fs::remove_dir(real).expect("remove root");
        let roots = store.discover().expect("unavailable roots");
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].metadata.status, SyncedFolderStatus::Offline);
    }

    #[cfg(unix)]
    #[test]
    fn round_trips_non_utf8_paths() {
        use std::os::unix::ffi::OsStringExt;

        let temp = TempDir::new().expect("temporary directory");
        let selected = temp
            .path()
            .join(OsString::from_vec(b"synced-\xFF-folder".to_vec()));

        let encoded = encode_path(&selected).expect("encode non-UTF-8 path");
        assert_eq!(decode_path(&encoded).expect("decode path"), selected);
    }
}
