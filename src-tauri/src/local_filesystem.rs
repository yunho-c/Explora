use std::{
    collections::HashMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, RwLock,
    },
    time::UNIX_EPOCH,
};

use uuid::Uuid;

use crate::content_request::ContentRequestPolicy;
use crate::filesystem::{
    BreadcrumbSegmentDto, ContentAvailability, DirectoryListingEvent, DirectoryRefDto, EntryRefDto,
    ExplorerError, FileEntrySummaryDto, LocationBackend, LocationRole, LocationSummaryDto,
    SyncedFolderMetadataDto, LISTING_BATCH_SIZE,
};
use crate::synced_availability::{SyncedAvailabilityInspector, SyncedAvailabilityPolicy};

#[derive(Debug, Clone)]
pub struct LocalRoot {
    pub id: &'static str,
    pub name: &'static str,
    pub role: LocationRole,
    pub path: PathBuf,
}

#[derive(Default)]
struct PathRegistryInner {
    paths_by_id: HashMap<String, PathBuf>,
    locations_by_id: HashMap<String, String>,
    ids_by_path: HashMap<(String, PathBuf), String>,
}

#[derive(Default)]
struct PathRegistry {
    inner: Mutex<PathRegistryInner>,
}

impl PathRegistry {
    fn register(&self, location_id: &str, path: PathBuf) -> Result<String, ExplorerError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;

        let key = (location_id.to_owned(), path.clone());
        if let Some(id) = inner.ids_by_path.get(&key) {
            return Ok(id.clone());
        }

        let id = Uuid::new_v4().to_string();
        inner.paths_by_id.insert(id.clone(), path.clone());
        inner
            .locations_by_id
            .insert(id.clone(), location_id.to_owned());
        inner.ids_by_path.insert(key, id.clone());
        Ok(id)
    }

    fn resolve(&self, location_id: &str, id: &str) -> Result<PathBuf, ExplorerError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        if inner.locations_by_id.get(id).map(String::as_str) != Some(location_id) {
            return Err(ExplorerError::InvalidReference);
        }
        inner
            .paths_by_id
            .get(id)
            .cloned()
            .ok_or(ExplorerError::InvalidReference)
    }

    fn remove_location(&self, location_id: &str) -> Result<(), ExplorerError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let removed_ids = inner
            .locations_by_id
            .iter()
            .filter_map(|(id, registered_location)| {
                (registered_location == location_id).then_some(id.clone())
            })
            .collect::<Vec<_>>();
        for id in removed_ids {
            if let Some(path) = inner.paths_by_id.remove(&id) {
                inner.ids_by_path.remove(&(location_id.to_owned(), path));
            }
            inner.locations_by_id.remove(&id);
        }
        Ok(())
    }
}

pub struct LocalFilesystem {
    registry: PathRegistry,
    locations: RwLock<Vec<LocationSummaryDto>>,
    synced_availability: RwLock<HashMap<String, SyncedAvailabilityPolicy>>,
    availability_inspector: SyncedAvailabilityInspector,
}

pub(crate) struct LocalPreviewAccess {
    pub path: PathBuf,
    pub availability: ContentAvailability,
    pub content_request_policy: Option<ContentRequestPolicy>,
    pub size: Option<String>,
    pub modified_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeRoot {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncedFolderRoot {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub detail: String,
    pub metadata: SyncedFolderMetadataDto,
    pub availability: SyncedAvailabilityPolicy,
}

impl LocalFilesystem {
    pub fn new(roots: Vec<LocalRoot>) -> Result<Self, ExplorerError> {
        let registry = PathRegistry::default();
        let mut locations = Vec::new();
        let mut seen_paths = Vec::<PathBuf>::new();

        for root in roots {
            if !root.path.is_dir() || seen_paths.iter().any(|path| path == &root.path) {
                continue;
            }

            seen_paths.push(root.path.clone());
            let directory = directory_ref(&registry, &root.path, root.id, Some(root.name))?;
            locations.push(LocationSummaryDto {
                id: root.id.to_owned(),
                name: root.name.to_owned(),
                backend: LocationBackend::Local,
                kind: "local",
                role: root.role,
                status: "available",
                display_path: directory.display_path.clone(),
                detail: "Local".to_owned(),
                root: directory,
                synced_folder: None,
            });
        }

        if locations.is_empty() {
            return Err(ExplorerError::Io {
                message: "Explora could not find an available local home directory.".to_owned(),
                kind: std::io::ErrorKind::NotFound,
            });
        }

        Ok(Self {
            registry,
            locations: RwLock::new(locations),
            synced_availability: RwLock::new(HashMap::new()),
            availability_inspector: SyncedAvailabilityInspector::default(),
        })
    }

    pub fn locations(&self) -> Result<Vec<LocationSummaryDto>, ExplorerError> {
        self.locations
            .read()
            .map(|locations| locations.clone())
            .map_err(|_| ExplorerError::StateUnavailable)
    }

    pub fn contains_location(&self, location_id: &str) -> Result<bool, ExplorerError> {
        self.locations
            .read()
            .map(|locations| locations.iter().any(|location| location.id == location_id))
            .map_err(|_| ExplorerError::StateUnavailable)
    }

    pub fn is_synced_folder(&self, location_id: &str) -> Result<bool, ExplorerError> {
        self.locations
            .read()
            .map(|locations| {
                locations
                    .iter()
                    .any(|location| location.id == location_id && location.kind == "syncedFolder")
            })
            .map_err(|_| ExplorerError::StateUnavailable)
    }

    pub fn replace_volumes(
        &self,
        volumes: Vec<VolumeRoot>,
    ) -> Result<Vec<LocationSummaryDto>, ExplorerError> {
        let mut locations = self
            .locations
            .write()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let previous = locations
            .iter()
            .filter(|location| location.kind == "volume")
            .map(|location| (location.id.clone(), location.clone()))
            .collect::<HashMap<_, _>>();
        locations.retain(|location| location.kind != "volume");

        let mut summaries = Vec::with_capacity(volumes.len());
        for volume in volumes {
            if !volume.path.is_dir() {
                continue;
            }
            let display_path = volume.path.display().to_string();
            let directory = if let Some(existing) = previous
                .get(&volume.id)
                .filter(|existing| existing.root.display_path == display_path)
            {
                let mut root = existing.root.clone();
                root.name = volume.name.clone();
                root
            } else {
                self.registry.remove_location(&volume.id)?;
                directory_ref(&self.registry, &volume.path, &volume.id, Some(&volume.name))?
            };
            summaries.push(LocationSummaryDto {
                id: volume.id,
                name: volume.name,
                backend: LocationBackend::Local,
                kind: "volume",
                role: LocationRole::Volume,
                status: "available",
                display_path: directory.display_path.clone(),
                detail: volume.detail,
                root: directory,
                synced_folder: None,
            });
        }
        for id in previous.keys() {
            if !summaries.iter().any(|location| &location.id == id) {
                self.registry.remove_location(id)?;
            }
        }
        locations.extend(summaries.iter().cloned());
        Ok(summaries)
    }

    pub fn replace_synced_folders(
        &self,
        folders: Vec<SyncedFolderRoot>,
    ) -> Result<Vec<LocationSummaryDto>, ExplorerError> {
        let mut locations = self
            .locations
            .write()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let previous = locations
            .iter()
            .filter(|location| location.kind == "syncedFolder")
            .map(|location| (location.id.clone(), location.clone()))
            .collect::<HashMap<_, _>>();
        locations.retain(|location| location.kind != "syncedFolder");
        let mut availability = self
            .synced_availability
            .write()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let mut next_availability = HashMap::new();

        let mut summaries = Vec::with_capacity(folders.len());
        for folder in folders {
            if folder.metadata.status == crate::filesystem::SyncedFolderStatus::Available
                && !folder.path.is_dir()
            {
                continue;
            }
            let display_path = folder.path.display().to_string();
            let directory = if let Some(existing) = previous
                .get(&folder.id)
                .filter(|existing| existing.root.display_path == display_path)
            {
                let mut root = existing.root.clone();
                root.name = folder.name.clone();
                root
            } else {
                self.registry.remove_location(&folder.id)?;
                directory_ref(&self.registry, &folder.path, &folder.id, Some(&folder.name))?
            };
            next_availability.insert(folder.id.clone(), folder.availability);
            summaries.push(LocationSummaryDto {
                id: folder.id,
                name: folder.name,
                backend: LocationBackend::Local,
                kind: "syncedFolder",
                role: LocationRole::SyncedFolder,
                status: if folder.metadata.status
                    == crate::filesystem::SyncedFolderStatus::Available
                {
                    "available"
                } else {
                    "offline"
                },
                display_path: directory.display_path.clone(),
                detail: folder.detail,
                root: directory,
                synced_folder: Some(folder.metadata),
            });
        }
        *availability = next_availability;
        for id in previous.keys() {
            if !summaries.iter().any(|location| &location.id == id) {
                self.registry.remove_location(id)?;
            }
        }
        locations.extend(summaries.iter().cloned());
        Ok(summaries)
    }

    pub(crate) fn resolve_preview_access(
        &self,
        entry_id: &str,
        location_id: &str,
    ) -> Result<LocalPreviewAccess, ExplorerError> {
        let (location, synced_availability) = self.location_with_availability(location_id)?;
        if location.status == "offline" {
            return Err(ExplorerError::Offline(
                "This synced folder is currently unavailable.".to_owned(),
            ));
        }
        let path = self.registry.resolve(location_id, entry_id)?;
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| ExplorerError::io("inspect", path.as_path(), error))?;
        let size = metadata
            .file_type()
            .is_file()
            .then(|| metadata.len().to_string());
        let modified_at = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .and_then(|duration| u64::try_from(duration.as_millis()).ok());
        let availability = if metadata.file_type().is_file() {
            synced_availability
                .map(|policy| {
                    self.availability_inspector
                        .inspect(policy, path.as_path(), false)
                })
                .unwrap_or(ContentAvailability::Local)
        } else {
            // Directories, symlinks, and special entries have no file content
            // for the preview pipeline to hydrate. The previewer will return
            // metadata without following or opening them.
            ContentAvailability::Local
        };

        Ok(LocalPreviewAccess {
            path,
            availability,
            content_request_policy: synced_availability
                .and_then(SyncedAvailabilityPolicy::content_request_policy),
            size,
            modified_at,
        })
    }

    pub fn list_directory<F>(
        &self,
        directory_id: &str,
        location_id: &str,
        cancelled: &AtomicBool,
        mut emit: F,
    ) -> Result<(), ExplorerError>
    where
        F: FnMut(DirectoryListingEvent) -> Result<(), ExplorerError>,
    {
        ensure_not_cancelled(cancelled)?;
        let (location, synced_availability) = self.location_with_availability(location_id)?;
        if location.status == "offline" {
            return Err(ExplorerError::Offline(
                "This synced folder is currently unavailable.".to_owned(),
            ));
        }
        let root_path = self.registry.resolve(location_id, &location.root.id)?;
        let path = self.registry.resolve(location_id, directory_id)?;
        if !path.starts_with(&root_path) {
            return Err(ExplorerError::InvalidReference);
        }
        let read_dir = fs::read_dir(&path)
            .map_err(|error| ExplorerError::io("open", path.as_path(), error))?;

        let directory = directory_ref(&self.registry, &path, location_id, None)?;
        let parent = (path != root_path)
            .then(|| path.parent())
            .flatten()
            .filter(|parent| parent.starts_with(&root_path))
            .map(|parent| directory_ref(&self.registry, parent, location_id, None))
            .transpose()?;
        let breadcrumbs = breadcrumbs(
            &self.registry,
            &path,
            &root_path,
            location_id,
            &location.name,
        )?;

        emit(DirectoryListingEvent::Started {
            directory,
            parent,
            breadcrumbs,
        })?;

        let mut batch = Vec::with_capacity(LISTING_BATCH_SIZE);
        let mut replace = true;
        let mut skipped_entries = 0;

        for entry in read_dir {
            ensure_not_cancelled(cancelled)?;

            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    skipped_entries += 1;
                    continue;
                }
            };

            match self.describe_entry(entry, location_id, synced_availability) {
                Ok(entry) => batch.push(entry),
                Err(_) => {
                    skipped_entries += 1;
                    continue;
                }
            }

            if batch.len() == LISTING_BATCH_SIZE {
                ensure_not_cancelled(cancelled)?;
                emit(DirectoryListingEvent::Entries {
                    entries: std::mem::take(&mut batch),
                    replace,
                })?;
                replace = false;
            }
        }

        if !batch.is_empty() {
            ensure_not_cancelled(cancelled)?;
            emit(DirectoryListingEvent::Entries {
                entries: batch,
                replace,
            })?;
        }

        ensure_not_cancelled(cancelled)?;
        emit(DirectoryListingEvent::Complete { skipped_entries })?;
        Ok(())
    }

    fn describe_entry(
        &self,
        entry: fs::DirEntry,
        location_id: &str,
        synced_availability: Option<SyncedAvailabilityPolicy>,
    ) -> Result<FileEntrySummaryDto, ExplorerError> {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let file_type = entry
            .file_type()
            .map_err(|error| ExplorerError::io("inspect", path.as_path(), error))?;
        let metadata = fs::symlink_metadata(&path).ok();
        let symlink_target_is_directory = file_type.is_symlink()
            && fs::metadata(&path)
                .map(|target| target.is_dir())
                .unwrap_or(false);
        let is_navigable = file_type.is_dir() || symlink_target_is_directory;
        let id = self.registry.register(location_id, path.clone())?;
        let directory = is_navigable.then(|| DirectoryRefDto {
            id: id.clone(),
            location_id: location_id.to_owned(),
            name: name.clone(),
            display_path: display_path(&path),
        });
        let kind = if file_type.is_dir() {
            "directory"
        } else if file_type.is_symlink() {
            "symlink"
        } else if file_type.is_file() {
            "file"
        } else {
            "other"
        };
        let size = (file_type.is_file())
            .then(|| metadata.as_ref().map(|metadata| metadata.len().to_string()))
            .flatten();
        let modified_at = metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .and_then(|duration| u64::try_from(duration.as_millis()).ok());
        let availability = if file_type.is_file() {
            synced_availability
                .map(|policy| {
                    self.availability_inspector
                        .inspect(policy, path.as_path(), false)
                })
                .unwrap_or(ContentAvailability::Local)
        } else {
            ContentAvailability::Local
        };

        Ok(FileEntrySummaryDto {
            reference: EntryRefDto {
                id,
                location_id: location_id.to_owned(),
            },
            name,
            kind,
            content_kind: content_kind(&path, is_navigable),
            size,
            modified_at,
            display_path: display_path(&path),
            directory,
            availability,
            detail: file_type.is_symlink().then_some("Symbolic link"),
        })
    }

    fn location_with_availability(
        &self,
        location_id: &str,
    ) -> Result<(LocationSummaryDto, Option<SyncedAvailabilityPolicy>), ExplorerError> {
        // Keep the location read lock while reading the policy. Synced-folder
        // replacement takes the same locks in write order, so callers never
        // observe a new location paired with stale availability behavior.
        let locations = self
            .locations
            .read()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let location = locations
            .iter()
            .find(|location| location.id == location_id)
            .cloned()
            .ok_or(ExplorerError::InvalidReference)?;
        let availability = self
            .synced_availability
            .read()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .get(location_id)
            .copied();
        Ok((location, availability))
    }
}

fn ensure_not_cancelled(cancelled: &AtomicBool) -> Result<(), ExplorerError> {
    if cancelled.load(Ordering::Relaxed) {
        Err(ExplorerError::Cancelled)
    } else {
        Ok(())
    }
}

fn directory_ref(
    registry: &PathRegistry,
    path: &Path,
    location_id: &str,
    preferred_name: Option<&str>,
) -> Result<DirectoryRefDto, ExplorerError> {
    Ok(DirectoryRefDto {
        id: registry.register(location_id, path.to_path_buf())?,
        location_id: location_id.to_owned(),
        name: preferred_name
            .map(str::to_owned)
            .unwrap_or_else(|| directory_name(path)),
        display_path: display_path(path),
    })
}

fn breadcrumbs(
    registry: &PathRegistry,
    path: &Path,
    root: &Path,
    location_id: &str,
    root_name: &str,
) -> Result<Vec<BreadcrumbSegmentDto>, ExplorerError> {
    path.ancestors()
        .take_while(|ancestor| ancestor.starts_with(root))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|ancestor| {
            let directory = directory_ref(
                registry,
                ancestor,
                location_id,
                (ancestor == root).then_some(root_name),
            )?;
            Ok(BreadcrumbSegmentDto {
                label: directory.name.clone(),
                directory,
            })
        })
        .collect()
}

fn directory_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| display_path(path))
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn content_kind(path: &Path, is_directory: bool) -> &'static str {
    if is_directory {
        return "folder";
    }

    match path
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "svg") => "image",
        Some(
            "rs" | "ts" | "tsx" | "js" | "jsx" | "svelte" | "html" | "css" | "scss" | "json"
            | "toml" | "yaml" | "yml" | "py" | "go" | "c" | "h" | "cpp" | "hpp" | "java" | "kt"
            | "swift" | "sh",
        ) => "code",
        Some("txt" | "md" | "pdf" | "rtf" | "doc" | "docx" | "odt" | "pages") => "document",
        Some("mp3" | "m4a" | "aac" | "wav" | "flac" | "ogg") => "audio",
        Some("mp4" | "m4v" | "mov" | "mkv" | "webm" | "avi") => "video",
        Some("zip" | "tar" | "gz" | "bz2" | "xz" | "zst" | "7z" | "rar") => "archive",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Write};

    use tempfile::TempDir;

    use crate::filesystem::{ExplorerErrorCode, ExplorerErrorDto};

    use super::*;

    fn fixture() -> (TempDir, LocalFilesystem, DirectoryRefDto) {
        let temp = TempDir::new().expect("temporary directory");
        fs::create_dir(temp.path().join("folder")).expect("fixture folder");
        let mut file = File::create(temp.path().join("notes.md")).expect("fixture file");
        file.write_all(b"hello").expect("fixture contents");

        let filesystem = LocalFilesystem::new(vec![LocalRoot {
            id: "home",
            name: "Home",
            role: LocationRole::Home,
            path: temp.path().to_path_buf(),
        }])
        .expect("local filesystem");
        let root = filesystem.locations().expect("locations")[0].root.clone();
        (temp, filesystem, root)
    }

    #[test]
    fn lists_entries_with_opaque_references_and_metadata() {
        let (_temp, filesystem, root) = fixture();
        let cancelled = AtomicBool::new(false);
        let mut events = Vec::new();

        filesystem
            .list_directory(&root.id, &root.location_id, &cancelled, |event| {
                events.push(event);
                Ok(())
            })
            .expect("directory listing");

        let entries = events
            .iter()
            .find_map(|event| match event {
                DirectoryListingEvent::Entries { entries, .. } => Some(entries),
                _ => None,
            })
            .expect("entries event");
        let started = events
            .iter()
            .find_map(|event| match event {
                DirectoryListingEvent::Started {
                    directory,
                    parent,
                    breadcrumbs,
                } => Some((directory, parent, breadcrumbs)),
                _ => None,
            })
            .expect("started event");
        let folder = entries
            .iter()
            .find(|entry| entry.name == "folder")
            .expect("folder");
        let file = entries
            .iter()
            .find(|entry| entry.name == "notes.md")
            .expect("file");

        assert_eq!(folder.kind, "directory");
        assert!(folder.directory.is_some());
        assert_eq!(file.size.as_deref(), Some("5"));
        assert_eq!(file.content_kind, "document");
        assert!(!file.reference.id.contains("notes.md"));
        assert_eq!(started.0.id, root.id);
        assert!(started.1.is_none());
        assert_eq!(
            started.2.last().map(|item| &item.directory.id),
            Some(&root.id)
        );
    }

    #[test]
    fn streams_large_directories_and_cancels_after_a_batch() {
        let temp = TempDir::new().expect("temporary directory");
        for index in 0..(LISTING_BATCH_SIZE + 5) {
            File::create(temp.path().join(format!("file-{index}"))).expect("fixture file");
        }
        let filesystem = LocalFilesystem::new(vec![LocalRoot {
            id: "home",
            name: "Home",
            role: LocationRole::Home,
            path: temp.path().to_path_buf(),
        }])
        .expect("local filesystem");
        let root = filesystem.locations().expect("locations")[0].root.clone();
        let cancelled = AtomicBool::new(false);
        let mut batch_sizes = Vec::new();

        let result = filesystem.list_directory(&root.id, &root.location_id, &cancelled, |event| {
            if let DirectoryListingEvent::Entries { entries, .. } = event {
                batch_sizes.push(entries.len());
                cancelled.store(true, Ordering::Relaxed);
            }
            Ok(())
        });

        assert!(matches!(result, Err(ExplorerError::Cancelled)));
        assert_eq!(batch_sizes, vec![LISTING_BATCH_SIZE]);
    }

    #[test]
    fn rejects_unknown_path_tokens() {
        let (_temp, filesystem, root) = fixture();
        let result = filesystem.list_directory(
            "not-a-registered-token",
            &root.location_id,
            &AtomicBool::new(false),
            |_| Ok(()),
        );

        assert!(matches!(result, Err(ExplorerError::InvalidReference)));
    }

    #[test]
    fn rejects_unknown_location_identity() {
        let (_temp, filesystem, root) = fixture();
        let result =
            filesystem.list_directory(&root.id, "forged-location", &AtomicBool::new(false), |_| {
                Ok(())
            });

        assert!(matches!(result, Err(ExplorerError::InvalidReference)));
    }

    #[test]
    fn rejects_tokens_claimed_under_another_location() {
        let temp = TempDir::new().expect("temporary directory");
        let home = temp.path().join("home");
        let desktop = temp.path().join("desktop");
        fs::create_dir(&home).expect("home fixture");
        fs::create_dir(&desktop).expect("desktop fixture");
        let filesystem = LocalFilesystem::new(vec![
            LocalRoot {
                id: "home",
                name: "Home",
                role: LocationRole::Home,
                path: home,
            },
            LocalRoot {
                id: "desktop",
                name: "Desktop",
                role: LocationRole::Desktop,
                path: desktop,
            },
        ])
        .expect("local filesystem");
        let locations = filesystem.locations().expect("locations");
        let home_root = &locations[0].root;

        let result = filesystem.list_directory(
            &home_root.id,
            "desktop",
            &AtomicBool::new(false),
            |_| Ok(()),
        );

        assert!(matches!(result, Err(ExplorerError::InvalidReference)));
    }

    #[test]
    fn invalidates_volume_tokens_when_the_volume_disappears() {
        let (temp, filesystem, _) = fixture();
        let mount = temp.path().join("mounted-volume");
        fs::create_dir(&mount).expect("volume fixture");
        let volume = filesystem
            .replace_volumes(vec![VolumeRoot {
                id: "volume:test".to_owned(),
                name: "Test Volume".to_owned(),
                path: mount,
                detail: "1 GB available".to_owned(),
            }])
            .expect("volume snapshot")
            .remove(0);

        filesystem
            .replace_volumes(vec![VolumeRoot {
                id: volume.id.clone(),
                name: "Renamed Volume".to_owned(),
                path: temp.path().join("mounted-volume"),
                detail: "900 MB available".to_owned(),
            }])
            .expect("volume metadata refresh");
        filesystem
            .list_directory(&volume.root.id, &volume.id, &AtomicBool::new(false), |_| {
                Ok(())
            })
            .expect("unchanged mount keeps its authorized tokens");

        filesystem
            .replace_volumes(Vec::new())
            .expect("volume removal");
        let result =
            filesystem.list_directory(&volume.root.id, &volume.id, &AtomicBool::new(false), |_| {
                Ok(())
            });

        assert!(matches!(result, Err(ExplorerError::InvalidReference)));
    }

    #[test]
    fn registers_synced_roots_conservatively_and_revokes_removed_tokens() {
        let (temp, filesystem, _) = fixture();
        let synced_path = temp.path().join("synced-root");
        fs::create_dir(&synced_path).expect("synced root fixture");
        File::create(synced_path.join("placeholder.txt")).expect("synced entry fixture");
        let root = SyncedFolderRoot {
            id: "synced:test".to_owned(),
            name: "Cloud Storage".to_owned(),
            path: synced_path,
            detail: "Cloud Storage · Synced folder".to_owned(),
            metadata: SyncedFolderMetadataDto {
                provider: crate::filesystem::SyncedFolderProvider::Other,
                status: crate::filesystem::SyncedFolderStatus::Available,
                source: crate::filesystem::SyncedFolderSource::System,
            },
            availability: SyncedAvailabilityPolicy::Unknown,
        };

        let first = filesystem
            .replace_synced_folders(vec![root.clone()])
            .expect("first synced snapshot")
            .remove(0);
        assert_eq!(first.backend, LocationBackend::Local);
        assert_eq!(first.role, LocationRole::SyncedFolder);
        assert_eq!(first.synced_folder, Some(root.metadata.clone()));

        let refreshed = filesystem
            .replace_synced_folders(vec![SyncedFolderRoot {
                name: "Renamed Cloud Storage".to_owned(),
                ..root
            }])
            .expect("synced metadata refresh")
            .remove(0);
        assert_eq!(refreshed.root.id, first.root.id);

        let mut availability = None;
        filesystem
            .list_directory(
                &refreshed.root.id,
                &refreshed.id,
                &AtomicBool::new(false),
                |event| {
                    if let DirectoryListingEvent::Entries { entries, .. } = event {
                        availability = entries.first().map(|entry| entry.availability);
                    }
                    Ok(())
                },
            )
            .expect("synced root listing");
        assert_eq!(availability, Some(ContentAvailability::Unknown));

        filesystem
            .replace_synced_folders(Vec::new())
            .expect("synced root removal");
        let result = filesystem.list_directory(
            &refreshed.root.id,
            &refreshed.id,
            &AtomicBool::new(false),
            |_| Ok(()),
        );
        assert!(matches!(result, Err(ExplorerError::InvalidReference)));
    }

    #[test]
    fn preview_access_revalidates_synced_tokens_without_opening_file_content() {
        let (temp, filesystem, _) = fixture();
        let synced_path = temp.path().join("preview-synced-root");
        let folder_path = synced_path.join("folder");
        let file_path = synced_path.join("online-only.txt");
        fs::create_dir_all(&folder_path).expect("synced directory fixture");
        File::create(&file_path).expect("synced file fixture");
        let location = filesystem
            .replace_synced_folders(vec![SyncedFolderRoot {
                id: "synced:preview-test".to_owned(),
                name: "Cloud Storage".to_owned(),
                path: synced_path,
                detail: "Cloud Storage · Synced folder".to_owned(),
                metadata: SyncedFolderMetadataDto {
                    provider: crate::filesystem::SyncedFolderProvider::Other,
                    status: crate::filesystem::SyncedFolderStatus::Available,
                    source: crate::filesystem::SyncedFolderSource::System,
                },
                availability: SyncedAvailabilityPolicy::Unknown,
            }])
            .expect("synced snapshot")
            .remove(0);
        let mut entries = Vec::new();

        filesystem
            .list_directory(
                &location.root.id,
                &location.id,
                &AtomicBool::new(false),
                |event| {
                    if let DirectoryListingEvent::Entries { entries: batch, .. } = event {
                        entries.extend(batch);
                    }
                    Ok(())
                },
            )
            .expect("synced listing");

        let file = entries
            .iter()
            .find(|entry| entry.name == "online-only.txt")
            .expect("file entry");
        let directory = entries
            .iter()
            .find(|entry| entry.name == "folder")
            .expect("directory entry");
        assert_eq!(file.availability, ContentAvailability::Unknown);
        assert_eq!(directory.availability, ContentAvailability::Local);

        let file_access = filesystem
            .resolve_preview_access(&file.reference.id, &location.id)
            .expect("file preview access");
        assert_eq!(file_access.path, file_path);
        assert_eq!(file_access.availability, ContentAvailability::Unknown);
        assert_eq!(file_access.content_request_policy, None);
        assert_eq!(file_access.size.as_deref(), Some("0"));

        let directory_access = filesystem
            .resolve_preview_access(&directory.reference.id, &location.id)
            .expect("directory preview access");
        assert_eq!(directory_access.path, folder_path);
        assert_eq!(directory_access.availability, ContentAvailability::Local);

        assert!(matches!(
            filesystem.resolve_preview_access("forged-token", &location.id),
            Err(ExplorerError::InvalidReference)
        ));
    }

    #[test]
    fn local_mirror_policy_allows_bounded_content_access() {
        let (temp, filesystem, _) = fixture();
        let synced_path = temp.path().join("local-mirror");
        fs::create_dir(&synced_path).expect("local mirror");
        let file_path = synced_path.join("available.txt");
        File::create(&file_path).expect("local file");
        let location = filesystem
            .replace_synced_folders(vec![SyncedFolderRoot {
                id: "synced:manual:test".to_owned(),
                name: "Synced Folder 1".to_owned(),
                path: synced_path,
                detail: "Manually added · Synced folder".to_owned(),
                metadata: SyncedFolderMetadataDto {
                    provider: crate::filesystem::SyncedFolderProvider::Other,
                    status: crate::filesystem::SyncedFolderStatus::Available,
                    source: crate::filesystem::SyncedFolderSource::Manual,
                },
                availability: SyncedAvailabilityPolicy::LocalMirror,
            }])
            .expect("synced snapshot")
            .remove(0);
        let mut file = None;

        filesystem
            .list_directory(
                &location.root.id,
                &location.id,
                &AtomicBool::new(false),
                |event| {
                    if let DirectoryListingEvent::Entries { entries, .. } = event {
                        file = entries
                            .into_iter()
                            .find(|entry| entry.name == "available.txt");
                    }
                    Ok(())
                },
            )
            .expect("local mirror listing");
        let file = file.expect("listed local file");
        assert_eq!(file.availability, ContentAvailability::Local);
        assert_eq!(
            filesystem
                .resolve_preview_access(&file.reference.id, &location.id)
                .expect("preview access")
                .availability,
            ContentAvailability::Local
        );
        assert_eq!(
            filesystem
                .resolve_preview_access(&file.reference.id, &location.id)
                .expect("preview access")
                .content_request_policy,
            None
        );
    }

    #[test]
    fn icloud_policy_exposes_only_an_explicit_content_request() {
        let (temp, filesystem, _) = fixture();
        let synced_path = temp.path().join("icloud-root");
        fs::create_dir(&synced_path).expect("iCloud fixture root");
        File::create(synced_path.join("placeholder.txt")).expect("placeholder fixture");
        let location = filesystem
            .replace_synced_folders(vec![SyncedFolderRoot {
                id: "synced:icloud:test".to_owned(),
                name: "iCloud Drive".to_owned(),
                path: synced_path,
                detail: "iCloud Drive · Synced folder".to_owned(),
                metadata: SyncedFolderMetadataDto {
                    provider: crate::filesystem::SyncedFolderProvider::ICloud,
                    status: crate::filesystem::SyncedFolderStatus::Available,
                    source: crate::filesystem::SyncedFolderSource::System,
                },
                availability: SyncedAvailabilityPolicy::ICloud,
            }])
            .expect("synced snapshot")
            .remove(0);
        let mut file = None;

        filesystem
            .list_directory(
                &location.root.id,
                &location.id,
                &AtomicBool::new(false),
                |event| {
                    if let DirectoryListingEvent::Entries { entries, .. } = event {
                        file = entries
                            .into_iter()
                            .find(|entry| entry.name == "placeholder.txt");
                    }
                    Ok(())
                },
            )
            .expect("iCloud listing");
        let file = file.expect("listed placeholder");
        let access = filesystem
            .resolve_preview_access(&file.reference.id, &location.id)
            .expect("preview access");

        assert_eq!(access.availability, ContentAvailability::Unknown);
        assert_eq!(
            access.content_request_policy,
            Some(ContentRequestPolicy::ICloud)
        );
    }

    #[test]
    fn offline_manual_roots_remain_listed_but_cannot_be_traversed() {
        let (temp, filesystem, _) = fixture();
        let missing_path = temp.path().join("temporarily-unavailable");
        let location = filesystem
            .replace_synced_folders(vec![SyncedFolderRoot {
                id: "synced:manual:offline".to_owned(),
                name: "Synced Folder 1".to_owned(),
                path: missing_path,
                detail: "Manually added · Folder unavailable".to_owned(),
                metadata: SyncedFolderMetadataDto {
                    provider: crate::filesystem::SyncedFolderProvider::Other,
                    status: crate::filesystem::SyncedFolderStatus::Offline,
                    source: crate::filesystem::SyncedFolderSource::Manual,
                },
                availability: SyncedAvailabilityPolicy::LocalMirror,
            }])
            .expect("offline snapshot")
            .remove(0);

        assert_eq!(location.status, "offline");
        assert!(matches!(
            filesystem.list_directory(
                &location.root.id,
                &location.id,
                &AtomicBool::new(false),
                |_| Ok(())
            ),
            Err(ExplorerError::Offline(_))
        ));
    }

    #[test]
    fn maps_filesystem_errors_to_stable_ipc_codes() {
        let cases = [
            (std::io::ErrorKind::NotFound, ExplorerErrorCode::NotFound),
            (
                std::io::ErrorKind::PermissionDenied,
                ExplorerErrorCode::PermissionDenied,
            ),
            (
                std::io::ErrorKind::NotADirectory,
                ExplorerErrorCode::NotDirectory,
            ),
        ];

        for (kind, expected) in cases {
            let error = ExplorerError::Io {
                message: "fixture error".to_owned(),
                kind,
            };
            assert_eq!(ExplorerErrorDto::from(error).code, expected);
        }
    }

    #[test]
    fn serializes_listing_events_with_the_frontend_wire_shape() {
        let value = serde_json::to_value(DirectoryListingEvent::Complete { skipped_entries: 2 })
            .expect("serializable event");

        assert_eq!(value["event"], "complete");
        assert_eq!(value["skippedEntries"], 2);
        assert!(value.get("skipped_entries").is_none());
    }

    #[test]
    fn omits_duplicate_or_missing_roots() {
        let temp = TempDir::new().expect("temporary directory");
        let filesystem = LocalFilesystem::new(vec![
            LocalRoot {
                id: "home",
                name: "Home",
                role: LocationRole::Home,
                path: temp.path().to_path_buf(),
            },
            LocalRoot {
                id: "desktop",
                name: "Desktop",
                role: LocationRole::Desktop,
                path: temp.path().to_path_buf(),
            },
            LocalRoot {
                id: "missing",
                name: "Missing",
                role: LocationRole::Downloads,
                path: temp.path().join("missing"),
            },
        ])
        .expect("local filesystem");

        let locations = filesystem.locations().expect("locations");
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].id, "home");
        assert_eq!(locations[0].role, LocationRole::Home);
        let wire = serde_json::to_value(&locations[0]).expect("serializable location");
        assert_eq!(wire["role"], "home");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn preserves_non_utf8_paths_behind_tokens() {
        use std::os::unix::ffi::OsStringExt;

        let temp = TempDir::new().expect("temporary directory");
        let name = std::ffi::OsString::from_vec(vec![b'f', b'o', 0x80]);
        fs::create_dir(temp.path().join(name)).expect("non-utf8 fixture");
        let filesystem = LocalFilesystem::new(vec![LocalRoot {
            id: "home",
            name: "Home",
            role: LocationRole::Home,
            path: temp.path().to_path_buf(),
        }])
        .expect("local filesystem");
        let root = filesystem.locations().expect("locations")[0].root.clone();
        let mut navigable = None;

        filesystem
            .list_directory(
                &root.id,
                &root.location_id,
                &AtomicBool::new(false),
                |event| {
                    if let DirectoryListingEvent::Entries { entries, .. } = event {
                        navigable = entries.into_iter().next().and_then(|entry| entry.directory);
                    }
                    Ok(())
                },
            )
            .expect("root listing");

        let directory = navigable.expect("navigable directory");
        filesystem
            .list_directory(
                &directory.id,
                &directory.location_id,
                &AtomicBool::new(false),
                |_| Ok(()),
            )
            .expect("non-utf8 directory remains navigable");
    }

    #[cfg(unix)]
    #[test]
    fn exposes_directory_symlinks_without_recursing_implicitly() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temporary directory");
        fs::create_dir(temp.path().join("target")).expect("target directory");
        symlink(temp.path().join("target"), temp.path().join("link")).expect("symlink");
        let filesystem = LocalFilesystem::new(vec![LocalRoot {
            id: "home",
            name: "Home",
            role: LocationRole::Home,
            path: temp.path().to_path_buf(),
        }])
        .expect("local filesystem");
        let root = filesystem.locations().expect("locations")[0].root.clone();
        let mut link = None;

        filesystem
            .list_directory(
                &root.id,
                &root.location_id,
                &AtomicBool::new(false),
                |event| {
                    if let DirectoryListingEvent::Entries { entries, .. } = event {
                        link = entries.into_iter().find(|entry| entry.name == "link");
                    }
                    Ok(())
                },
            )
            .expect("root listing");

        let link = link.expect("link entry");
        assert_eq!(link.kind, "symlink");
        assert!(link.directory.is_some());
    }
}
