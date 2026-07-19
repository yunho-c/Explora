use std::{
    collections::HashMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::UNIX_EPOCH,
};

use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

const LISTING_BATCH_SIZE: usize = 256;

#[derive(Debug, Clone)]
pub struct LocalRoot {
    pub id: &'static str,
    pub name: &'static str,
    pub role: LocationRole,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LocationRole {
    Home,
    Desktop,
    Documents,
    Downloads,
    Pictures,
    Music,
    Videos,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EntryRefDto {
    pub id: String,
    pub location_id: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryRefDto {
    pub id: String,
    pub location_id: String,
    pub name: String,
    pub display_path: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BreadcrumbSegmentDto {
    pub label: String,
    pub directory: DirectoryRefDto,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocationSummaryDto {
    pub id: String,
    pub name: String,
    pub kind: &'static str,
    pub role: LocationRole,
    pub status: &'static str,
    pub display_path: String,
    pub detail: &'static str,
    pub root: DirectoryRefDto,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileEntrySummaryDto {
    pub reference: EntryRefDto,
    pub name: String,
    pub kind: &'static str,
    pub content_kind: &'static str,
    pub size: Option<String>,
    pub modified_at: Option<u64>,
    pub display_path: String,
    pub directory: Option<DirectoryRefDto>,
    pub detail: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(
    tag = "event",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DirectoryListingEvent {
    Started {
        directory: DirectoryRefDto,
        parent: Option<DirectoryRefDto>,
        breadcrumbs: Vec<BreadcrumbSegmentDto>,
    },
    Entries {
        entries: Vec<FileEntrySummaryDto>,
        replace: bool,
    },
    Complete {
        skipped_entries: usize,
    },
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ExplorerErrorCode {
    InvalidReference,
    NotFound,
    PermissionDenied,
    NotDirectory,
    Cancelled,
    Unexpected,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExplorerErrorDto {
    pub code: ExplorerErrorCode,
    pub message: String,
}

#[derive(Debug, Error)]
pub enum ExplorerError {
    #[error("This filesystem reference is no longer valid.")]
    InvalidReference,
    #[error("The directory listing was cancelled.")]
    Cancelled,
    #[error("Explora's filesystem state is unavailable.")]
    StateUnavailable,
    #[error("{message}")]
    Io {
        message: String,
        kind: std::io::ErrorKind,
    },
    #[error("The directory listing channel closed unexpectedly.")]
    ChannelClosed,
}

impl ExplorerError {
    fn io(action: &str, path: &Path, error: std::io::Error) -> Self {
        Self::Io {
            message: format!("Explora could not {action} {}: {error}", path.display()),
            kind: error.kind(),
        }
    }
}

impl From<ExplorerError> for ExplorerErrorDto {
    fn from(error: ExplorerError) -> Self {
        let code = match &error {
            ExplorerError::InvalidReference => ExplorerErrorCode::InvalidReference,
            ExplorerError::Cancelled => ExplorerErrorCode::Cancelled,
            ExplorerError::Io { kind, .. } => match kind {
                std::io::ErrorKind::NotFound => ExplorerErrorCode::NotFound,
                std::io::ErrorKind::PermissionDenied => ExplorerErrorCode::PermissionDenied,
                std::io::ErrorKind::NotADirectory => ExplorerErrorCode::NotDirectory,
                _ => ExplorerErrorCode::Unexpected,
            },
            ExplorerError::StateUnavailable | ExplorerError::ChannelClosed => {
                ExplorerErrorCode::Unexpected
            }
        };

        Self {
            code,
            message: error.to_string(),
        }
    }
}

#[derive(Default)]
struct PathRegistryInner {
    paths_by_id: HashMap<String, PathBuf>,
    ids_by_path: HashMap<PathBuf, String>,
}

#[derive(Default)]
struct PathRegistry {
    inner: Mutex<PathRegistryInner>,
}

impl PathRegistry {
    fn register(&self, path: PathBuf) -> Result<String, ExplorerError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;

        if let Some(id) = inner.ids_by_path.get(&path) {
            return Ok(id.clone());
        }

        let id = Uuid::new_v4().to_string();
        inner.paths_by_id.insert(id.clone(), path.clone());
        inner.ids_by_path.insert(path, id.clone());
        Ok(id)
    }

    fn resolve(&self, id: &str) -> Result<PathBuf, ExplorerError> {
        self.inner
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .paths_by_id
            .get(id)
            .cloned()
            .ok_or(ExplorerError::InvalidReference)
    }
}

pub struct LocalFilesystem {
    registry: PathRegistry,
    locations: Vec<LocationSummaryDto>,
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
                detail: "Local",
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
            locations,
        })
    }

    pub fn locations(&self) -> Vec<LocationSummaryDto> {
        self.locations.clone()
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
        if !self
            .locations
            .iter()
            .any(|location| location.id == location_id)
        {
            return Err(ExplorerError::InvalidReference);
        }
        let path = self.registry.resolve(directory_id)?;
        let read_dir = fs::read_dir(&path)
            .map_err(|error| ExplorerError::io("open", path.as_path(), error))?;

        let directory = directory_ref(&self.registry, &path, location_id, None)?;
        let parent = path
            .parent()
            .map(|parent| directory_ref(&self.registry, parent, location_id, None))
            .transpose()?;
        let breadcrumbs = breadcrumbs(&self.registry, &path, location_id)?;

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
        let symlink_target_is_directory = file_type.is_symlink()
            && fs::metadata(&path)
                .map(|target| target.is_dir())
                .unwrap_or(false);
        let is_navigable = file_type.is_dir() || symlink_target_is_directory;
        let id = self.registry.register(path.clone())?;
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
        })
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
        id: registry.register(path.to_path_buf())?,
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
    location_id: &str,
) -> Result<Vec<BreadcrumbSegmentDto>, ExplorerError> {
    path.ancestors()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|ancestor| {
            let directory = directory_ref(registry, ancestor, location_id, None)?;
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
        let root = filesystem.locations()[0].root.clone();
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
        assert!(started.1.is_some());
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
        let root = filesystem.locations()[0].root.clone();
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

        assert_eq!(filesystem.locations().len(), 1);
        assert_eq!(filesystem.locations()[0].id, "home");
        assert_eq!(filesystem.locations()[0].role, LocationRole::Home);
        let wire = serde_json::to_value(&filesystem.locations()[0]).expect("serializable location");
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
        let root = filesystem.locations()[0].root.clone();
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
        let root = filesystem.locations()[0].root.clone();
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
