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

use crate::filesystem::{
    BreadcrumbSegmentDto, DirectoryListingEvent, DirectoryRefDto, EntryRefDto, ExplorerError,
    FileEntrySummaryDto, LocationRole, LocationSummaryDto, LISTING_BATCH_SIZE,
};

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeRoot {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub detail: String,
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
                kind: "local",
                role: root.role,
                status: "available",
                display_path: directory.display_path.clone(),
                detail: "Local".to_owned(),
                root: directory,
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
        })
    }

    pub fn locations(&self) -> Result<Vec<LocationSummaryDto>, ExplorerError> {
        self.locations
            .read()
            .map(|locations| locations.clone())
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
                kind: "volume",
                role: LocationRole::Volume,
                status: "available",
                display_path: directory.display_path.clone(),
                detail: volume.detail,
                root: directory,
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

    pub(crate) fn resolve_preview_path(
        &self,
        entry_id: &str,
        location_id: &str,
    ) -> Result<PathBuf, ExplorerError> {
        let location_exists = self
            .locations
            .read()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .iter()
            .any(|location| location.id == location_id);
        if !location_exists {
            return Err(ExplorerError::InvalidReference);
        }
        self.registry.resolve(location_id, entry_id)
    }

    pub(crate) fn resolve_native_open_path(
        &self,
        entry_id: &str,
        location_id: &str,
    ) -> Result<PathBuf, ExplorerError> {
        let path = self.resolve_preview_path(entry_id, location_id)?;
        let link_metadata = fs::symlink_metadata(&path)
            .map_err(|error| ExplorerError::io("inspect", path.as_path(), error))?;
        let target_metadata = if link_metadata.file_type().is_symlink() {
            Some(
                fs::metadata(&path)
                    .map_err(|error| ExplorerError::io("open", path.as_path(), error))?,
            )
        } else {
            None
        };
        if native_open_capability(&path, &link_metadata, target_metadata.as_ref()) != "direct" {
            return Err(ExplorerError::Unsupported(
                "This item cannot be opened with a native application.".to_owned(),
            ));
        }
        Ok(path)
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
        let location = self
            .locations
            .read()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .iter()
            .find(|location| location.id == location_id)
            .cloned()
            .ok_or(ExplorerError::InvalidReference)?;
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

            match self.describe_entry(entry, location_id) {
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
    ) -> Result<FileEntrySummaryDto, ExplorerError> {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let file_type = entry
            .file_type()
            .map_err(|error| ExplorerError::io("inspect", path.as_path(), error))?;
        let metadata = fs::symlink_metadata(&path).ok();
        let target_metadata = file_type
            .is_symlink()
            .then(|| fs::metadata(&path).ok())
            .flatten();
        let symlink_target_is_directory = target_metadata
            .as_ref()
            .is_some_and(std::fs::Metadata::is_dir);
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
            detail: file_type.is_symlink().then_some("Symbolic link"),
            native_open: metadata
                .as_ref()
                .map(|metadata| native_open_capability(&path, metadata, target_metadata.as_ref()))
                .unwrap_or("none"),
        })
    }
}

fn native_open_capability(
    path: &Path,
    metadata: &fs::Metadata,
    target_metadata: Option<&fs::Metadata>,
) -> &'static str {
    let target_is_file = if metadata.file_type().is_symlink() {
        target_metadata.is_some_and(fs::Metadata::is_file)
    } else {
        metadata.is_file()
    };
    if target_is_file {
        return "direct";
    }

    let target_is_directory = if metadata.file_type().is_symlink() {
        target_metadata.is_some_and(fs::Metadata::is_dir)
    } else {
        metadata.is_dir()
    };
    if target_is_directory && is_native_application_bundle(path) {
        "direct"
    } else {
        "none"
    }
}

#[cfg(target_os = "macos")]
fn is_native_application_bundle(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
}

#[cfg(not(target_os = "macos"))]
fn is_native_application_bundle(_path: &Path) -> bool {
    false
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
        assert_eq!(folder.native_open, "none");
        assert_eq!(file.size.as_deref(), Some("5"));
        assert_eq!(file.content_kind, "document");
        assert_eq!(file.native_open, "direct");
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
        assert_eq!(link.native_open, "none");
    }

    #[cfg(unix)]
    #[test]
    fn opens_file_symlinks_through_their_opaque_reference() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temporary directory");
        fs::write(temp.path().join("target.txt"), b"hello").expect("target file");
        symlink(temp.path().join("target.txt"), temp.path().join("link.txt")).expect("symlink");
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
                        link = entries.into_iter().find(|entry| entry.name == "link.txt");
                    }
                    Ok(())
                },
            )
            .expect("listing");
        let link = link.expect("file symlink");
        assert_eq!(link.native_open, "direct");
        assert_eq!(
            filesystem
                .resolve_native_open_path(&link.reference.id, &link.reference.location_id)
                .expect("resolved open path"),
            temp.path().join("link.txt")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_application_bundles_are_openable_instead_of_only_navigable() {
        let temp = TempDir::new().expect("temporary directory");
        fs::create_dir(temp.path().join("Example.app")).expect("application bundle");
        let filesystem = LocalFilesystem::new(vec![LocalRoot {
            id: "home",
            name: "Home",
            role: LocationRole::Home,
            path: temp.path().to_path_buf(),
        }])
        .expect("local filesystem");
        let root = filesystem.locations().expect("locations")[0].root.clone();
        let mut application = None;
        filesystem
            .list_directory(
                &root.id,
                &root.location_id,
                &AtomicBool::new(false),
                |event| {
                    if let DirectoryListingEvent::Entries { entries, .. } = event {
                        application = entries
                            .into_iter()
                            .find(|entry| entry.name == "Example.app");
                    }
                    Ok(())
                },
            )
            .expect("listing");
        let application = application.expect("application entry");
        assert_eq!(application.kind, "directory");
        assert!(application.directory.is_some());
        assert_eq!(application.native_open, "direct");
        assert_eq!(
            filesystem
                .resolve_native_open_path(
                    &application.reference.id,
                    &application.reference.location_id,
                )
                .expect("resolved application bundle"),
            temp.path().join("Example.app")
        );
    }
}
