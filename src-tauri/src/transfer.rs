use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::SystemTime,
};

use uuid::Uuid;

use crate::{filesystem::ExplorerError, local_relocate::relocate_no_replace};

pub(crate) const TRANSFER_CHUNK_BYTES: usize = 256 * 1024;
const MAX_PARTIAL_NAME_ATTEMPTS: usize = 32;
pub(crate) const MAX_LOCAL_TRANSFER_ENTRIES: usize = 100_000;
pub(crate) const MAX_LOCAL_TRANSFER_DEPTH: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq)]
struct TransferFileIdentity {
    volume: u64,
    file: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LocalTransferEntryKind {
    File,
    Directory,
    Symlink { target_is_directory: bool },
}

#[derive(Clone)]
pub(crate) struct LocalTransferPlanEntry {
    pub(crate) relative_path: PathBuf,
    pub(crate) kind: LocalTransferEntryKind,
    pub(crate) len: u64,
    modified: Option<SystemTime>,
    identity: Option<TransferFileIdentity>,
    permission_fingerprint: u64,
    pub(crate) link_target: Option<PathBuf>,
    pub(crate) permissions: fs::Permissions,
}

impl LocalTransferPlanEntry {
    pub(crate) fn remote_relative_path(&self) -> Result<String, ExplorerError> {
        let mut components = Vec::new();
        for component in self.relative_path.components() {
            let Component::Normal(component) = component else {
                return Err(ExplorerError::InvalidReference);
            };
            let component = component.to_str().ok_or_else(|| {
                ExplorerError::Unsupported(
                    "A local file name cannot be represented on the remote filesystem.".to_owned(),
                )
            })?;
            if component.is_empty()
                || component == "."
                || component == ".."
                || component.len() > 1_024
                || component.contains('/')
                || component.contains('\0')
            {
                return Err(ExplorerError::Unsupported(
                    "A local file name is not valid on the remote filesystem.".to_owned(),
                ));
            }
            components.push(component);
        }
        Ok(components.join("/"))
    }

    pub(crate) fn remote_permissions(&self) -> Option<u32> {
        local_permissions_for_remote(&self.permissions)
    }
}

#[cfg(unix)]
fn local_permissions_for_remote(permissions: &fs::Permissions) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(permissions.mode() & 0o777)
}

#[cfg(windows)]
fn local_permissions_for_remote(permissions: &fs::Permissions) -> Option<u32> {
    Some(if permissions.readonly() { 0o444 } else { 0o644 })
}

#[cfg(not(any(unix, windows)))]
fn local_permissions_for_remote(_permissions: &fs::Permissions) -> Option<u32> {
    None
}

#[derive(Clone)]
pub(crate) struct LocalTransferPlan {
    source_root: PathBuf,
    entries: Vec<LocalTransferPlanEntry>,
    total_bytes: u64,
}

impl LocalTransferPlan {
    pub(crate) fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub(crate) fn root_is_directory(&self) -> bool {
        matches!(
            self.entries.first().map(|entry| entry.kind),
            Some(LocalTransferEntryKind::Directory)
        )
    }

    pub(crate) fn root_is_file(&self) -> bool {
        matches!(
            self.entries.first().map(|entry| entry.kind),
            Some(LocalTransferEntryKind::File)
        )
    }

    pub(crate) fn root_is_symlink(&self) -> bool {
        matches!(
            self.entries.first().map(|entry| entry.kind),
            Some(LocalTransferEntryKind::Symlink { .. })
        )
    }

    pub(crate) fn root_link_target(&self) -> Option<&Path> {
        self.entries.first()?.link_target.as_deref()
    }

    pub(crate) fn source_root(&self) -> &Path {
        &self.source_root
    }

    pub(crate) fn entries(&self) -> &[LocalTransferPlanEntry] {
        &self.entries
    }

    pub(crate) fn source_entry_path(&self, entry: &LocalTransferPlanEntry) -> PathBuf {
        transfer_entry_path(&self.source_root, &entry.relative_path)
    }

    fn root_kind(&self) -> Result<LocalTransferEntryKind, ExplorerError> {
        self.entries
            .first()
            .map(|entry| entry.kind)
            .ok_or(ExplorerError::StateUnavailable)
    }

    fn source_matches(&self, other: &Self) -> bool {
        self.entries.len() == other.entries.len()
            && self
                .entries
                .iter()
                .zip(&other.entries)
                .all(|(expected, actual)| {
                    expected.relative_path == actual.relative_path
                        && expected.kind == actual.kind
                        && expected.len == actual.len
                        && expected.modified == actual.modified
                        && expected.identity == actual.identity
                        && expected.permission_fingerprint == actual.permission_fingerprint
                        && expected.link_target == actual.link_target
                })
    }

    fn destination_matches(&self, other: &Self) -> bool {
        self.entries.len() == other.entries.len()
            && self
                .entries
                .iter()
                .zip(&other.entries)
                .all(|(expected, actual)| {
                    expected.relative_path == actual.relative_path
                        && expected.kind == actual.kind
                        && expected.len == actual.len
                        && expected.link_target == actual.link_target
                })
    }
}

#[cfg(unix)]
fn transfer_file_identity(metadata: &fs::Metadata) -> Option<TransferFileIdentity> {
    use std::os::unix::fs::MetadataExt;
    Some(TransferFileIdentity {
        volume: metadata.dev(),
        file: metadata.ino(),
    })
}

#[cfg(windows)]
fn transfer_file_identity(metadata: &fs::Metadata) -> Option<TransferFileIdentity> {
    use std::os::windows::fs::MetadataExt;
    Some(TransferFileIdentity {
        volume: u64::from(metadata.volume_serial_number()?),
        file: metadata.file_index()?,
    })
}

#[cfg(not(any(unix, windows)))]
fn transfer_file_identity(_metadata: &fs::Metadata) -> Option<TransferFileIdentity> {
    None
}

#[cfg(unix)]
fn transfer_permission_fingerprint(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::PermissionsExt;
    u64::from(metadata.permissions().mode())
}

#[cfg(windows)]
fn transfer_permission_fingerprint(metadata: &fs::Metadata) -> u64 {
    use std::os::windows::fs::MetadataExt;
    u64::from(metadata.file_attributes())
}

#[cfg(not(any(unix, windows)))]
fn transfer_permission_fingerprint(metadata: &fs::Metadata) -> u64 {
    u64::from(metadata.permissions().readonly())
}

/// Owns exactly one transfer-created local artifact. Until `preserve` is called,
/// dropping this value removes whichever path Explora currently owns: the hidden
/// partial file before finalization or the final destination after finalization.
/// This keeps failed copy and verification paths from leaving ambiguous files.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OwnedLocalArtifactKind {
    File,
    Directory,
    Symlink,
}

pub(crate) struct OwnedLocalTransferArtifact {
    file: Option<File>,
    owned_path: PathBuf,
    final_path: PathBuf,
    kind: OwnedLocalArtifactKind,
    finalized: bool,
    preserved: bool,
    bytes_written: u64,
    identity: Option<TransferFileIdentity>,
}

impl OwnedLocalTransferArtifact {
    #[cfg(test)]
    pub(crate) fn create(
        destination_directory: &Path,
        final_name: &std::ffi::OsStr,
    ) -> Result<Self, ExplorerError> {
        Self::create_file(destination_directory, final_name)
    }

    pub(crate) fn create_for_plan(
        destination_directory: &Path,
        final_name: &std::ffi::OsStr,
        plan: &LocalTransferPlan,
    ) -> Result<Self, ExplorerError> {
        match plan.root_kind()? {
            LocalTransferEntryKind::File => Self::create_file(destination_directory, final_name),
            LocalTransferEntryKind::Directory => {
                Self::create_directory(destination_directory, final_name)
            }
            LocalTransferEntryKind::Symlink {
                target_is_directory,
            } => {
                let target = plan
                    .entries
                    .first()
                    .and_then(|entry| entry.link_target.as_deref())
                    .ok_or(ExplorerError::StateUnavailable)?;
                Self::create_symlink(
                    destination_directory,
                    final_name,
                    target,
                    target_is_directory,
                )
            }
        }
    }

    pub(crate) fn create_file(
        destination_directory: &Path,
        final_name: &std::ffi::OsStr,
    ) -> Result<Self, ExplorerError> {
        let metadata = fs::symlink_metadata(destination_directory)
            .map_err(|error| ExplorerError::io("inspect", destination_directory, error))?;
        if !metadata.file_type().is_dir() {
            return Err(ExplorerError::DestinationUnavailable(
                "Choose a folder that can accept moved items.".to_owned(),
            ));
        }
        let final_path = destination_directory.join(final_name);
        match fs::symlink_metadata(&final_path) {
            Ok(_) => return Err(ExplorerError::Conflict),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(ExplorerError::io("inspect", &final_path, error)),
        }

        for _ in 0..MAX_PARTIAL_NAME_ATTEMPTS {
            let owned_path =
                destination_directory.join(format!(".explora-partial-{}", Uuid::new_v4()));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&owned_path) {
                Ok(file) => {
                    let identity = file
                        .metadata()
                        .ok()
                        .and_then(|metadata| transfer_file_identity(&metadata));
                    return Ok(Self {
                        file: Some(file),
                        owned_path,
                        final_path,
                        kind: OwnedLocalArtifactKind::File,
                        finalized: false,
                        preserved: false,
                        bytes_written: 0,
                        identity,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(ExplorerError::io("create", &owned_path, error)),
            }
        }
        Err(ExplorerError::DestinationUnavailable(
            "Explora could not allocate an owned partial file in the destination.".to_owned(),
        ))
    }

    pub(crate) fn create_directory(
        destination_directory: &Path,
        final_name: &std::ffi::OsStr,
    ) -> Result<Self, ExplorerError> {
        let final_path = validate_artifact_destination(destination_directory, final_name)?;
        for _ in 0..MAX_PARTIAL_NAME_ATTEMPTS {
            let owned_path = next_partial_path(destination_directory);
            let mut builder = DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&owned_path) {
                Ok(()) => {
                    let identity = fs::symlink_metadata(&owned_path)
                        .ok()
                        .and_then(|metadata| transfer_file_identity(&metadata));
                    return Ok(Self {
                        file: None,
                        owned_path,
                        final_path,
                        kind: OwnedLocalArtifactKind::Directory,
                        finalized: false,
                        preserved: false,
                        bytes_written: 0,
                        identity,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(ExplorerError::io("create", &owned_path, error)),
            }
        }
        Err(ExplorerError::DestinationUnavailable(
            "Explora could not allocate an owned partial folder in the destination.".to_owned(),
        ))
    }

    pub(crate) fn create_symlink(
        destination_directory: &Path,
        final_name: &std::ffi::OsStr,
        target: &Path,
        target_is_directory: bool,
    ) -> Result<Self, ExplorerError> {
        let final_path = validate_artifact_destination(destination_directory, final_name)?;
        for _ in 0..MAX_PARTIAL_NAME_ATTEMPTS {
            let owned_path = next_partial_path(destination_directory);
            match create_local_symlink(target, &owned_path, target_is_directory) {
                Ok(()) => {
                    let identity = fs::symlink_metadata(&owned_path)
                        .ok()
                        .and_then(|metadata| transfer_file_identity(&metadata));
                    return Ok(Self {
                        file: None,
                        owned_path,
                        final_path,
                        kind: OwnedLocalArtifactKind::Symlink,
                        finalized: false,
                        preserved: false,
                        bytes_written: 0,
                        identity,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(ExplorerError::io("create", &owned_path, error)),
            }
        }
        Err(ExplorerError::DestinationUnavailable(
            "Explora could not allocate an owned partial symbolic link in the destination."
                .to_owned(),
        ))
    }

    pub(crate) fn write_chunk(&mut self, chunk: &[u8]) -> Result<u64, ExplorerError> {
        if self.kind != OwnedLocalArtifactKind::File
            || self.finalized
            || chunk.len() > TRANSFER_CHUNK_BYTES
        {
            return Err(ExplorerError::InvalidConfiguration(
                "The transfer chunk is not valid for this partial file.".to_owned(),
            ));
        }
        self.file
            .as_mut()
            .ok_or(ExplorerError::StateUnavailable)?
            .write_all(chunk)
            .map_err(|error| ExplorerError::io("write", &self.owned_path, error))?;
        self.bytes_written = self
            .bytes_written
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| {
                ExplorerError::InvalidConfiguration(
                    "The transfer exceeded the supported size.".to_owned(),
                )
            })?;
        Ok(self.bytes_written)
    }

    pub(crate) fn finalize(&mut self) -> Result<&Path, ExplorerError> {
        if self.finalized {
            return Ok(&self.owned_path);
        }
        if self.kind == OwnedLocalArtifactKind::File {
            let file = self.file.take().ok_or(ExplorerError::StateUnavailable)?;
            file.sync_all()
                .map_err(|error| ExplorerError::io("flush", &self.owned_path, error))?;
            drop(file);
        }
        relocate_no_replace(&self.owned_path, &self.final_path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                ExplorerError::Conflict
            } else {
                ExplorerError::io("finalize", &self.owned_path, error)
            }
        })?;
        self.owned_path = self.final_path.clone();
        self.finalized = true;
        Ok(&self.owned_path)
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.owned_path
    }

    pub(crate) fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    pub(crate) fn current_path(&self) -> &Path {
        &self.owned_path
    }

    pub(crate) fn entry_path(&self, relative_path: &Path) -> Result<PathBuf, ExplorerError> {
        if relative_path.as_os_str().is_empty() {
            return Ok(self.owned_path.clone());
        }
        if relative_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(ExplorerError::InvalidReference);
        }
        Ok(self.owned_path.join(relative_path))
    }

    pub(crate) fn create_directory_entry(&self, relative_path: &Path) -> Result<(), ExplorerError> {
        if self.kind != OwnedLocalArtifactKind::Directory || self.finalized {
            return Err(ExplorerError::StateUnavailable);
        }
        create_owned_directory(&self.entry_path(relative_path)?)
    }

    pub(crate) fn create_symlink_entry(
        &self,
        relative_path: &Path,
        target: &Path,
        target_is_directory: bool,
    ) -> Result<(), ExplorerError> {
        if self.kind != OwnedLocalArtifactKind::Directory || self.finalized {
            return Err(ExplorerError::StateUnavailable);
        }
        create_local_symlink(
            target,
            &self.entry_path(relative_path)?,
            target_is_directory,
        )
        .map_err(|error| ExplorerError::io("create", relative_path, error))
    }

    pub(crate) fn create_file_entry(&self, relative_path: &Path) -> Result<File, ExplorerError> {
        if self.kind != OwnedLocalArtifactKind::Directory || self.finalized {
            return Err(ExplorerError::StateUnavailable);
        }
        create_owned_file(&self.entry_path(relative_path)?)
    }

    pub(crate) fn take_file(&mut self) -> Result<File, ExplorerError> {
        if self.kind != OwnedLocalArtifactKind::File || self.finalized {
            return Err(ExplorerError::StateUnavailable);
        }
        self.file.take().ok_or(ExplorerError::StateUnavailable)
    }

    pub(crate) fn restore_file(
        &mut self,
        file: File,
        bytes_written: u64,
    ) -> Result<(), ExplorerError> {
        if self.kind != OwnedLocalArtifactKind::File || self.finalized || self.file.is_some() {
            return Err(ExplorerError::StateUnavailable);
        }
        self.file = Some(file);
        self.bytes_written = bytes_written;
        Ok(())
    }

    pub(crate) fn preserve(mut self) -> PathBuf {
        self.preserved = true;
        self.owned_path.clone()
    }
}

impl Drop for OwnedLocalTransferArtifact {
    fn drop(&mut self) {
        if self.preserved {
            return;
        }
        self.file.take();
        let current_identity = fs::symlink_metadata(&self.owned_path)
            .ok()
            .and_then(|metadata| transfer_file_identity(&metadata));
        if self.identity.is_some() && current_identity == self.identity {
            remove_owned_artifact(&self.owned_path, self.kind);
        }
    }
}

fn remove_owned_artifact(path: &Path, kind: OwnedLocalArtifactKind) {
    match kind {
        OwnedLocalArtifactKind::Directory => {
            make_owned_tree_removable(path);
            let _ = fs::remove_dir_all(path);
        }
        OwnedLocalArtifactKind::File => {
            make_owned_path_removable(path, false);
            let _ = fs::remove_file(path);
        }
        OwnedLocalArtifactKind::Symlink => {
            let _ = fs::remove_file(path);
        }
    }
}

fn make_owned_tree_removable(root: &Path) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            continue;
        }
        make_owned_path_removable(&path, file_type.is_dir());
        if file_type.is_dir() {
            let Ok(children) = fs::read_dir(&path) else {
                continue;
            };
            pending.extend(children.filter_map(Result::ok).map(|entry| entry.path()));
        }
    }
}

#[cfg(unix)]
fn make_owned_path_removable(path: &Path, is_directory: bool) {
    use std::os::unix::fs::PermissionsExt;

    if let Ok(metadata) = fs::symlink_metadata(path) {
        let mut permissions = metadata.permissions();
        let required = if is_directory { 0o700 } else { 0o600 };
        permissions.set_mode(permissions.mode() | required);
        let _ = fs::set_permissions(path, permissions);
    }
}

#[cfg(windows)]
fn make_owned_path_removable(path: &Path, _is_directory: bool) {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        let mut permissions = metadata.permissions();
        permissions.set_readonly(false);
        let _ = fs::set_permissions(path, permissions);
    }
}

#[cfg(not(any(unix, windows)))]
fn make_owned_path_removable(_path: &Path, _is_directory: bool) {}

fn validate_artifact_destination(
    destination_directory: &Path,
    final_name: &std::ffi::OsStr,
) -> Result<PathBuf, ExplorerError> {
    let metadata = fs::symlink_metadata(destination_directory)
        .map_err(|error| ExplorerError::io("inspect", destination_directory, error))?;
    if !metadata.file_type().is_dir() {
        return Err(ExplorerError::DestinationUnavailable(
            "Choose a folder that can accept moved items.".to_owned(),
        ));
    }
    let final_path = destination_directory.join(final_name);
    match fs::symlink_metadata(&final_path) {
        Ok(_) => Err(ExplorerError::Conflict),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(final_path),
        Err(error) => Err(ExplorerError::io("inspect", &final_path, error)),
    }
}

fn next_partial_path(destination_directory: &Path) -> PathBuf {
    destination_directory.join(format!(".explora-partial-{}", Uuid::new_v4()))
}

#[cfg(unix)]
fn create_local_symlink(
    target: &Path,
    link: &Path,
    _target_is_directory: bool,
) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_local_symlink(
    target: &Path,
    link: &Path,
    target_is_directory: bool,
) -> std::io::Result<()> {
    if target_is_directory {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

#[cfg(not(any(unix, windows)))]
fn create_local_symlink(
    _target: &Path,
    _link: &Path,
    _target_is_directory: bool,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "symbolic-link creation is unavailable on this platform",
    ))
}

#[cfg(windows)]
fn symlink_targets_directory(file_type: fs::FileType) -> bool {
    use std::os::windows::fs::FileTypeExt;
    file_type.is_symlink_dir()
}

#[cfg(not(windows))]
fn symlink_targets_directory(_file_type: fs::FileType) -> bool {
    false
}

pub(crate) fn plan_local_transfer(
    source_root: &Path,
    cancelled: &AtomicBool,
) -> Result<LocalTransferPlan, ExplorerError> {
    let mut pending = vec![(source_root.to_path_buf(), PathBuf::new(), 0_usize)];
    let mut entries = Vec::new();
    let mut total_bytes = 0_u64;

    while let Some((path, relative_path, depth)) = pending.pop() {
        ensure_not_cancelled(cancelled)?;
        if depth > MAX_LOCAL_TRANSFER_DEPTH {
            return Err(ExplorerError::Unsupported(
                "This folder is nested too deeply to transfer safely in one operation.".to_owned(),
            ));
        }
        if entries.len() >= MAX_LOCAL_TRANSFER_ENTRIES {
            return Err(ExplorerError::Unsupported(
                "This folder contains too many items to transfer safely in one operation."
                    .to_owned(),
            ));
        }
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ExplorerError::SourceChanged
            } else {
                ExplorerError::io("inspect", &path, error)
            }
        })?;
        let file_type = metadata.file_type();
        let (kind, link_target) = if file_type.is_symlink() {
            (
                LocalTransferEntryKind::Symlink {
                    target_is_directory: symlink_targets_directory(file_type),
                },
                Some(
                    fs::read_link(&path)
                        .map_err(|error| ExplorerError::io("read", &path, error))?,
                ),
            )
        } else if file_type.is_dir() {
            (LocalTransferEntryKind::Directory, None)
        } else if file_type.is_file() {
            total_bytes = total_bytes.checked_add(metadata.len()).ok_or_else(|| {
                ExplorerError::Unsupported(
                    "This transfer exceeds the supported byte count.".to_owned(),
                )
            })?;
            (LocalTransferEntryKind::File, None)
        } else {
            return Err(ExplorerError::Unsupported(
                "This filesystem entry type cannot be transferred safely.".to_owned(),
            ));
        };
        entries.push(LocalTransferPlanEntry {
            relative_path: relative_path.clone(),
            kind,
            len: metadata.len(),
            modified: metadata.modified().ok(),
            identity: transfer_file_identity(&metadata),
            permission_fingerprint: transfer_permission_fingerprint(&metadata),
            link_target,
            permissions: metadata.permissions(),
        });

        if kind == LocalTransferEntryKind::Directory {
            let directory =
                fs::read_dir(&path).map_err(|error| ExplorerError::io("open", &path, error))?;
            let mut children = Vec::new();
            for child in directory {
                ensure_not_cancelled(cancelled)?;
                let child = child.map_err(|error| ExplorerError::Io {
                    message: "Explora could not enumerate an item for transfer.".to_owned(),
                    kind: error.kind(),
                })?;
                children.push((child.file_name(), child.path()));
            }
            children.sort_by(|left, right| left.0.cmp(&right.0));
            for (name, child_path) in children.into_iter().rev() {
                pending.push((child_path, relative_path.join(name), depth + 1));
            }
        }
    }

    entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(LocalTransferPlan {
        source_root: source_root.to_path_buf(),
        entries,
        total_bytes,
    })
}

pub(crate) fn copy_local_transfer_into_owned_artifact<F>(
    plan: &LocalTransferPlan,
    artifact: &mut OwnedLocalTransferArtifact,
    cancelled: &AtomicBool,
    mut on_progress: F,
) -> Result<u64, ExplorerError>
where
    F: FnMut(u64) -> Result<(), ExplorerError>,
{
    let root = plan
        .entries
        .first()
        .ok_or(ExplorerError::StateUnavailable)?;
    match root.kind {
        LocalTransferEntryKind::File => {
            copy_local_file_into_owned_partial(
                &plan.source_root,
                artifact,
                cancelled,
                &mut on_progress,
            )?;
            fs::set_permissions(&artifact.owned_path, root.permissions.clone()).map_err(
                |error| ExplorerError::io("set permissions on", &artifact.owned_path, error),
            )?;
        }
        LocalTransferEntryKind::Symlink { .. } => {
            ensure_not_cancelled(cancelled)?;
        }
        LocalTransferEntryKind::Directory => {
            let mut completed_bytes = 0_u64;
            for entry in plan.entries.iter().skip(1) {
                ensure_not_cancelled(cancelled)?;
                ensure_planned_source_entry_unchanged(plan, entry)?;
                let source = transfer_entry_path(&plan.source_root, &entry.relative_path);
                let destination = transfer_entry_path(&artifact.owned_path, &entry.relative_path);
                match entry.kind {
                    LocalTransferEntryKind::Directory => {
                        create_owned_directory(&destination)?;
                    }
                    LocalTransferEntryKind::Symlink {
                        target_is_directory,
                    } => {
                        let target = entry
                            .link_target
                            .as_deref()
                            .ok_or(ExplorerError::StateUnavailable)?;
                        create_local_symlink(target, &destination, target_is_directory)
                            .map_err(|error| ExplorerError::io("create", &destination, error))?;
                    }
                    LocalTransferEntryKind::File => {
                        let mut destination_file = create_owned_file(&destination)?;
                        copy_local_file_to_writer(
                            &source,
                            &mut destination_file,
                            cancelled,
                            &mut completed_bytes,
                            &mut on_progress,
                        )?;
                        destination_file
                            .sync_all()
                            .map_err(|error| ExplorerError::io("flush", &destination, error))?;
                    }
                }
            }

            // Apply source permissions only after all descendants have been
            // created, so read-only directories cannot interrupt owned cleanup.
            for entry in plan.entries.iter().rev() {
                if matches!(entry.kind, LocalTransferEntryKind::Symlink { .. }) {
                    continue;
                }
                let destination = transfer_entry_path(&artifact.owned_path, &entry.relative_path);
                fs::set_permissions(&destination, entry.permissions.clone()).map_err(|error| {
                    ExplorerError::io("set permissions on", &destination, error)
                })?;
            }
            artifact.bytes_written = completed_bytes;
        }
    }
    Ok(artifact.bytes_written())
}

pub(crate) fn verify_local_transfer(
    plan: &LocalTransferPlan,
    destination: &Path,
    cancelled: &AtomicBool,
) -> Result<(), ExplorerError> {
    ensure_not_cancelled(cancelled)?;
    let current_source = plan_local_transfer(&plan.source_root, cancelled)?;
    if !plan.source_matches(&current_source) {
        return Err(ExplorerError::SourceChanged);
    }
    let current_destination =
        plan_local_transfer(destination, cancelled).map_err(|error| match error {
            ExplorerError::SourceChanged => ExplorerError::Unexpected(
                "The finalized transfer destination disappeared before verification.".to_owned(),
            ),
            other => other,
        })?;
    if !plan.destination_matches(&current_destination) {
        return Err(ExplorerError::Unexpected(
            "The transferred entry did not match its source structure.".to_owned(),
        ));
    }
    for entry in &plan.entries {
        ensure_not_cancelled(cancelled)?;
        if entry.kind == LocalTransferEntryKind::File {
            verify_local_file_copy(
                &transfer_entry_path(&plan.source_root, &entry.relative_path),
                &transfer_entry_path(destination, &entry.relative_path),
                cancelled,
            )?;
        }
    }
    Ok(())
}

pub(crate) fn revalidate_local_transfer_source(
    plan: &LocalTransferPlan,
    cancelled: &AtomicBool,
) -> Result<(), ExplorerError> {
    ensure_not_cancelled(cancelled)?;
    let current_source = plan_local_transfer(&plan.source_root, cancelled)?;
    if plan.source_matches(&current_source) {
        Ok(())
    } else {
        Err(ExplorerError::SourceChanged)
    }
}

pub(crate) fn remove_verified_local_transfer_source(
    plan: &LocalTransferPlan,
) -> Result<(), ExplorerError> {
    // Delete only entries that were part of the verified snapshot. If a new
    // child appears after revalidation, removing its parent fails as non-empty
    // instead of silently deleting data that was never copied.
    for entry in plan.entries.iter().rev() {
        let path = transfer_entry_path(&plan.source_root, &entry.relative_path);
        ensure_planned_source_entry_identity(entry, &path)?;
        match entry.kind {
            LocalTransferEntryKind::Directory => fs::remove_dir(&path),
            LocalTransferEntryKind::File | LocalTransferEntryKind::Symlink { .. } => {
                fs::remove_file(&path)
            }
        }
        .map_err(|error| ExplorerError::io("delete", &path, error))?;
    }
    Ok(())
}

fn ensure_planned_source_entry_identity(
    entry: &LocalTransferPlanEntry,
    path: &Path,
) -> Result<(), ExplorerError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ExplorerError::SourceChanged
        } else {
            ExplorerError::io("inspect", path, error)
        }
    })?;
    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        LocalTransferEntryKind::Symlink {
            target_is_directory: symlink_targets_directory(file_type),
        }
    } else if file_type.is_dir() {
        LocalTransferEntryKind::Directory
    } else if file_type.is_file() {
        LocalTransferEntryKind::File
    } else {
        return Err(ExplorerError::SourceChanged);
    };
    if kind != entry.kind || transfer_file_identity(&metadata) != entry.identity {
        return Err(ExplorerError::SourceChanged);
    }
    if let LocalTransferEntryKind::Symlink { .. } = kind {
        let target = fs::read_link(path).map_err(|error| ExplorerError::io("read", path, error))?;
        if entry.link_target.as_deref() != Some(target.as_path()) {
            return Err(ExplorerError::SourceChanged);
        }
    }
    Ok(())
}

fn ensure_planned_source_entry_unchanged(
    plan: &LocalTransferPlan,
    entry: &LocalTransferPlanEntry,
) -> Result<(), ExplorerError> {
    let path = transfer_entry_path(&plan.source_root, &entry.relative_path);
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ExplorerError::SourceChanged
        } else {
            ExplorerError::io("inspect", &path, error)
        }
    })?;
    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        LocalTransferEntryKind::Symlink {
            target_is_directory: symlink_targets_directory(file_type),
        }
    } else if file_type.is_dir() {
        LocalTransferEntryKind::Directory
    } else if file_type.is_file() {
        LocalTransferEntryKind::File
    } else {
        return Err(ExplorerError::SourceChanged);
    };
    let link_target = if file_type.is_symlink() {
        Some(fs::read_link(&path).map_err(|error| ExplorerError::io("read", &path, error))?)
    } else {
        None
    };
    if kind != entry.kind
        || metadata.len() != entry.len
        || metadata.modified().ok() != entry.modified
        || transfer_file_identity(&metadata) != entry.identity
        || transfer_permission_fingerprint(&metadata) != entry.permission_fingerprint
        || link_target != entry.link_target
    {
        return Err(ExplorerError::SourceChanged);
    }
    Ok(())
}

fn create_owned_directory(path: &Path) -> Result<(), ExplorerError> {
    let mut builder = DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|error| ExplorerError::io("create", path, error))
}

fn transfer_entry_path(root: &Path, relative_path: &Path) -> PathBuf {
    if relative_path.as_os_str().is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative_path)
    }
}

fn create_owned_file(path: &Path) -> Result<File, ExplorerError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|error| ExplorerError::io("create", path, error))
}

fn copy_local_file_to_writer<F>(
    source: &Path,
    destination: &mut File,
    cancelled: &AtomicBool,
    completed_bytes: &mut u64,
    on_progress: &mut F,
) -> Result<(), ExplorerError>
where
    F: FnMut(u64) -> Result<(), ExplorerError>,
{
    let mut source_file =
        File::open(source).map_err(|error| ExplorerError::io("open", source, error))?;
    let mut buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
    loop {
        ensure_not_cancelled(cancelled)?;
        let read = source_file
            .read(&mut buffer)
            .map_err(|error| ExplorerError::io("read", source, error))?;
        if read == 0 {
            return Ok(());
        }
        destination
            .write_all(&buffer[..read])
            .map_err(|error| ExplorerError::Io {
                message: "Explora could not write an owned transfer file.".to_owned(),
                kind: error.kind(),
            })?;
        *completed_bytes = completed_bytes.checked_add(read as u64).ok_or_else(|| {
            ExplorerError::InvalidConfiguration(
                "The transfer exceeded the supported size.".to_owned(),
            )
        })?;
        on_progress(*completed_bytes)?;
    }
}

fn ensure_not_cancelled(cancelled: &AtomicBool) -> Result<(), ExplorerError> {
    if cancelled.load(Ordering::SeqCst) {
        Err(ExplorerError::Cancelled)
    } else {
        Ok(())
    }
}

pub(crate) fn copy_local_file_into_owned_partial<F>(
    source: &Path,
    artifact: &mut OwnedLocalTransferArtifact,
    cancelled: &AtomicBool,
    mut on_progress: F,
) -> Result<u64, ExplorerError>
where
    F: FnMut(u64) -> Result<(), ExplorerError>,
{
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| ExplorerError::io("inspect", source, error))?;
    if !metadata.file_type().is_file() {
        return Err(ExplorerError::Unsupported(
            "This transfer path currently accepts regular files only.".to_owned(),
        ));
    }
    let mut reader =
        File::open(source).map_err(|error| ExplorerError::io("open", source, error))?;
    let mut buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
    loop {
        if cancelled.load(Ordering::SeqCst) {
            return Err(ExplorerError::Cancelled);
        }
        let read = reader
            .read(&mut buffer)
            .map_err(|error| ExplorerError::io("read", source, error))?;
        if read == 0 {
            break;
        }
        let completed = artifact.write_chunk(&buffer[..read])?;
        on_progress(completed)?;
    }
    Ok(artifact.bytes_written())
}

pub(crate) fn verify_local_file_copy(
    source: &Path,
    destination: &Path,
    cancelled: &AtomicBool,
) -> Result<(), ExplorerError> {
    let mut source_file =
        File::open(source).map_err(|error| ExplorerError::io("open", source, error))?;
    let mut destination_file =
        File::open(destination).map_err(|error| ExplorerError::io("open", destination, error))?;
    let mut source_buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
    let mut destination_buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
    loop {
        if cancelled.load(Ordering::SeqCst) {
            return Err(ExplorerError::Cancelled);
        }
        let source_read = source_file
            .read(&mut source_buffer)
            .map_err(|error| ExplorerError::io("verify", source, error))?;
        let destination_read = destination_file
            .read(&mut destination_buffer)
            .map_err(|error| ExplorerError::io("verify", destination, error))?;
        if source_read != destination_read
            || source_buffer[..source_read] != destination_buffer[..destination_read]
        {
            return Err(ExplorerError::Unexpected(
                "The transferred file did not match its source.".to_owned(),
            ));
        }
        if source_read == 0 {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_partial_cleans_up_failed_and_unverified_transfers() {
        let temp = tempfile::tempdir().expect("transfer fixture");
        let mut artifact =
            OwnedLocalTransferArtifact::create(temp.path(), std::ffi::OsStr::new("final.bin"))
                .expect("owned partial");
        let partial = artifact.path().to_path_buf();
        artifact.write_chunk(b"partial").expect("write partial");
        assert!(partial.exists());
        drop(artifact);
        assert!(!partial.exists());

        let mut finalized =
            OwnedLocalTransferArtifact::create(temp.path(), std::ffi::OsStr::new("final.bin"))
                .expect("second partial");
        finalized.write_chunk(b"unverified").expect("write file");
        let final_path = finalized.finalize().expect("finalize").to_path_buf();
        assert!(final_path.exists());
        drop(finalized);
        assert!(!final_path.exists());
    }

    #[test]
    fn verified_transfer_is_preserved_and_never_replaces_a_conflict() {
        let temp = tempfile::tempdir().expect("transfer fixture");
        let source = temp.path().join("source.bin");
        fs::write(&source, vec![0x5a; TRANSFER_CHUNK_BYTES + 17]).expect("source bytes");
        let mut artifact =
            OwnedLocalTransferArtifact::create(temp.path(), std::ffi::OsStr::new("final.bin"))
                .expect("owned partial");
        let progress = std::sync::Mutex::new(Vec::new());
        copy_local_file_into_owned_partial(
            &source,
            &mut artifact,
            &AtomicBool::new(false),
            |completed| {
                progress.lock().expect("progress").push(completed);
                Ok(())
            },
        )
        .expect("copy source");
        let final_path = artifact.finalize().expect("finalize").to_path_buf();
        verify_local_file_copy(&source, &final_path, &AtomicBool::new(false))
            .expect("verify bytes");
        let preserved = artifact.preserve();
        assert_eq!(
            fs::read(&source).expect("source"),
            fs::read(&preserved).expect("copy")
        );
        assert_eq!(
            progress.lock().expect("progress").last().copied(),
            Some((TRANSFER_CHUNK_BYTES + 17) as u64)
        );

        assert!(matches!(
            OwnedLocalTransferArtifact::create(temp.path(), std::ffi::OsStr::new("final.bin")),
            Err(ExplorerError::Conflict)
        ));
    }

    #[test]
    fn cancellation_removes_the_owned_partial_and_preserves_the_source() {
        let temp = tempfile::tempdir().expect("transfer fixture");
        let source = temp.path().join("source.bin");
        fs::write(&source, b"source remains").expect("source bytes");
        let partial_path = {
            let mut artifact =
                OwnedLocalTransferArtifact::create(temp.path(), std::ffi::OsStr::new("final.bin"))
                    .expect("owned partial");
            let partial_path = artifact.path().to_path_buf();
            let cancelled = AtomicBool::new(true);
            assert!(matches!(
                copy_local_file_into_owned_partial(&source, &mut artifact, &cancelled, |_| Ok(())),
                Err(ExplorerError::Cancelled)
            ));
            partial_path
        };
        assert!(!partial_path.exists());
        assert_eq!(
            fs::read(source).expect("source preserved"),
            b"source remains"
        );
    }

    #[test]
    fn cleanup_never_removes_a_replacement_at_the_owned_path() {
        let temp = tempfile::tempdir().expect("transfer fixture");
        let mut artifact =
            OwnedLocalTransferArtifact::create(temp.path(), std::ffi::OsStr::new("final.bin"))
                .expect("owned partial");
        let replacement_path = temp.path().join("replacement.tmp");
        fs::write(&replacement_path, b"replacement").expect("replacement fixture");
        artifact.write_chunk(b"owned").expect("owned bytes");
        let final_path = artifact.finalize().expect("finalize").to_path_buf();
        fs::remove_file(&final_path).expect("replace owned artifact");
        fs::rename(replacement_path, &final_path).expect("replacement bytes");

        drop(artifact);

        assert_eq!(
            fs::read(final_path).expect("replacement preserved"),
            b"replacement"
        );
    }

    #[test]
    fn directory_transfer_is_bounded_verified_and_cleans_unpreserved_results() {
        let temp = tempfile::tempdir().expect("transfer fixture");
        let source = temp.path().join("source-tree");
        let destination = temp.path().join("destination");
        fs::create_dir(&source).expect("source tree");
        fs::create_dir(source.join("nested")).expect("nested tree");
        fs::create_dir(source.join("empty")).expect("empty tree");
        fs::write(
            source.join("nested/payload.bin"),
            vec![0x3c; TRANSFER_CHUNK_BYTES + 19],
        )
        .expect("payload");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&source, fs::Permissions::from_mode(0o750))
                .expect("source permissions");
            fs::set_permissions(
                source.join("nested/payload.bin"),
                fs::Permissions::from_mode(0o640),
            )
            .expect("payload permissions");
        }
        fs::create_dir(&destination).expect("destination");

        let cancelled = AtomicBool::new(false);
        let plan = plan_local_transfer(&source, &cancelled).expect("transfer plan");
        assert_eq!(plan.entries.len(), 4);
        assert_eq!(plan.total_bytes(), (TRANSFER_CHUNK_BYTES + 19) as u64);
        let mut artifact = OwnedLocalTransferArtifact::create_for_plan(
            &destination,
            std::ffi::OsStr::new("source-tree"),
            &plan,
        )
        .expect("owned tree");
        let progress = std::sync::Mutex::new(Vec::new());
        copy_local_transfer_into_owned_artifact(&plan, &mut artifact, &cancelled, |completed| {
            progress.lock().expect("progress").push(completed);
            Ok(())
        })
        .expect("copy tree");
        let finalized = artifact.finalize().expect("finalize tree").to_path_buf();
        verify_local_transfer(&plan, &finalized, &cancelled).expect("verify tree");
        assert_eq!(
            progress.lock().expect("progress").last().copied(),
            Some((TRANSFER_CHUNK_BYTES + 19) as u64)
        );
        assert!(finalized.join("empty").is_dir());
        assert_eq!(
            fs::read(finalized.join("nested/payload.bin")).expect("copied payload"),
            vec![0x3c; TRANSFER_CHUNK_BYTES + 19]
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::symlink_metadata(&finalized)
                    .expect("destination permissions")
                    .permissions()
                    .mode()
                    & 0o777,
                0o750
            );
            assert_eq!(
                fs::symlink_metadata(finalized.join("nested/payload.bin"))
                    .expect("payload permissions")
                    .permissions()
                    .mode()
                    & 0o777,
                0o640
            );
        }

        drop(artifact);
        assert!(!finalized.exists());
        assert!(source.exists());
    }

    #[test]
    fn changed_directory_source_fails_verification_and_removes_the_copy() {
        let temp = tempfile::tempdir().expect("transfer fixture");
        let source = temp.path().join("source-tree");
        let destination = temp.path().join("destination");
        fs::create_dir(&source).expect("source tree");
        fs::write(source.join("payload.bin"), b"original").expect("payload");
        fs::create_dir(&destination).expect("destination");
        let cancelled = AtomicBool::new(false);
        let plan = plan_local_transfer(&source, &cancelled).expect("transfer plan");
        let mut artifact = OwnedLocalTransferArtifact::create_for_plan(
            &destination,
            std::ffi::OsStr::new("source-tree"),
            &plan,
        )
        .expect("owned tree");
        copy_local_transfer_into_owned_artifact(&plan, &mut artifact, &cancelled, |_| Ok(()))
            .expect("copy tree");
        let finalized = artifact.finalize().expect("finalize tree").to_path_buf();
        fs::write(source.join("payload.bin"), b"changed!").expect("mutate source");

        assert!(matches!(
            verify_local_transfer(&plan, &finalized, &cancelled),
            Err(ExplorerError::SourceChanged)
        ));
        drop(artifact);
        assert!(!finalized.exists());
        assert_eq!(
            fs::read(source.join("payload.bin")).expect("changed source preserved"),
            b"changed!"
        );
    }

    #[test]
    fn verified_source_removal_never_deletes_an_unplanned_late_child() {
        let temp = tempfile::tempdir().expect("transfer fixture");
        let source = temp.path().join("source-tree");
        fs::create_dir(&source).expect("source tree");
        fs::write(source.join("planned.txt"), b"planned").expect("planned child");
        let plan = plan_local_transfer(&source, &AtomicBool::new(false)).expect("transfer plan");
        fs::write(source.join("late.txt"), b"late").expect("late child");

        assert!(remove_verified_local_transfer_source(&plan).is_err());
        assert_eq!(
            fs::read(source.join("late.txt")).expect("late child preserved"),
            b"late"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_link_transfer_preserves_the_link_without_following_it() {
        let temp = tempfile::tempdir().expect("transfer fixture");
        let source = temp.path().join("source-link");
        let destination = temp.path().join("destination");
        fs::create_dir(&destination).expect("destination");
        std::os::unix::fs::symlink("missing-target", &source).expect("dangling source link");
        let cancelled = AtomicBool::new(false);
        let plan = plan_local_transfer(&source, &cancelled).expect("transfer plan");
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.total_bytes(), 0);
        let mut artifact = OwnedLocalTransferArtifact::create_for_plan(
            &destination,
            std::ffi::OsStr::new("source-link"),
            &plan,
        )
        .expect("owned link");
        copy_local_transfer_into_owned_artifact(&plan, &mut artifact, &cancelled, |_| Ok(()))
            .expect("copy link");
        let finalized = artifact.finalize().expect("finalize link").to_path_buf();
        verify_local_transfer(&plan, &finalized, &cancelled).expect("verify link");
        assert!(fs::symlink_metadata(&finalized)
            .expect("link metadata")
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_link(&finalized).expect("link target"),
            PathBuf::from("missing-target")
        );
    }
}
