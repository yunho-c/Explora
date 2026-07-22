use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
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
    BreadcrumbSegmentDto, DirectoryCapabilitiesDto, DirectoryListingEvent, DirectoryRefDto,
    EntryCapabilitiesDto, EntryRefDto, ExplorerError, FileEntrySummaryDto, LocationRole,
    LocationSummaryDto, LISTING_BATCH_SIZE,
};
use crate::local_relocate::relocate_no_replace;
use crate::platform_trash::PlatformTrash;
use crate::transfer::{
    copy_local_file_into_owned_partial, verify_local_file_copy, OwnedLocalTransferArtifact,
};

const MAX_PERMANENT_DELETE_ENTRIES: usize = 1_000_000;
const MAX_KEEP_BOTH_ATTEMPTS: u32 = 10_000;
const MAX_LOCAL_NAME_UNITS: usize = 255;

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
    identities_by_id: HashMap<String, Option<FileIdentity>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    volume: u64,
    file: u64,
}

#[derive(Default)]
struct PathRegistry {
    inner: Mutex<PathRegistryInner>,
}

impl PathRegistry {
    fn register(&self, location_id: &str, path: PathBuf) -> Result<String, ExplorerError> {
        let identity = fs::symlink_metadata(&path)
            .ok()
            .and_then(|metadata| metadata_identity(&metadata));
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;

        let key = (location_id.to_owned(), path.clone());
        if let Some(id) = inner.ids_by_path.get(&key).cloned() {
            inner.identities_by_id.insert(id.clone(), identity);
            return Ok(id);
        }

        let id = Uuid::new_v4().to_string();
        inner.paths_by_id.insert(id.clone(), path.clone());
        inner
            .locations_by_id
            .insert(id.clone(), location_id.to_owned());
        inner.ids_by_path.insert(key, id.clone());
        inner.identities_by_id.insert(id.clone(), identity);
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

    fn resolve_for_operation(&self, location_id: &str, id: &str) -> Result<PathBuf, ExplorerError> {
        let (path, expected_identity) = {
            let inner = self
                .inner
                .lock()
                .map_err(|_| ExplorerError::StateUnavailable)?;
            if inner.locations_by_id.get(id).map(String::as_str) != Some(location_id) {
                return Err(ExplorerError::InvalidReference);
            }
            (
                inner
                    .paths_by_id
                    .get(id)
                    .cloned()
                    .ok_or(ExplorerError::InvalidReference)?,
                inner.identities_by_id.get(id).copied().flatten(),
            )
        };
        let current_metadata = fs::symlink_metadata(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ExplorerError::SourceChanged
            } else {
                ExplorerError::io("inspect", &path, error)
            }
        })?;
        let current_identity = metadata_identity(&current_metadata);
        if expected_identity.is_some() && current_identity != expected_identity {
            return Err(ExplorerError::SourceChanged);
        }
        Ok(path)
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
            inner.identities_by_id.remove(&id);
        }
        Ok(())
    }

    fn rebase_subtree(
        &self,
        location_id: &str,
        old_path: &Path,
        new_path: &Path,
    ) -> Result<Vec<String>, ExplorerError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let rebased = inner
            .paths_by_id
            .iter()
            .filter(|(id, path)| {
                inner.locations_by_id.get(*id).map(String::as_str) == Some(location_id)
                    && path.starts_with(old_path)
            })
            .map(|(id, path)| {
                path.strip_prefix(old_path)
                    .map(|suffix| (id.clone(), path.clone(), new_path.join(suffix)))
                    .map_err(|_| ExplorerError::StateUnavailable)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let rebased_ids = rebased
            .iter()
            .map(|(id, _, _)| id.clone())
            .collect::<Vec<_>>();
        let stale_destination_ids = inner
            .ids_by_path
            .iter()
            .filter_map(|((registered_location, path), id)| {
                (registered_location == location_id
                    && path.starts_with(new_path)
                    && !rebased_ids.contains(id))
                .then_some(id.clone())
            })
            .collect::<Vec<_>>();
        for id in stale_destination_ids {
            if let Some(path) = inner.paths_by_id.remove(&id) {
                inner.ids_by_path.remove(&(location_id.to_owned(), path));
            }
            inner.locations_by_id.remove(&id);
            inner.identities_by_id.remove(&id);
        }

        for (id, old_registered_path, new_registered_path) in rebased {
            inner
                .ids_by_path
                .remove(&(location_id.to_owned(), old_registered_path));
            inner
                .paths_by_id
                .insert(id.clone(), new_registered_path.clone());
            inner
                .ids_by_path
                .insert((location_id.to_owned(), new_registered_path), id);
        }
        Ok(rebased_ids)
    }

    fn invalidate_subtree(
        &self,
        location_id: &str,
        removed_path: &Path,
    ) -> Result<Vec<String>, ExplorerError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let removed_ids = inner
            .paths_by_id
            .iter()
            .filter_map(|(id, path)| {
                (inner.locations_by_id.get(id).map(String::as_str) == Some(location_id)
                    && path.starts_with(removed_path))
                .then_some(id.clone())
            })
            .collect::<Vec<_>>();

        for id in &removed_ids {
            if let Some(path) = inner.paths_by_id.remove(id) {
                inner.ids_by_path.remove(&(location_id.to_owned(), path));
            }
            inner.locations_by_id.remove(id);
            inner.identities_by_id.remove(id);
        }
        Ok(removed_ids)
    }
}

pub struct LocalFilesystem {
    registry: PathRegistry,
    locations: RwLock<Vec<LocationSummaryDto>>,
    trash_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedLocalEntry {
    pub reference: EntryRefDto,
    pub name: String,
    pub invalidated_entry_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalMoveConflictPolicy {
    Fail,
    KeepBoth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovedLocalEntry {
    pub entry: FileEntrySummaryDto,
    pub source_parent: DirectoryRefDto,
    pub destination: DirectoryRefDto,
    pub rebased_entry_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferredLocalEntry {
    pub entry: FileEntrySummaryDto,
    pub source_parent: DirectoryRefDto,
    pub destination: DirectoryRefDto,
    pub invalidated_entry_ids: Vec<String>,
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
        Self::new_with_trash_support(
            roots,
            cfg!(any(
                target_os = "macos",
                target_os = "linux",
                target_os = "windows"
            )),
        )
    }

    pub(crate) fn new_with_trash_support(
        roots: Vec<LocalRoot>,
        trash_available: bool,
    ) -> Result<Self, ExplorerError> {
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
            trash_available,
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

    pub fn rename_entry(
        &self,
        entry: &EntryRefDto,
        new_name: &str,
        cancelled: &AtomicBool,
    ) -> Result<FileEntrySummaryDto, ExplorerError> {
        ensure_not_cancelled(cancelled)?;
        validate_entry_name(new_name)?;

        let (source_path, _) = self.resolve_mutation_source(entry)?;
        let parent = source_path
            .parent()
            .ok_or(ExplorerError::InvalidReference)?;
        if source_path.file_name() == Some(OsStr::new(new_name)) {
            return self.describe_path(source_path, &entry.location_id);
        }

        let source_metadata = fs::symlink_metadata(&source_path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ExplorerError::SourceChanged
            } else {
                ExplorerError::io("inspect", &source_path, error)
            }
        })?;
        let destination_path = parent.join(new_name);
        match fs::symlink_metadata(&destination_path) {
            Ok(destination_metadata) => {
                if !same_entry(&source_metadata, &destination_metadata) {
                    return Err(ExplorerError::Conflict);
                }
                rename_case_only(&source_path, &destination_path)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ensure_not_cancelled(cancelled)?;
                relocate_no_replace(&source_path, &destination_path).map_err(|error| {
                    if error.kind() == std::io::ErrorKind::AlreadyExists {
                        ExplorerError::Conflict
                    } else {
                        ExplorerError::io("rename", &source_path, error)
                    }
                })?;
            }
            Err(error) => return Err(ExplorerError::io("inspect", &destination_path, error)),
        }

        if self
            .registry
            .rebase_subtree(&entry.location_id, &source_path, &destination_path)
            .is_err()
        {
            // A successful filesystem mutation must never leave authoritative
            // tokens pointing at their old paths.
            let _ = self.registry.remove_location(&entry.location_id);
            return Err(ExplorerError::StateUnavailable);
        }
        self.describe_path(destination_path, &entry.location_id)
    }

    pub fn move_entry(
        &self,
        entry: &EntryRefDto,
        destination: &DirectoryRefDto,
        conflict_policy: LocalMoveConflictPolicy,
        cancelled: &AtomicBool,
    ) -> Result<MovedLocalEntry, ExplorerError> {
        ensure_not_cancelled(cancelled)?;
        let (source_path, _) = self.resolve_mutation_source(entry)?;
        let destination_path = self.resolve_move_destination(entry, destination, &source_path)?;
        let source_parent_path = source_path
            .parent()
            .ok_or(ExplorerError::InvalidReference)?
            .to_path_buf();
        let source_parent = directory_ref(
            &self.registry,
            &source_parent_path,
            &entry.location_id,
            None,
        )?;

        if source_parent_path == destination_path {
            return Ok(MovedLocalEntry {
                entry: self.describe_path(source_path, &entry.location_id)?,
                source_parent,
                destination: directory_ref(
                    &self.registry,
                    &destination_path,
                    &entry.location_id,
                    None,
                )?,
                rebased_entry_ids: vec![entry.id.clone()],
            });
        }

        let original_name = source_path
            .file_name()
            .ok_or(ExplorerError::InvalidReference)?;
        let source_is_directory = fs::symlink_metadata(&source_path)
            .map_err(|error| ExplorerError::io("inspect", &source_path, error))?
            .file_type()
            .is_dir();
        let preferred_destination = destination_path.join(original_name);
        ensure_not_cancelled(cancelled)?;
        let final_destination = match fs::symlink_metadata(&preferred_destination) {
            Ok(_) if conflict_policy == LocalMoveConflictPolicy::Fail => {
                return Err(ExplorerError::Conflict)
            }
            Ok(_) => relocate_keep_both(
                &source_path,
                &destination_path,
                original_name,
                source_is_directory,
                cancelled,
            )?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match relocate_no_replace(&source_path, &preferred_destination) {
                    Ok(()) => preferred_destination,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::AlreadyExists
                            && conflict_policy == LocalMoveConflictPolicy::KeepBoth =>
                    {
                        relocate_keep_both(
                            &source_path,
                            &destination_path,
                            original_name,
                            source_is_directory,
                            cancelled,
                        )?
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        return Err(ExplorerError::Conflict)
                    }
                    Err(error) => {
                        return Err(ExplorerError::io("move", &source_path, error));
                    }
                }
            }
            Err(error) => {
                return Err(ExplorerError::io("inspect", &preferred_destination, error));
            }
        };

        let rebased_entry_ids =
            match self
                .registry
                .rebase_subtree(&entry.location_id, &source_path, &final_destination)
            {
                Ok(ids) => ids,
                Err(_) => {
                    let _ = self.registry.remove_location(&entry.location_id);
                    return Err(ExplorerError::StateUnavailable);
                }
            };
        Ok(MovedLocalEntry {
            entry: self.describe_path(final_destination, &entry.location_id)?,
            source_parent,
            destination: directory_ref(
                &self.registry,
                &destination_path,
                &entry.location_id,
                None,
            )?,
            rebased_entry_ids,
        })
    }

    pub fn transfer_file_to_local_location<F>(
        &self,
        entry: &EntryRefDto,
        destination: &DirectoryRefDto,
        conflict_policy: LocalMoveConflictPolicy,
        cancelled: &AtomicBool,
        mut on_progress: F,
    ) -> Result<TransferredLocalEntry, ExplorerError>
    where
        F: FnMut(u64, u64) -> Result<(), ExplorerError>,
    {
        ensure_not_cancelled(cancelled)?;
        if destination.location_id == entry.location_id {
            return Err(ExplorerError::InvalidConfiguration(
                "A same-location move must use atomic relocation.".to_owned(),
            ));
        }
        let (source_path, _) = self.resolve_mutation_source(entry)?;
        let source_metadata = fs::symlink_metadata(&source_path)
            .map_err(|error| ExplorerError::io("inspect", &source_path, error))?;
        if !source_metadata.file_type().is_file() {
            return Err(ExplorerError::Unsupported(
                "Cross-location directory and symlink moves are not available yet.".to_owned(),
            ));
        }
        let destination_path = self.resolve_transfer_destination(destination)?;
        let original_name = source_path
            .file_name()
            .ok_or(ExplorerError::InvalidReference)?;
        let source_parent_path = source_path
            .parent()
            .ok_or(ExplorerError::InvalidReference)?
            .to_path_buf();
        let source_parent = directory_ref(
            &self.registry,
            &source_parent_path,
            &entry.location_id,
            None,
        )?;
        let destination_ref = directory_ref(
            &self.registry,
            &destination_path,
            &destination.location_id,
            None,
        )?;

        let mut artifact = match conflict_policy {
            LocalMoveConflictPolicy::Fail => {
                OwnedLocalTransferArtifact::create(&destination_path, original_name)?
            }
            LocalMoveConflictPolicy::KeepBoth => {
                let mut artifact = None;
                for attempt in 1..=MAX_KEEP_BOTH_ATTEMPTS {
                    ensure_not_cancelled(cancelled)?;
                    let candidate = keep_both_name(original_name, false, attempt);
                    match OwnedLocalTransferArtifact::create(&destination_path, &candidate) {
                        Ok(created) => {
                            artifact = Some(created);
                            break;
                        }
                        Err(ExplorerError::Conflict) => continue,
                        Err(error) => return Err(error),
                    }
                }
                artifact.ok_or(ExplorerError::Conflict)?
            }
        };
        let total_bytes = source_metadata.len();
        on_progress(0, total_bytes)?;
        copy_local_file_into_owned_partial(&source_path, &mut artifact, cancelled, |completed| {
            on_progress(completed, total_bytes)
        })?;
        let finalized_path = artifact.finalize()?.to_path_buf();
        verify_local_file_copy(&source_path, &finalized_path, cancelled)?;

        // Revalidate the source token and cheap mutable metadata immediately
        // before the irreversible delete. A changed source is preserved.
        let revalidated_path = self
            .registry
            .resolve_for_operation(&entry.location_id, &entry.id)?;
        let revalidated_metadata = fs::symlink_metadata(&revalidated_path)
            .map_err(|error| ExplorerError::io("inspect", &revalidated_path, error))?;
        if revalidated_metadata.len() != source_metadata.len()
            || revalidated_metadata.modified().ok() != source_metadata.modified().ok()
        {
            return Err(ExplorerError::SourceChanged);
        }
        let final_path = artifact.preserve();
        if let Err(error) = fs::remove_file(&source_path) {
            return Err(ExplorerError::PartialCompletion(format!(
                "The verified copy was kept, but Explora could not remove the source: {}.",
                error.kind()
            )));
        }
        let invalidated_entry_ids = self
            .registry
            .invalidate_subtree(&entry.location_id, &source_path)?;
        let entry = self.describe_path(final_path, &destination.location_id)?;
        Ok(TransferredLocalEntry {
            entry,
            source_parent,
            destination: destination_ref,
            invalidated_entry_ids,
        })
    }

    pub fn describe_transfer_conflict(
        &self,
        entry: &EntryRefDto,
        destination: &DirectoryRefDto,
    ) -> Result<(String, String), ExplorerError> {
        let (_, source_name) = self.resolve_mutation_source(entry)?;
        let destination_path = self.resolve_transfer_destination(destination)?;
        Ok((source_name, directory_name(&destination_path)))
    }

    pub fn describe_move_conflict(
        &self,
        entry: &EntryRefDto,
        destination: &DirectoryRefDto,
    ) -> Result<(String, String), ExplorerError> {
        let (source_path, source_name) = self.resolve_mutation_source(entry)?;
        let destination_path = self.resolve_move_destination(entry, destination, &source_path)?;
        Ok((source_name, directory_name(&destination_path)))
    }

    pub fn trash_entry(
        &self,
        entry: &EntryRefDto,
        cancelled: &AtomicBool,
        platform_trash: &dyn PlatformTrash,
    ) -> Result<RemovedLocalEntry, ExplorerError> {
        ensure_not_cancelled(cancelled)?;
        if !self.trash_available || !platform_trash.is_available() {
            return Err(ExplorerError::Unsupported(
                "The operating system Trash is not available for this item.".to_owned(),
            ));
        }
        let (source_path, name) = self.resolve_mutation_source(entry)?;
        ensure_not_cancelled(cancelled)?;
        platform_trash.move_to_trash(&source_path)?;
        let invalidated_entry_ids = self
            .registry
            .invalidate_subtree(&entry.location_id, &source_path)?;
        Ok(RemovedLocalEntry {
            reference: entry.clone(),
            name,
            invalidated_entry_ids,
        })
    }

    pub fn permanently_delete_entry(
        &self,
        entry: &EntryRefDto,
        cancelled: &AtomicBool,
    ) -> Result<RemovedLocalEntry, ExplorerError> {
        ensure_not_cancelled(cancelled)?;
        let (source_path, name) = self.resolve_mutation_source(entry)?;
        let metadata = fs::symlink_metadata(&source_path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ExplorerError::SourceChanged
            } else {
                ExplorerError::io("inspect", &source_path, error)
            }
        })?;
        ensure_not_cancelled(cancelled)?;
        if metadata.file_type().is_dir() {
            let removal_plan = plan_directory_removal(&source_path, cancelled)?;
            // Once the first entry is removed, cancellation would produce a
            // misleading terminal state and a partially deleted tree. The plan
            // is cancellable; execution is an explicit irreversible section.
            for planned in removal_plan {
                match planned.kind {
                    PlannedRemovalKind::FileOrSymlink => fs::remove_file(&planned.path),
                    PlannedRemovalKind::Directory => fs::remove_dir(&planned.path),
                }
                .map_err(|error| ExplorerError::io("delete", &planned.path, error))?;
            }
        } else {
            // symlink_metadata keeps symlink targets out of this branch.
            fs::remove_file(&source_path)
                .map_err(|error| ExplorerError::io("delete", &source_path, error))?;
        }
        let invalidated_entry_ids = self
            .registry
            .invalidate_subtree(&entry.location_id, &source_path)?;
        Ok(RemovedLocalEntry {
            reference: entry.clone(),
            name,
            invalidated_entry_ids,
        })
    }

    pub fn describe_operation_target(
        &self,
        entry: &EntryRefDto,
    ) -> Result<(String, String), ExplorerError> {
        let (_, name) = self.resolve_mutation_source(entry)?;
        let location_name = self
            .locations
            .read()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .iter()
            .find(|location| location.id == entry.location_id)
            .map(|location| location.name.clone())
            .ok_or(ExplorerError::InvalidReference)?;
        Ok((name, location_name))
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
            directory: Box::new(directory),
            parent: parent.map(Box::new),
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
        self.describe_path(entry.path(), location_id)
    }

    fn describe_path(
        &self,
        path: PathBuf,
        location_id: &str,
    ) -> Result<FileEntrySummaryDto, ExplorerError> {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or(ExplorerError::InvalidReference)?;
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| ExplorerError::io("inspect", path.as_path(), error))?;
        let file_type = metadata.file_type();
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
            capabilities: if file_type.is_dir() {
                DirectoryCapabilitiesDto::LOCAL
            } else {
                // Directory symlinks remain navigable, but are not accepted as
                // mutation destinations because following them could escape the
                // authorized location root.
                DirectoryCapabilitiesDto::READ_ONLY
            },
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
        let size = file_type.is_file().then(|| metadata.len().to_string());
        let modified_at = metadata
            .modified()
            .ok()
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
            capabilities: EntryCapabilitiesDto::local(self.trash_available),
        })
    }

    fn resolve_mutation_source(
        &self,
        entry: &EntryRefDto,
    ) -> Result<(PathBuf, String), ExplorerError> {
        let location = self
            .locations
            .read()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .iter()
            .find(|location| location.id == entry.location_id)
            .cloned()
            .ok_or(ExplorerError::InvalidReference)?;
        let root_path = self
            .registry
            .resolve(&entry.location_id, &location.root.id)?;
        let source_path = self
            .registry
            .resolve_for_operation(&entry.location_id, &entry.id)?;
        if source_path == root_path || !source_path.starts_with(&root_path) {
            return Err(ExplorerError::InvalidReference);
        }
        let name = source_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or(ExplorerError::InvalidReference)?;
        Ok((source_path, name))
    }

    fn resolve_move_destination(
        &self,
        entry: &EntryRefDto,
        destination: &DirectoryRefDto,
        source_path: &Path,
    ) -> Result<PathBuf, ExplorerError> {
        if destination.location_id != entry.location_id {
            return Err(ExplorerError::Unsupported(
                "Moving between locations requires a transfer, which is not available yet."
                    .to_owned(),
            ));
        }
        let location = self
            .locations
            .read()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .iter()
            .find(|location| location.id == destination.location_id)
            .cloned()
            .ok_or(ExplorerError::InvalidReference)?;
        let root_path = self
            .registry
            .resolve(&destination.location_id, &location.root.id)?;
        let destination_path = self
            .registry
            .resolve_for_operation(&destination.location_id, &destination.id)?;
        if !destination_path.starts_with(&root_path) {
            return Err(ExplorerError::InvalidReference);
        }

        let destination_metadata = fs::symlink_metadata(&destination_path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ExplorerError::DestinationUnavailable(
                    "The destination folder is no longer available.".to_owned(),
                )
            } else {
                ExplorerError::io("inspect", &destination_path, error)
            }
        })?;
        if !destination_metadata.file_type().is_dir() {
            return Err(ExplorerError::DestinationUnavailable(
                "Choose a folder that can accept moved items.".to_owned(),
            ));
        }
        let source_metadata = fs::symlink_metadata(source_path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ExplorerError::SourceChanged
            } else {
                ExplorerError::io("inspect", source_path, error)
            }
        })?;
        if metadata_identity(&source_metadata).map(|identity| identity.volume)
            != metadata_identity(&destination_metadata).map(|identity| identity.volume)
        {
            return Err(ExplorerError::Unsupported(
                "Moving between filesystems requires a verified transfer, which is not available yet."
                    .to_owned(),
            ));
        }
        if source_metadata.file_type().is_dir() && destination_path.starts_with(source_path) {
            return Err(ExplorerError::DestinationUnavailable(
                "A folder cannot be moved into itself or one of its subfolders.".to_owned(),
            ));
        }
        Ok(destination_path)
    }

    fn resolve_transfer_destination(
        &self,
        destination: &DirectoryRefDto,
    ) -> Result<PathBuf, ExplorerError> {
        let location = self
            .locations
            .read()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .iter()
            .find(|location| location.id == destination.location_id)
            .cloned()
            .ok_or(ExplorerError::InvalidReference)?;
        let root_path = self
            .registry
            .resolve(&destination.location_id, &location.root.id)?;
        let destination_path = self
            .registry
            .resolve_for_operation(&destination.location_id, &destination.id)?;
        if !destination_path.starts_with(&root_path) {
            return Err(ExplorerError::InvalidReference);
        }
        let metadata = fs::symlink_metadata(&destination_path)
            .map_err(|error| ExplorerError::io("inspect", &destination_path, error))?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
            return Err(ExplorerError::DestinationUnavailable(
                "Choose a folder that can accept moved items.".to_owned(),
            ));
        }
        Ok(destination_path)
    }
}

enum PlannedRemovalKind {
    FileOrSymlink,
    Directory,
}

struct PlannedRemoval {
    path: PathBuf,
    kind: PlannedRemovalKind,
}

enum RemovalWalkItem {
    Visit(PathBuf),
    FinishDirectory(PathBuf),
}

fn plan_directory_removal(
    root: &Path,
    cancelled: &AtomicBool,
) -> Result<Vec<PlannedRemoval>, ExplorerError> {
    let mut pending = vec![RemovalWalkItem::Visit(root.to_path_buf())];
    let mut plan = Vec::new();
    while let Some(item) = pending.pop() {
        ensure_not_cancelled(cancelled)?;
        if plan.len().saturating_add(pending.len()) >= MAX_PERMANENT_DELETE_ENTRIES {
            return Err(ExplorerError::Unsupported(
                "This folder contains too many items to delete safely in one operation.".to_owned(),
            ));
        }
        match item {
            RemovalWalkItem::FinishDirectory(path) => plan.push(PlannedRemoval {
                path,
                kind: PlannedRemovalKind::Directory,
            }),
            RemovalWalkItem::Visit(path) => {
                let metadata = fs::symlink_metadata(&path)
                    .map_err(|error| ExplorerError::io("inspect", &path, error))?;
                if metadata.file_type().is_dir() {
                    pending.push(RemovalWalkItem::FinishDirectory(path.clone()));
                    let entries = fs::read_dir(&path)
                        .map_err(|error| ExplorerError::io("open", &path, error))?;
                    for entry in entries {
                        let entry = entry.map_err(|error| ExplorerError::Io {
                            message: "Explora could not enumerate an item for deletion.".to_owned(),
                            kind: error.kind(),
                        })?;
                        pending.push(RemovalWalkItem::Visit(entry.path()));
                    }
                } else {
                    plan.push(PlannedRemoval {
                        path,
                        kind: PlannedRemovalKind::FileOrSymlink,
                    });
                }
            }
        }
    }
    Ok(plan)
}

fn validate_entry_name(name: &str) -> Result<(), ExplorerError> {
    if name.is_empty() || name.len() > 255 || name == "." || name == ".." {
        return Err(ExplorerError::InvalidName(
            "Enter a file name between 1 and 255 bytes.".to_owned(),
        ));
    }
    if name.contains(['/', '\0']) {
        return Err(ExplorerError::InvalidName(
            "File names cannot contain a path separator or null character.".to_owned(),
        ));
    }

    #[cfg(target_os = "windows")]
    {
        let invalid_char = name
            .chars()
            .any(|character| matches!(character, '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*'));
        let stem = name
            .trim_end_matches(['.', ' '])
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        let stem_bytes = stem.as_bytes();
        let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem_bytes.len() == 4
                && matches!(&stem_bytes[..3], b"COM" | b"LPT")
                && stem_bytes[3].is_ascii_digit()
                && stem_bytes[3] != b'0');
        if invalid_char || name.ends_with(['.', ' ']) || reserved {
            return Err(ExplorerError::InvalidName(
                "That name is not valid on Windows.".to_owned(),
            ));
        }
    }

    Ok(())
}

#[cfg(unix)]
fn metadata_identity(metadata: &fs::Metadata) -> Option<FileIdentity> {
    use std::os::unix::fs::MetadataExt;

    Some(FileIdentity {
        volume: metadata.dev(),
        file: metadata.ino(),
    })
}

#[cfg(windows)]
fn metadata_identity(metadata: &fs::Metadata) -> Option<FileIdentity> {
    use std::os::windows::fs::MetadataExt;

    Some(FileIdentity {
        volume: u64::from(metadata.volume_serial_number()?),
        file: metadata.file_index()?,
    })
}

#[cfg(not(any(unix, windows)))]
fn metadata_identity(_metadata: &fs::Metadata) -> Option<FileIdentity> {
    None
}

fn same_entry(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    metadata_identity(left).is_some() && metadata_identity(left) == metadata_identity(right)
}

fn relocate_keep_both(
    source: &Path,
    destination_directory: &Path,
    original_name: &OsStr,
    is_directory: bool,
    cancelled: &AtomicBool,
) -> Result<PathBuf, ExplorerError> {
    for attempt in 1..=MAX_KEEP_BOTH_ATTEMPTS {
        ensure_not_cancelled(cancelled)?;
        let candidate =
            destination_directory.join(keep_both_name(original_name, is_directory, attempt));
        match relocate_no_replace(source, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(ExplorerError::io("move", source, error)),
        }
    }
    Err(ExplorerError::Conflict)
}

fn keep_both_name(original: &OsStr, is_directory: bool, attempt: u32) -> OsString {
    let suffix = if attempt == 1 {
        " copy".to_owned()
    } else {
        format!(" copy {attempt}")
    };
    let original_path = Path::new(original);
    let stem = if is_directory {
        original
    } else {
        original_path.file_stem().unwrap_or(original)
    };
    let extension = (!is_directory)
        .then(|| original_path.extension())
        .flatten()
        .filter(|extension| !extension.is_empty());
    let mut extension_segment = OsString::new();
    if let Some(extension) = extension {
        extension_segment.push(".");
        extension_segment.push(extension);
    }
    if os_name_len(&extension_segment).saturating_add(suffix.len()) >= MAX_LOCAL_NAME_UNITS {
        extension_segment.clear();
    }
    let stem_limit = MAX_LOCAL_NAME_UNITS
        .saturating_sub(suffix.len())
        .saturating_sub(os_name_len(&extension_segment));
    let mut candidate = truncate_os_name(stem, stem_limit);
    if candidate.is_empty() {
        candidate.push("item");
    }
    candidate.push(suffix);
    candidate.push(extension_segment);
    candidate
}

#[cfg(unix)]
fn os_name_len(value: &OsStr) -> usize {
    use std::os::unix::ffi::OsStrExt;

    value.as_bytes().len()
}

#[cfg(windows)]
fn os_name_len(value: &OsStr) -> usize {
    use std::os::windows::ffi::OsStrExt;

    value.encode_wide().count()
}

#[cfg(not(any(unix, windows)))]
fn os_name_len(value: &OsStr) -> usize {
    value.to_string_lossy().len()
}

#[cfg(unix)]
fn truncate_os_name(value: &OsStr, max_len: usize) -> OsString {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let bytes = value.as_bytes();
    if bytes.len() <= max_len {
        return value.to_owned();
    }
    if let Some(text) = value.to_str() {
        let mut end = max_len.min(text.len());
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        return OsString::from(&text[..end]);
    }
    OsString::from_vec(bytes[..max_len.min(bytes.len())].to_vec())
}

#[cfg(windows)]
fn truncate_os_name(value: &OsStr, max_len: usize) -> OsString {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let mut units = value.encode_wide().take(max_len).collect::<Vec<_>>();
    if units
        .last()
        .is_some_and(|unit| (0xD800..=0xDBFF).contains(unit))
    {
        units.pop();
    }
    OsString::from_wide(&units)
}

#[cfg(not(any(unix, windows)))]
fn truncate_os_name(value: &OsStr, max_len: usize) -> OsString {
    value.to_string_lossy().chars().take(max_len).collect()
}

fn rename_case_only(source: &Path, destination: &Path) -> Result<(), ExplorerError> {
    let parent = source.parent().ok_or(ExplorerError::InvalidReference)?;
    let mut intermediate = None;
    for _ in 0..16 {
        let candidate = parent.join(format!(".explora-rename-{}", Uuid::new_v4()));
        match relocate_no_replace(source, &candidate) {
            Ok(()) => {
                intermediate = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(ExplorerError::io("rename", source, error)),
        }
    }
    let intermediate = intermediate.ok_or_else(|| {
        ExplorerError::Unexpected(
            "Explora could not reserve an intermediate rename path.".to_owned(),
        )
    })?;

    if let Err(error) = relocate_no_replace(&intermediate, destination) {
        if let Err(rollback_error) = relocate_no_replace(&intermediate, source) {
            return Err(ExplorerError::Unexpected(format!(
                "The rename failed and Explora could not restore the original name: {rollback_error}"
            )));
        }
        return Err(ExplorerError::io("rename", source, error));
    }
    Ok(())
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
        capabilities: DirectoryCapabilitiesDto::LOCAL,
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
    use std::{
        fs::File,
        io::Write,
        path::{Path, PathBuf},
    };

    use tempfile::TempDir;

    use crate::filesystem::{ExplorerErrorCode, ExplorerErrorDto};

    use super::*;

    struct MoveAsideTrash {
        destination: PathBuf,
    }

    impl PlatformTrash for MoveAsideTrash {
        fn is_available(&self) -> bool {
            true
        }

        fn move_to_trash(&self, path: &Path) -> Result<(), ExplorerError> {
            let name = path.file_name().ok_or(ExplorerError::InvalidReference)?;
            fs::rename(path, self.destination.join(name))
                .map_err(|error| ExplorerError::io("trash", path, error))
        }
    }

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

    fn transfer_fixture() -> (
        TempDir,
        LocalFilesystem,
        DirectoryRefDto,
        DirectoryRefDto,
        EntryRefDto,
    ) {
        let temp = TempDir::new().expect("temporary directory");
        let source_dir = temp.path().join("source");
        let destination_dir = temp.path().join("destination");
        fs::create_dir(&source_dir).expect("source root");
        fs::create_dir(&destination_dir).expect("destination root");
        fs::write(source_dir.join("large.bin"), vec![0x2a; 300_000]).expect("source file");
        let filesystem = LocalFilesystem::new(vec![
            LocalRoot {
                id: "source-root",
                name: "Source",
                role: LocationRole::Home,
                path: source_dir,
            },
            LocalRoot {
                id: "destination-root",
                name: "Destination",
                role: LocationRole::Volume,
                path: destination_dir,
            },
        ])
        .expect("local filesystem");
        let locations = filesystem.locations().expect("locations");
        let source_root = locations
            .iter()
            .find(|location| location.id == "source-root")
            .expect("source location")
            .root
            .clone();
        let destination_root = locations
            .iter()
            .find(|location| location.id == "destination-root")
            .expect("destination location")
            .root
            .clone();
        let entry = listed_entries(&filesystem, &source_root)
            .into_iter()
            .find(|entry| entry.name == "large.bin")
            .expect("source entry")
            .reference;
        (temp, filesystem, source_root, destination_root, entry)
    }

    #[test]
    fn transfers_a_regular_file_between_local_locations_without_overwriting() {
        let (temp, filesystem, source_root, destination_root, entry) = transfer_fixture();
        let progress = Mutex::new(Vec::new());
        let moved = filesystem
            .transfer_file_to_local_location(
                &entry,
                &destination_root,
                LocalMoveConflictPolicy::Fail,
                &AtomicBool::new(false),
                |completed, total| {
                    progress.lock().expect("progress").push((completed, total));
                    Ok(())
                },
            )
            .expect("verified transfer");

        assert!(!temp.path().join("source/large.bin").exists());
        assert_eq!(
            fs::read(temp.path().join("destination/large.bin")).expect("destination bytes"),
            vec![0x2a; 300_000]
        );
        assert_eq!(
            moved.entry.reference.location_id,
            destination_root.location_id
        );
        assert!(moved.invalidated_entry_ids.contains(&entry.id));
        assert_eq!(
            progress.lock().expect("progress").last().copied(),
            Some((300_000, 300_000))
        );
        assert_eq!(moved.source_parent.id, source_root.id);
    }

    #[test]
    fn cancelled_local_transfer_cleans_partial_and_preserves_source() {
        let (temp, filesystem, _source_root, destination_root, entry) = transfer_fixture();
        let cancelled = AtomicBool::new(true);
        assert!(matches!(
            filesystem.transfer_file_to_local_location(
                &entry,
                &destination_root,
                LocalMoveConflictPolicy::Fail,
                &cancelled,
                |_, _| Ok(())
            ),
            Err(ExplorerError::Cancelled)
        ));
        assert!(temp.path().join("source/large.bin").exists());
        assert!(!temp.path().join("destination/large.bin").exists());
        assert!(fs::read_dir(temp.path().join("destination"))
            .expect("destination listing")
            .next()
            .is_none());
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
        assert!(file.capabilities.rename);
        assert!(file.capabilities.move_entry);
        assert!(file.capabilities.trash);
        assert!(file.capabilities.delete_permanently);
        assert!(!file.reference.id.contains("notes.md"));
        assert_eq!(started.0.id, root.id);
        assert!(started.1.is_none());
        assert_eq!(
            started.2.last().map(|item| &item.directory.id),
            Some(&root.id)
        );
    }

    fn listed_entries(
        filesystem: &LocalFilesystem,
        directory: &DirectoryRefDto,
    ) -> Vec<FileEntrySummaryDto> {
        let mut entries = Vec::new();
        filesystem
            .list_directory(
                &directory.id,
                &directory.location_id,
                &AtomicBool::new(false),
                |event| {
                    if let DirectoryListingEvent::Entries { entries: batch, .. } = event {
                        entries.extend(batch);
                    }
                    Ok(())
                },
            )
            .expect("directory listing");
        entries
    }

    #[test]
    fn renames_a_file_without_changing_its_opaque_identity() {
        let (temp, filesystem, root) = fixture();
        let entry = listed_entries(&filesystem, &root)
            .into_iter()
            .find(|entry| entry.name == "notes.md")
            .expect("notes entry");

        let renamed = filesystem
            .rename_entry(&entry.reference, "renamed.md", &AtomicBool::new(false))
            .expect("rename file");

        assert_eq!(renamed.reference, entry.reference);
        assert_eq!(renamed.name, "renamed.md");
        assert!(!temp.path().join("notes.md").exists());
        assert_eq!(
            fs::read(temp.path().join("renamed.md")).expect("file"),
            b"hello"
        );
    }

    #[test]
    fn rebases_registered_descendants_after_a_directory_rename() {
        let (temp, filesystem, root) = fixture();
        fs::write(temp.path().join("folder").join("child.txt"), b"child").expect("nested fixture");
        let folder = listed_entries(&filesystem, &root)
            .into_iter()
            .find(|entry| entry.name == "folder")
            .expect("folder entry");
        let folder_directory = folder.directory.clone().expect("folder directory");
        let child = listed_entries(&filesystem, &folder_directory)
            .into_iter()
            .find(|entry| entry.name == "child.txt")
            .expect("child entry");

        let renamed = filesystem
            .rename_entry(&folder.reference, "renamed-folder", &AtomicBool::new(false))
            .expect("rename folder");
        let child_path = filesystem
            .resolve_preview_path(&child.reference.id, &child.reference.location_id)
            .expect("rebased child reference");

        assert_eq!(renamed.reference, folder.reference);
        assert_eq!(child_path, temp.path().join("renamed-folder/child.txt"));
        assert_eq!(
            listed_entries(&filesystem, &folder_directory)[0].name,
            "child.txt"
        );
    }

    #[test]
    fn rename_rejects_conflicts_invalid_names_and_stale_sources() {
        let (temp, filesystem, root) = fixture();
        fs::write(temp.path().join("existing.md"), b"existing").expect("conflict fixture");
        let entry = listed_entries(&filesystem, &root)
            .into_iter()
            .find(|entry| entry.name == "notes.md")
            .expect("notes entry");

        assert!(matches!(
            filesystem.rename_entry(&entry.reference, "existing.md", &AtomicBool::new(false)),
            Err(ExplorerError::Conflict)
        ));
        assert!(matches!(
            filesystem.rename_entry(&entry.reference, "../escape", &AtomicBool::new(false)),
            Err(ExplorerError::InvalidName(_))
        ));
        fs::remove_file(temp.path().join("notes.md")).expect("remove source");
        fs::write(temp.path().join("notes.md"), b"replacement").expect("replace source");
        assert!(matches!(
            filesystem.rename_entry(&entry.reference, "new.md", &AtomicBool::new(false)),
            Err(ExplorerError::SourceChanged)
        ));
        assert_eq!(
            fs::read(temp.path().join("notes.md")).expect("replacement preserved"),
            b"replacement"
        );
        assert_eq!(
            fs::read(temp.path().join("existing.md")).expect("conflict preserved"),
            b"existing"
        );
    }

    #[test]
    fn rename_honors_cancellation_before_mutating() {
        let (temp, filesystem, root) = fixture();
        let entry = listed_entries(&filesystem, &root)
            .into_iter()
            .find(|entry| entry.name == "notes.md")
            .expect("notes entry");
        let cancelled = AtomicBool::new(true);

        assert!(matches!(
            filesystem.rename_entry(&entry.reference, "new.md", &cancelled),
            Err(ExplorerError::Cancelled)
        ));
        assert!(temp.path().join("notes.md").exists());
        assert!(!temp.path().join("new.md").exists());
    }

    #[test]
    fn moves_a_file_without_replacing_and_preserves_its_opaque_identity() {
        let (temp, filesystem, root) = fixture();
        let entries = listed_entries(&filesystem, &root);
        let source = entries
            .iter()
            .find(|entry| entry.name == "notes.md")
            .expect("source entry");
        let destination = entries
            .iter()
            .find(|entry| entry.name == "folder")
            .and_then(|entry| entry.directory.as_ref())
            .expect("destination directory");

        let moved = filesystem
            .move_entry(
                &source.reference,
                destination,
                LocalMoveConflictPolicy::Fail,
                &AtomicBool::new(false),
            )
            .expect("move file");

        assert_eq!(moved.entry.reference, source.reference);
        assert_eq!(moved.source_parent.id, root.id);
        assert_eq!(moved.destination.id, destination.id);
        assert!(moved.rebased_entry_ids.contains(&source.reference.id));
        assert!(!temp.path().join("notes.md").exists());
        assert_eq!(
            fs::read(temp.path().join("folder/notes.md")).expect("moved file"),
            b"hello"
        );
    }

    #[test]
    fn move_rebases_registered_directory_descendants() {
        let (temp, filesystem, root) = fixture();
        fs::create_dir(temp.path().join("destination")).expect("destination fixture");
        fs::write(temp.path().join("folder/child.txt"), b"child").expect("child fixture");
        let entries = listed_entries(&filesystem, &root);
        let source = entries
            .iter()
            .find(|entry| entry.name == "folder")
            .expect("source directory");
        let child = listed_entries(
            &filesystem,
            source.directory.as_ref().expect("source directory ref"),
        )
        .into_iter()
        .find(|entry| entry.name == "child.txt")
        .expect("child entry");
        let destination = listed_entries(&filesystem, &root)
            .into_iter()
            .find(|entry| entry.name == "destination")
            .and_then(|entry| entry.directory)
            .expect("destination directory");

        let moved = filesystem
            .move_entry(
                &source.reference,
                &destination,
                LocalMoveConflictPolicy::Fail,
                &AtomicBool::new(false),
            )
            .expect("move directory");

        assert!(moved.rebased_entry_ids.contains(&source.reference.id));
        assert!(moved.rebased_entry_ids.contains(&child.reference.id));
        assert_eq!(
            filesystem
                .resolve_preview_path(&child.reference.id, &child.reference.location_id)
                .expect("rebased child"),
            temp.path().join("destination/folder/child.txt")
        );
    }

    #[test]
    fn move_conflicts_preserve_both_sources_until_keep_both_is_selected() {
        let (temp, filesystem, root) = fixture();
        fs::write(temp.path().join("folder/notes.md"), b"existing").expect("conflict fixture");
        let entries = listed_entries(&filesystem, &root);
        let source = entries
            .iter()
            .find(|entry| entry.name == "notes.md")
            .expect("source entry");
        let destination = entries
            .iter()
            .find(|entry| entry.name == "folder")
            .and_then(|entry| entry.directory.as_ref())
            .expect("destination directory");

        assert!(matches!(
            filesystem.move_entry(
                &source.reference,
                destination,
                LocalMoveConflictPolicy::Fail,
                &AtomicBool::new(false),
            ),
            Err(ExplorerError::Conflict)
        ));
        assert_eq!(fs::read(temp.path().join("notes.md")).unwrap(), b"hello");
        assert_eq!(
            fs::read(temp.path().join("folder/notes.md")).unwrap(),
            b"existing"
        );

        let moved = filesystem
            .move_entry(
                &source.reference,
                destination,
                LocalMoveConflictPolicy::KeepBoth,
                &AtomicBool::new(false),
            )
            .expect("keep both move");
        assert_eq!(moved.entry.name, "notes copy.md");
        assert_eq!(
            fs::read(temp.path().join("folder/notes copy.md")).unwrap(),
            b"hello"
        );
        assert_eq!(
            fs::read(temp.path().join("folder/notes.md")).unwrap(),
            b"existing"
        );
    }

    #[test]
    fn move_rejects_a_directory_descendant_and_honors_early_cancellation() {
        let (temp, filesystem, root) = fixture();
        fs::create_dir(temp.path().join("folder/child")).expect("child directory fixture");
        let folder = listed_entries(&filesystem, &root)
            .into_iter()
            .find(|entry| entry.name == "folder")
            .expect("folder entry");
        let child = listed_entries(
            &filesystem,
            folder.directory.as_ref().expect("folder directory"),
        )
        .into_iter()
        .find(|entry| entry.name == "child")
        .and_then(|entry| entry.directory)
        .expect("child directory");

        assert!(matches!(
            filesystem.move_entry(
                &folder.reference,
                &child,
                LocalMoveConflictPolicy::Fail,
                &AtomicBool::new(false),
            ),
            Err(ExplorerError::DestinationUnavailable(_))
        ));
        let cancelled = AtomicBool::new(true);
        assert!(matches!(
            filesystem.move_entry(
                &folder.reference,
                &root,
                LocalMoveConflictPolicy::Fail,
                &cancelled,
            ),
            Err(ExplorerError::Cancelled)
        ));
        assert!(temp.path().join("folder/child").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn move_does_not_follow_a_directory_symlink_destination() {
        use std::os::unix::fs::symlink;

        let (temp, filesystem, root) = fixture();
        let external = temp.path().join("external");
        fs::create_dir(&external).expect("external directory");
        symlink(&external, temp.path().join("external-link")).expect("directory symlink");
        let source = listed_entries(&filesystem, &root)
            .into_iter()
            .find(|entry| entry.name == "notes.md")
            .expect("source entry");
        let destination = listed_entries(&filesystem, &root)
            .into_iter()
            .find(|entry| entry.name == "external-link")
            .and_then(|entry| entry.directory)
            .expect("navigable symlink");

        assert!(!destination.capabilities.accept_move);
        assert!(matches!(
            filesystem.move_entry(
                &source.reference,
                &destination,
                LocalMoveConflictPolicy::Fail,
                &AtomicBool::new(false),
            ),
            Err(ExplorerError::DestinationUnavailable(_))
        ));
        assert!(temp.path().join("notes.md").is_file());
        assert!(!external.join("notes.md").exists());
    }

    #[test]
    fn trash_moves_the_entry_and_invalidates_registered_descendants() {
        let (temp, filesystem, root) = fixture();
        fs::write(temp.path().join("folder/child.txt"), b"child").expect("nested fixture");
        let folder = listed_entries(&filesystem, &root)
            .into_iter()
            .find(|entry| entry.name == "folder")
            .expect("folder entry");
        let child = listed_entries(
            &filesystem,
            folder.directory.as_ref().expect("folder directory"),
        )
        .into_iter()
        .find(|entry| entry.name == "child.txt")
        .expect("child entry");
        let destination = temp.path().join("native-trash");
        fs::create_dir(&destination).expect("trash fixture");

        let removed = filesystem
            .trash_entry(
                &folder.reference,
                &AtomicBool::new(false),
                &MoveAsideTrash {
                    destination: destination.clone(),
                },
            )
            .expect("trash folder");

        assert_eq!(removed.reference, folder.reference);
        assert_eq!(removed.name, "folder");
        assert!(destination.join("folder/child.txt").is_file());
        assert!(matches!(
            filesystem.resolve_preview_path(&child.reference.id, &child.reference.location_id),
            Err(ExplorerError::InvalidReference)
        ));
    }

    #[test]
    fn unavailable_trash_is_explicit_and_preserves_the_source() {
        let temp = TempDir::new().expect("temporary directory");
        fs::write(temp.path().join("notes.md"), b"hello").expect("fixture file");
        let filesystem = LocalFilesystem::new_with_trash_support(
            vec![LocalRoot {
                id: "home",
                name: "Home",
                role: LocationRole::Home,
                path: temp.path().to_path_buf(),
            }],
            false,
        )
        .expect("local filesystem");
        let root = filesystem.locations().expect("locations")[0].root.clone();
        let entry = listed_entries(&filesystem, &root)
            .into_iter()
            .find(|entry| entry.name == "notes.md")
            .expect("notes entry");

        assert!(!entry.capabilities.trash);
        assert!(entry.capabilities.delete_permanently);
        assert!(matches!(
            filesystem.trash_entry(
                &entry.reference,
                &AtomicBool::new(false),
                &MoveAsideTrash {
                    destination: temp.path().join("unused")
                }
            ),
            Err(ExplorerError::Unsupported(_))
        ));
        assert!(temp.path().join("notes.md").is_file());
    }

    #[test]
    fn permanent_delete_removes_a_directory_tree_after_revalidation() {
        let (temp, filesystem, root) = fixture();
        fs::write(temp.path().join("folder/child.txt"), b"child").expect("nested fixture");
        let folder = listed_entries(&filesystem, &root)
            .into_iter()
            .find(|entry| entry.name == "folder")
            .expect("folder entry");

        filesystem
            .permanently_delete_entry(&folder.reference, &AtomicBool::new(false))
            .expect("delete folder");

        assert!(!temp.path().join("folder").exists());
        assert!(matches!(
            filesystem.resolve_preview_path(&folder.reference.id, &folder.reference.location_id),
            Err(ExplorerError::InvalidReference)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn permanent_delete_removes_a_symlink_without_following_its_target() {
        use std::os::unix::fs::symlink;

        let (temp, filesystem, root) = fixture();
        fs::write(temp.path().join("target.txt"), b"target").expect("target fixture");
        symlink(temp.path().join("target.txt"), temp.path().join("link.txt"))
            .expect("symlink fixture");
        let link = listed_entries(&filesystem, &root)
            .into_iter()
            .find(|entry| entry.name == "link.txt")
            .expect("link entry");

        filesystem
            .permanently_delete_entry(&link.reference, &AtomicBool::new(false))
            .expect("delete link");

        assert!(!temp.path().join("link.txt").exists());
        assert_eq!(
            fs::read(temp.path().join("target.txt")).expect("target preserved"),
            b"target"
        );
    }

    #[cfg(unix)]
    #[test]
    fn recursive_permanent_delete_does_not_follow_child_symlinks() {
        use std::os::unix::fs::symlink;

        let (temp, filesystem, root) = fixture();
        let external = temp.path().join("external");
        fs::create_dir(&external).expect("external fixture");
        fs::write(external.join("preserved.txt"), b"preserved").expect("external file");
        symlink(&external, temp.path().join("folder/external-link"))
            .expect("child symlink fixture");
        let folder = listed_entries(&filesystem, &root)
            .into_iter()
            .find(|entry| entry.name == "folder")
            .expect("folder entry");

        filesystem
            .permanently_delete_entry(&folder.reference, &AtomicBool::new(false))
            .expect("delete folder");

        assert!(!temp.path().join("folder").exists());
        assert_eq!(
            fs::read(external.join("preserved.txt")).expect("external target preserved"),
            b"preserved"
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
    }
}
