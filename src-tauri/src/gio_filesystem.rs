#![cfg_attr(all(test, not(target_os = "linux")), allow(dead_code))]

use std::sync::{atomic::AtomicBool, Arc, RwLock};

#[cfg(any(target_os = "linux", test))]
use std::collections::{BTreeMap, HashMap, HashSet};

#[cfg(any(target_os = "linux", test))]
use uuid::Uuid;

use crate::filesystem::{
    DirectoryListingEvent, ExplorerError, LocationSummaryDto, PreviewUnavailableReason,
};

#[cfg(any(target_os = "linux", test))]
use crate::filesystem::{
    BreadcrumbSegmentDto, DirectoryRefDto, LocationBackend, LocationRole, RevocationTracker,
    SyncedFolderMetadataDto, SyncedFolderProvider, SyncedFolderSource, SyncedFolderStatus,
};

#[cfg(any(target_os = "linux", test))]
const GIO_SYNCED_FOLDER_NAMESPACE: Uuid = Uuid::from_u128(0x5369ad81_e91f_44e4_8187_58d55eac53b0);

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, PartialEq, Eq)]
struct GioRootCandidate {
    identity: Vec<u8>,
    uri: String,
    provider: SyncedFolderProvider,
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone)]
struct GioReference {
    id: String,
    location_id: String,
    uri: String,
    name: String,
    display_path: String,
    parent_id: Option<String>,
    kind: GioEntryKind,
    size: Option<u64>,
    modified_at: Option<u64>,
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum GioEntryKind {
    Directory,
    File,
    Symlink,
    Other,
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Copy)]
struct GioEntryMetadata {
    kind: GioEntryKind,
    size: Option<u64>,
    modified_at: Option<u64>,
}

#[cfg(any(target_os = "linux", test))]
struct NewGioReference {
    uri: String,
    name: String,
    display_path: String,
    parent_id: Option<String>,
    metadata: GioEntryMetadata,
}

pub(crate) struct GioPreviewAccess {
    pub name: String,
    pub uri: String,
    pub size: Option<u64>,
    pub modified_at: Option<u64>,
    pub reason: Option<PreviewUnavailableReason>,
}

#[cfg(any(target_os = "linux", test))]
impl GioReference {
    fn directory(&self) -> DirectoryRefDto {
        DirectoryRefDto {
            id: self.id.clone(),
            location_id: self.location_id.clone(),
            name: self.name.clone(),
            display_path: self.display_path.clone(),
        }
    }
}

#[derive(Default)]
struct GioState {
    locations: Vec<LocationSummaryDto>,
    #[cfg(any(target_os = "linux", test))]
    root_keys: HashMap<String, (String, String)>,
    #[cfg(any(target_os = "linux", test))]
    references: HashMap<String, GioReference>,
    #[cfg(any(target_os = "linux", test))]
    ids_by_uri: HashMap<(String, String), String>,
    #[cfg(any(target_os = "linux", test))]
    revoked_locations: RevocationTracker,
    #[cfg(any(target_os = "linux", test))]
    revoked_references: RevocationTracker,
}

#[derive(Default)]
pub struct GioFilesystem {
    state: RwLock<GioState>,
}

impl GioFilesystem {
    pub fn start() -> Result<Arc<Self>, ExplorerError> {
        let filesystem = Arc::new(Self::default());
        #[cfg(target_os = "linux")]
        platform::install_mount_monitor(&filesystem)?;
        Ok(filesystem)
    }

    pub fn locations(&self) -> Result<Vec<LocationSummaryDto>, ExplorerError> {
        self.state
            .read()
            .map(|state| state.locations.clone())
            .map_err(|_| ExplorerError::StateUnavailable)
    }

    pub fn contains_location(&self, location_id: &str) -> Result<bool, ExplorerError> {
        self.state
            .read()
            .map(|state| {
                state
                    .locations
                    .iter()
                    .any(|location| location.id == location_id)
            })
            .map_err(|_| ExplorerError::StateUnavailable)
    }

    pub fn is_unavailable_location(&self, location_id: &str) -> Result<bool, ExplorerError> {
        #[cfg(any(target_os = "linux", test))]
        {
            self.state
                .read()
                .map(|state| state.revoked_locations.contains(location_id))
                .map_err(|_| ExplorerError::StateUnavailable)
        }

        #[cfg(not(any(target_os = "linux", test)))]
        {
            let _ = location_id;
            Ok(false)
        }
    }

    pub fn list_directory<F>(
        &self,
        directory_id: &str,
        location_id: &str,
        cancelled: &AtomicBool,
        emit: F,
    ) -> Result<(), ExplorerError>
    where
        F: FnMut(DirectoryListingEvent) -> Result<(), ExplorerError>,
    {
        #[cfg(target_os = "linux")]
        {
            platform::list_directory(self, directory_id, location_id, cancelled, emit)
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = (directory_id, location_id, cancelled, emit);
            Err(ExplorerError::Unsupported(
                "GIO locations are available only on supported Linux desktops.".to_owned(),
            ))
        }
    }

    pub(crate) fn resolve_preview_access(
        &self,
        entry_id: &str,
        location_id: &str,
    ) -> Result<GioPreviewAccess, ExplorerError> {
        #[cfg(any(target_os = "linux", test))]
        {
            let reference = self.resolve_reference(location_id, entry_id)?;
            let reason = match reference.kind {
                GioEntryKind::Directory => Some(PreviewUnavailableReason::Directory),
                GioEntryKind::Symlink => Some(PreviewUnavailableReason::Symlink),
                GioEntryKind::Other => Some(PreviewUnavailableReason::Unsupported),
                GioEntryKind::File => None,
            };
            Ok(GioPreviewAccess {
                name: reference.name,
                uri: reference.uri,
                size: reference.size,
                modified_at: reference.modified_at,
                reason,
            })
        }

        #[cfg(not(any(target_os = "linux", test)))]
        {
            let _ = (entry_id, location_id);
            Err(ExplorerError::Unsupported(
                "GIO locations are available only on supported Linux desktops.".to_owned(),
            ))
        }
    }

    pub(crate) fn materialize_preview(
        uri: String,
        output: &mut std::fs::File,
        read_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<(), ExplorerError> {
        #[cfg(target_os = "linux")]
        {
            platform::materialize_preview(uri, output, read_limit, cancelled)
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = (uri, output, read_limit, cancelled);
            Err(ExplorerError::Unsupported(
                "GIO locations are available only on supported Linux desktops.".to_owned(),
            ))
        }
    }

    #[cfg(any(target_os = "linux", test))]
    fn replace_roots(&self, candidates: Vec<GioRootCandidate>) -> Result<(), ExplorerError> {
        let normalized = normalize_roots(candidates);
        let mut state = self
            .state
            .write()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let retained_ids = normalized
            .iter()
            .map(|root| root.location_id.as_str())
            .collect::<HashSet<_>>();
        let removed_ids = state
            .root_keys
            .keys()
            .filter(|id| !retained_ids.contains(id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        for id in removed_ids {
            remove_location(&mut state, &id);
        }

        let previous_locations = state
            .locations
            .iter()
            .map(|location| (location.id.clone(), location.clone()))
            .collect::<HashMap<_, _>>();
        let mut locations = Vec::with_capacity(normalized.len());
        for root in normalized {
            let key = (root.uri.clone(), root.name.clone());
            if state.root_keys.get(&root.location_id) == Some(&key) {
                if let Some(location) = previous_locations.get(&root.location_id) {
                    state.revoked_locations.forget(&root.location_id);
                    locations.push(location.clone());
                    continue;
                }
            }

            remove_location(&mut state, &root.location_id);
            let reference = register_reference(
                &mut state,
                &root.location_id,
                NewGioReference {
                    uri: root.uri.clone(),
                    name: root.name.clone(),
                    display_path: root.name.clone(),
                    parent_id: None,
                    metadata: GioEntryMetadata {
                        kind: GioEntryKind::Directory,
                        size: None,
                        modified_at: None,
                    },
                },
            )?;
            state.root_keys.insert(root.location_id.clone(), key);
            state.revoked_locations.forget(&root.location_id);
            locations.push(LocationSummaryDto {
                id: root.location_id,
                name: root.name.clone(),
                backend: LocationBackend::Gio,
                kind: "syncedFolder",
                role: LocationRole::SyncedFolder,
                status: "available",
                display_path: root.name.clone(),
                detail: format!("{} · GIO", root.provider.display_name()),
                root: reference.directory(),
                synced_folder: Some(SyncedFolderMetadataDto {
                    provider: root.provider,
                    status: SyncedFolderStatus::Available,
                    source: SyncedFolderSource::System,
                }),
            });
        }
        state.locations = locations;
        Ok(())
    }

    #[cfg(any(target_os = "linux", test))]
    fn resolve_reference(
        &self,
        location_id: &str,
        reference_id: &str,
    ) -> Result<GioReference, ExplorerError> {
        let state = self
            .state
            .read()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        if !location_is_active(&state, location_id) {
            return Err(missing_location_error(&state, location_id));
        }
        match state.references.get(reference_id) {
            Some(reference) if reference.location_id == location_id => Ok(reference.clone()),
            Some(_) => Err(ExplorerError::InvalidReference),
            None if state
                .revoked_references
                .contains(&reference_revocation_key(location_id, reference_id)) =>
            {
                Err(ExplorerError::StaleReference)
            }
            None => Err(ExplorerError::InvalidReference),
        }
    }

    #[cfg(any(target_os = "linux", test))]
    fn root_reference(&self, location_id: &str) -> Result<GioReference, ExplorerError> {
        let state = self
            .state
            .read()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let root_id = state
            .locations
            .iter()
            .find(|location| location.id == location_id)
            .map(|location| location.root.id.as_str())
            .ok_or_else(|| missing_location_error(&state, location_id))?;
        state
            .references
            .get(root_id)
            .cloned()
            .ok_or(ExplorerError::InvalidReference)
    }

    #[cfg(any(target_os = "linux", test))]
    fn navigation_context(
        &self,
        location_id: &str,
        directory_id: &str,
    ) -> Result<
        (
            DirectoryRefDto,
            Option<DirectoryRefDto>,
            Vec<BreadcrumbSegmentDto>,
        ),
        ExplorerError,
    > {
        let state = self
            .state
            .read()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        if !location_is_active(&state, location_id) {
            return Err(missing_location_error(&state, location_id));
        }
        let current = state
            .references
            .get(directory_id)
            .filter(|reference| reference.location_id == location_id)
            .ok_or_else(|| missing_reference_error(&state, location_id, directory_id))?;
        let parent = current
            .parent_id
            .as_ref()
            .and_then(|id| state.references.get(id))
            .map(GioReference::directory);

        let mut chain = Vec::new();
        let mut cursor = Some(current);
        while let Some(reference) = cursor {
            chain.push(BreadcrumbSegmentDto {
                label: reference.name.clone(),
                directory: reference.directory(),
            });
            cursor = reference
                .parent_id
                .as_ref()
                .and_then(|id| state.references.get(id));
        }
        chain.reverse();
        Ok((current.directory(), parent, chain))
    }

    #[cfg(any(target_os = "linux", test))]
    fn register_child(
        &self,
        location_id: &str,
        parent_id: &str,
        uri: String,
        name: String,
        metadata: GioEntryMetadata,
    ) -> Result<GioReference, ExplorerError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        if !location_is_active(&state, location_id) {
            return Err(missing_location_error(&state, location_id));
        }
        let parent = state
            .references
            .get(parent_id)
            .filter(|reference| reference.location_id == location_id)
            .cloned()
            .ok_or_else(|| missing_reference_error(&state, location_id, parent_id))?;
        let display_path = format!("{}/{}", parent.display_path.trim_end_matches('/'), name);
        register_reference(
            &mut state,
            location_id,
            NewGioReference {
                uri,
                name,
                display_path,
                parent_id: Some(parent_id.to_owned()),
                metadata,
            },
        )
    }
}

#[cfg(any(target_os = "linux", test))]
struct NormalizedGioRoot {
    location_id: String,
    uri: String,
    name: String,
    provider: SyncedFolderProvider,
}

#[cfg(any(target_os = "linux", test))]
fn normalize_roots(mut candidates: Vec<GioRootCandidate>) -> Vec<NormalizedGioRoot> {
    candidates.sort_by(|left, right| {
        left.provider
            .cmp(&right.provider)
            .then_with(|| left.identity.cmp(&right.identity))
    });
    let mut seen_identity = HashSet::new();
    let mut seen_uri = HashSet::new();
    candidates.retain(|candidate| {
        seen_identity.insert(candidate.identity.clone()) && seen_uri.insert(candidate.uri.clone())
    });
    let counts = candidates
        .iter()
        .fold(BTreeMap::new(), |mut counts, candidate| {
            *counts.entry(candidate.provider).or_insert(0_usize) += 1;
            counts
        });
    let mut indexes = BTreeMap::<SyncedFolderProvider, usize>::new();
    candidates
        .into_iter()
        .map(|candidate| {
            let index = indexes.entry(candidate.provider).or_insert(0);
            *index += 1;
            let base = candidate.provider.display_name();
            let name = if counts.get(&candidate.provider).copied().unwrap_or(0) > 1 {
                format!("{base} {}", *index)
            } else {
                base.to_owned()
            };
            NormalizedGioRoot {
                location_id: format!(
                    "synced:gio:{}",
                    Uuid::new_v5(&GIO_SYNCED_FOLDER_NAMESPACE, &candidate.identity)
                ),
                uri: candidate.uri,
                name,
                provider: candidate.provider,
            }
        })
        .collect()
}

#[cfg(any(target_os = "linux", test))]
fn register_reference(
    state: &mut GioState,
    location_id: &str,
    new_reference: NewGioReference,
) -> Result<GioReference, ExplorerError> {
    let key = (location_id.to_owned(), new_reference.uri.clone());
    if let Some(existing_id) = state.ids_by_uri.get(&key) {
        return state
            .references
            .get(existing_id)
            .cloned()
            .ok_or(ExplorerError::StateUnavailable);
    }
    let reference = GioReference {
        id: Uuid::new_v4().to_string(),
        location_id: location_id.to_owned(),
        uri: new_reference.uri,
        name: new_reference.name,
        display_path: new_reference.display_path,
        parent_id: new_reference.parent_id,
        kind: new_reference.metadata.kind,
        size: new_reference.metadata.size,
        modified_at: new_reference.metadata.modified_at,
    };
    state.ids_by_uri.insert(key, reference.id.clone());
    state
        .references
        .insert(reference.id.clone(), reference.clone());
    Ok(reference)
}

#[cfg(any(target_os = "linux", test))]
fn location_is_active(state: &GioState, location_id: &str) -> bool {
    state
        .locations
        .iter()
        .any(|location| location.id == location_id)
}

#[cfg(any(target_os = "linux", test))]
fn missing_location_error(state: &GioState, location_id: &str) -> ExplorerError {
    if state.revoked_locations.contains(location_id) {
        ExplorerError::Unavailable("This location is no longer available.".to_owned())
    } else {
        ExplorerError::InvalidReference
    }
}

#[cfg(any(target_os = "linux", test))]
fn missing_reference_error(
    state: &GioState,
    location_id: &str,
    reference_id: &str,
) -> ExplorerError {
    if state
        .revoked_references
        .contains(&reference_revocation_key(location_id, reference_id))
    {
        ExplorerError::StaleReference
    } else {
        ExplorerError::InvalidReference
    }
}

#[cfg(any(target_os = "linux", test))]
fn reference_revocation_key(location_id: &str, reference_id: &str) -> String {
    format!("{location_id}\0{reference_id}")
}

#[cfg(any(target_os = "linux", test))]
fn remove_location(state: &mut GioState, location_id: &str) {
    state.root_keys.remove(location_id);
    let removed = state
        .references
        .iter()
        .filter_map(|(id, reference)| {
            (reference.location_id == location_id).then_some((id.clone(), reference.uri.clone()))
        })
        .collect::<Vec<_>>();
    for (id, uri) in removed {
        state.references.remove(&id);
        state.ids_by_uri.remove(&(location_id.to_owned(), uri));
        state
            .revoked_references
            .record(reference_revocation_key(location_id, &id));
    }
    state.revoked_locations.record(location_id.to_owned());
}

#[cfg(target_os = "linux")]
fn content_kind(name: &str, is_directory: bool) -> &'static str {
    if is_directory {
        return "folder";
    }
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    match extension.as_deref() {
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

#[cfg(target_os = "linux")]
mod platform {
    use std::{
        io::Write,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        thread,
        time::Duration,
    };

    use gio::{
        prelude::{
            CancellableExt, FileEnumeratorExt, FileExt, InputStreamExt, MountExt, VolumeMonitorExt,
        },
        Cancellable, FileQueryInfoFlags, FileType, IOErrorEnum, VolumeMonitor,
    };

    use crate::filesystem::{
        ContentAvailability, EntryRefDto, FileEntrySummaryDto, LISTING_BATCH_SIZE,
    };

    use super::*;

    const LISTING_ATTRIBUTES: &str = "standard::name,standard::display-name,standard::type,standard::is-symlink,standard::size,time::modified";

    pub(super) fn install_mount_monitor(
        filesystem: &Arc<GioFilesystem>,
    ) -> Result<(), ExplorerError> {
        let monitor = VolumeMonitor::get();
        refresh_mounts(filesystem, &monitor)?;

        let weak = Arc::downgrade(filesystem);
        monitor.connect_mount_added(move |monitor, _| {
            if let Some(filesystem) = weak.upgrade() {
                let _ = refresh_mounts(&filesystem, monitor);
            }
        });
        let weak = Arc::downgrade(filesystem);
        monitor.connect_mount_changed(move |monitor, _| {
            if let Some(filesystem) = weak.upgrade() {
                let _ = refresh_mounts(&filesystem, monitor);
            }
        });
        let weak = Arc::downgrade(filesystem);
        monitor.connect_mount_removed(move |monitor, _| {
            if let Some(filesystem) = weak.upgrade() {
                let _ = refresh_mounts(&filesystem, monitor);
            }
        });
        Ok(())
    }

    fn refresh_mounts(
        filesystem: &GioFilesystem,
        monitor: &VolumeMonitor,
    ) -> Result<(), ExplorerError> {
        let candidates = monitor
            .mounts()
            .into_iter()
            .filter_map(|mount| {
                let root = mount.root();
                (root.uri_scheme().as_deref() == Some("google-drive")).then(|| {
                    let uri = root.uri().to_string();
                    GioRootCandidate {
                        identity: uri.as_bytes().to_vec(),
                        uri,
                        provider: SyncedFolderProvider::GoogleDrive,
                    }
                })
            })
            .collect();
        filesystem.replace_roots(candidates)
    }

    pub(super) fn list_directory<F>(
        filesystem: &GioFilesystem,
        directory_id: &str,
        location_id: &str,
        cancelled: &AtomicBool,
        mut emit: F,
    ) -> Result<(), ExplorerError>
    where
        F: FnMut(DirectoryListingEvent) -> Result<(), ExplorerError>,
    {
        ensure_not_cancelled(cancelled)?;
        let reference = filesystem.resolve_reference(location_id, directory_id)?;
        let root = filesystem.root_reference(location_id)?;
        let file = gio::File::for_uri(&reference.uri);
        let root_file = gio::File::for_uri(&root.uri);
        if !file.equal(&root_file) && !file.has_prefix(&root_file) {
            return Err(ExplorerError::InvalidReference);
        }
        let (directory, parent, breadcrumbs) =
            filesystem.navigation_context(location_id, directory_id)?;

        with_cancellable(cancelled, |cancellable| {
            let enumerator = file
                .enumerate_children(
                    LISTING_ATTRIBUTES,
                    FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                    Some(cancellable),
                )
                .map_err(map_gio_error)?;
            // Opening a provider namespace can block. Do not publish a Started
            // event after the request has been cancelled or timed out.
            ensure_not_cancelled(cancelled)?;
            emit(DirectoryListingEvent::Started {
                directory,
                parent,
                breadcrumbs,
            })?;
            let mut batch = Vec::with_capacity(LISTING_BATCH_SIZE);
            let mut replace = true;
            loop {
                ensure_not_cancelled(cancelled)?;
                let Some(info) = enumerator
                    .next_file(Some(cancellable))
                    .map_err(map_gio_error)?
                else {
                    break;
                };
                let child = enumerator.child(&info);
                let name = info.display_name().to_string();
                let file_type = info.file_type();
                let is_directory = file_type == FileType::Directory;
                let is_symlink = info.is_symlink() || file_type == FileType::SymbolicLink;
                let kind = if is_directory {
                    GioEntryKind::Directory
                } else if is_symlink {
                    GioEntryKind::Symlink
                } else if file_type == FileType::Regular {
                    GioEntryKind::File
                } else {
                    GioEntryKind::Other
                };
                let numeric_size = (file_type == FileType::Regular && info.size() >= 0)
                    .then(|| info.size() as u64);
                let modified_at = info
                    .has_attribute("time::modified")
                    .then(|| info.attribute_uint64("time::modified"))
                    .and_then(|seconds| seconds.checked_mul(1_000));
                let registered = filesystem.register_child(
                    location_id,
                    directory_id,
                    child.uri().to_string(),
                    name.clone(),
                    GioEntryMetadata {
                        kind,
                        size: numeric_size,
                        modified_at,
                    },
                )?;
                let kind = if is_directory {
                    "directory"
                } else if is_symlink {
                    "symlink"
                } else if file_type == FileType::Regular {
                    "file"
                } else {
                    "other"
                };
                batch.push(FileEntrySummaryDto {
                    reference: EntryRefDto {
                        id: registered.id.clone(),
                        location_id: location_id.to_owned(),
                    },
                    name: name.clone(),
                    kind,
                    content_kind: content_kind(&name, is_directory),
                    size: numeric_size.map(|size| size.to_string()),
                    modified_at,
                    display_path: registered.display_path.clone(),
                    directory: is_directory.then(|| registered.directory()),
                    availability: if file_type == FileType::Regular {
                        ContentAvailability::Unknown
                    } else {
                        ContentAvailability::Local
                    },
                    detail: is_symlink.then_some("Symbolic link"),
                });
                if batch.len() == LISTING_BATCH_SIZE {
                    emit(DirectoryListingEvent::Entries {
                        entries: std::mem::take(&mut batch),
                        replace,
                    })?;
                    replace = false;
                }
            }
            if !batch.is_empty() {
                emit(DirectoryListingEvent::Entries {
                    entries: batch,
                    replace,
                })?;
            }
            ensure_not_cancelled(cancelled)?;
            emit(DirectoryListingEvent::Complete { skipped_entries: 0 })
        })
    }

    pub(super) fn materialize_preview(
        uri: String,
        output: &mut std::fs::File,
        read_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<(), ExplorerError> {
        ensure_not_cancelled(cancelled)?;
        let file = gio::File::for_uri(&uri);
        with_cancellable(cancelled, |cancellable| {
            let stream = file.read(Some(cancellable)).map_err(map_gio_error)?;
            let mut remaining = read_limit;
            while remaining > 0 {
                ensure_not_cancelled(cancelled)?;
                let chunk = stream
                    .read_bytes(remaining.min(64 * 1024), Some(cancellable))
                    .map_err(map_gio_error)?;
                if chunk.is_empty() {
                    break;
                }
                output
                    .write_all(chunk.as_ref())
                    .map_err(|error| ExplorerError::Io {
                        message: "Explora could not write the temporary cloud preview.".to_owned(),
                        kind: error.kind(),
                    })?;
                remaining = remaining.saturating_sub(chunk.len());
            }
            ensure_not_cancelled(cancelled)
        })
    }

    fn with_cancellable<T>(
        cancelled: &AtomicBool,
        operation: impl FnOnce(&Cancellable) -> Result<T, ExplorerError>,
    ) -> Result<T, ExplorerError> {
        let cancellable = Cancellable::new();
        let finished = Arc::new(AtomicBool::new(false));
        thread::scope(|scope| {
            let watcher_finished = finished.clone();
            let watcher_cancellable = cancellable.clone();
            scope.spawn(move || {
                while !watcher_finished.load(Ordering::Relaxed) {
                    if cancelled.load(Ordering::Relaxed) {
                        watcher_cancellable.cancel();
                        break;
                    }
                    thread::sleep(Duration::from_millis(20));
                }
            });
            let result = operation(&cancellable);
            finished.store(true, Ordering::Relaxed);
            result
        })
    }

    fn ensure_not_cancelled(cancelled: &AtomicBool) -> Result<(), ExplorerError> {
        if cancelled.load(Ordering::Relaxed) {
            Err(ExplorerError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn map_gio_error(error: gio::glib::Error) -> ExplorerError {
        match error.kind::<IOErrorEnum>() {
            Some(IOErrorEnum::Cancelled) => ExplorerError::Cancelled,
            Some(IOErrorEnum::NotFound) => ExplorerError::Io {
                message: "This cloud entry is no longer available.".to_owned(),
                kind: std::io::ErrorKind::NotFound,
            },
            Some(IOErrorEnum::PermissionDenied) => ExplorerError::Io {
                message: "Explora does not have permission to read this cloud entry.".to_owned(),
                kind: std::io::ErrorKind::PermissionDenied,
            },
            Some(IOErrorEnum::NotDirectory) => ExplorerError::Io {
                message: "This cloud entry is not a folder.".to_owned(),
                kind: std::io::ErrorKind::NotADirectory,
            },
            Some(
                IOErrorEnum::NotMounted
                | IOErrorEnum::HostNotFound
                | IOErrorEnum::HostUnreachable
                | IOErrorEnum::NetworkUnreachable
                | IOErrorEnum::ConnectionRefused
                | IOErrorEnum::NotConnected,
            ) => ExplorerError::Offline("This cloud location is currently offline.".to_owned()),
            Some(IOErrorEnum::TimedOut) => {
                ExplorerError::TimedOut("The cloud filesystem operation took too long.".to_owned())
            }
            Some(IOErrorEnum::NotSupported) => ExplorerError::Unsupported(
                "This cloud filesystem operation is not supported.".to_owned(),
            ),
            _ => ExplorerError::Unexpected(
                "The cloud filesystem could not complete this operation.".to_owned(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(identity: &str, uri: &str) -> GioRootCandidate {
        GioRootCandidate {
            identity: identity.as_bytes().to_vec(),
            uri: uri.to_owned(),
            provider: SyncedFolderProvider::GoogleDrive,
        }
    }

    #[test]
    fn normalizes_private_mounts_to_stable_opaque_locations() {
        let roots = normalize_roots(vec![
            candidate("private-a", "google-drive://person@example.com/"),
            candidate("private-b", "google-drive://work@example.com/"),
        ]);

        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].name, "Google Drive 1");
        assert_eq!(roots[1].name, "Google Drive 2");
        assert!(roots
            .iter()
            .all(|root| root.location_id.starts_with("synced:gio:")));
        assert!(roots
            .iter()
            .all(|root| !root.location_id.contains("example.com")));
    }

    #[test]
    fn replacement_preserves_references_until_the_mount_is_removed() {
        let filesystem = GioFilesystem::default();
        let root = candidate("private-a", "google-drive://person@example.com/");
        filesystem
            .replace_roots(vec![root.clone()])
            .expect("initial roots");
        let location = filesystem.locations().expect("locations")[0].clone();
        let child = filesystem
            .register_child(
                &location.id,
                &location.root.id,
                "google-drive://person@example.com/folder".to_owned(),
                "Folder".to_owned(),
                GioEntryMetadata {
                    kind: GioEntryKind::Directory,
                    size: None,
                    modified_at: None,
                },
            )
            .expect("child reference");

        filesystem
            .replace_roots(vec![root.clone()])
            .expect("same roots");
        assert_eq!(
            filesystem
                .resolve_reference(&location.id, &child.id)
                .expect("preserved child")
                .display_path,
            "Google Drive/Folder"
        );

        filesystem.replace_roots(Vec::new()).expect("remove roots");
        assert!(matches!(
            filesystem.resolve_reference(&location.id, &child.id),
            Err(ExplorerError::Unavailable(_))
        ));

        filesystem.replace_roots(vec![root]).expect("restore root");
        assert!(matches!(
            filesystem.resolve_reference(&location.id, &child.id),
            Err(ExplorerError::StaleReference)
        ));
        assert!(matches!(
            filesystem.resolve_reference(&location.id, "never-valid"),
            Err(ExplorerError::InvalidReference)
        ));
    }

    #[test]
    fn serialized_locations_never_expose_mount_uris_or_account_labels() {
        let filesystem = GioFilesystem::default();
        filesystem
            .replace_roots(vec![candidate(
                "private-a",
                "google-drive://person@example.com/",
            )])
            .expect("roots");

        let json = serde_json::to_string(&filesystem.locations().expect("locations"))
            .expect("serialize locations");
        assert!(!json.contains("google-drive://"));
        assert!(!json.contains("person@example.com"));
        assert!(json.contains("Google Drive"));
        assert!(json.contains("\"backend\":\"gio\""));
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires a real GNOME Online Accounts Google Drive mount"]
    fn native_linux_google_drive_mounts_register_and_open_without_uris_crossing_ipc() {
        use std::{sync::mpsc, time::Duration};

        let filesystem = GioFilesystem::start()
            .unwrap_or_else(|_| panic!("native Linux GIO synced-folder discovery failed"));
        let locations = filesystem
            .locations()
            .unwrap_or_else(|_| panic!("native Linux GIO synced-folder snapshot failed"));
        assert!(
            !locations.is_empty(),
            "native smoke requires at least one Google Drive GIO mount"
        );
        let serialized =
            serde_json::to_string(&locations).expect("native GIO locations should serialize");
        assert!(!serialized.contains("google-drive://"));
        assert!(!serialized.contains('@'));

        let expected_root_count = locations.len();
        let mut opened_roots = 0_usize;
        for location in locations {
            let filesystem = Arc::clone(&filesystem);
            let (sender, receiver) = mpsc::sync_channel(1);
            std::thread::spawn(move || {
                let mut opened = false;
                let result = filesystem.list_directory(
                    &location.root.id,
                    &location.id,
                    &AtomicBool::new(false),
                    |event| {
                        if matches!(event, DirectoryListingEvent::Started { .. }) {
                            opened = true;
                            // Opening the provider namespace is enough for this
                            // smoke. Enumerating private user entries belongs in
                            // a controlled fixture or packaged-app scenario.
                            return Err(ExplorerError::Cancelled);
                        }
                        Ok(())
                    },
                );
                let _ = sender.send(opened && matches!(result, Err(ExplorerError::Cancelled)));
            });

            if receiver
                .recv_timeout(Duration::from_secs(5))
                .unwrap_or(false)
            {
                opened_roots += 1;
            }
        }

        eprintln!(
            "native Linux GIO synced-folder smoke: discovered={expected_root_count}, opened={opened_roots}, stalled_or_failed={}",
            expected_root_count - opened_roots
        );
        assert!(
            opened_roots > 0,
            "no discovered Google Drive GIO root opened through the GIO backend"
        );
    }
}
