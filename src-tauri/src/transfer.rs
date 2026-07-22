use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use uuid::Uuid;

use crate::{filesystem::ExplorerError, local_relocate::relocate_no_replace};

pub(crate) const TRANSFER_CHUNK_BYTES: usize = 256 * 1024;
const MAX_PARTIAL_NAME_ATTEMPTS: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq)]
struct TransferFileIdentity {
    volume: u64,
    file: u64,
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

/// Owns exactly one transfer-created local artifact. Until `preserve` is called,
/// dropping this value removes whichever path Explora currently owns: the hidden
/// partial file before finalization or the final destination after finalization.
/// This keeps failed copy and verification paths from leaving ambiguous files.
pub(crate) struct OwnedLocalTransferArtifact {
    file: Option<File>,
    owned_path: PathBuf,
    final_path: PathBuf,
    finalized: bool,
    preserved: bool,
    bytes_written: u64,
    identity: Option<TransferFileIdentity>,
}

impl OwnedLocalTransferArtifact {
    pub(crate) fn create(
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

    pub(crate) fn write_chunk(&mut self, chunk: &[u8]) -> Result<u64, ExplorerError> {
        if self.finalized || chunk.len() > TRANSFER_CHUNK_BYTES {
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
        let file = self.file.take().ok_or(ExplorerError::StateUnavailable)?;
        file.sync_all()
            .map_err(|error| ExplorerError::io("flush", &self.owned_path, error))?;
        drop(file);
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
            let _ = fs::remove_file(&self.owned_path);
        }
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
}
