use std::sync::Arc;

use russh_sftp::{
    client::{error::Error as SftpError, fs::File as SftpFile, SftpSession},
    protocol::{FileAttributes, OpenFlags, StatusCode},
};
use tokio::io::{AsyncWriteExt, Error as TokioIoError};
use uuid::Uuid;

use crate::{filesystem::ExplorerError, transfer::TRANSFER_CHUNK_BYTES};

const MAX_REMOTE_PARTIAL_NAME_ATTEMPTS: usize = 32;

/// Owns a uniquely named remote partial file until it is either atomically
/// finalized or explicitly abandoned. Callers must verify the hidden partial
/// before finalization, because SFTP does not expose a stable file identity that
/// would make deleting a replaced final path safe.
pub(crate) struct OwnedRemoteFileArtifact {
    sftp: Arc<SftpSession>,
    file: Option<SftpFile>,
    partial_path: String,
    final_path: String,
    finalized: bool,
    preserved: bool,
    bytes_written: u64,
}

impl OwnedRemoteFileArtifact {
    pub(crate) async fn create(
        sftp: Arc<SftpSession>,
        destination_directory: &str,
        final_name: &str,
    ) -> Result<Self, ExplorerError> {
        let final_path = remote_join(destination_directory, final_name);
        ensure_remote_path_absent(&sftp, &final_path).await?;

        for _ in 0..MAX_REMOTE_PARTIAL_NAME_ATTEMPTS {
            let partial_path = remote_join(
                destination_directory,
                &format!(".explora-partial-{}", Uuid::new_v4()),
            );
            let mut attributes = FileAttributes::empty();
            attributes.permissions = Some(0o600);
            match sftp
                .open_with_flags_and_attributes(
                    partial_path.clone(),
                    OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
                    attributes,
                )
                .await
            {
                Ok(file) => {
                    return Ok(Self {
                        sftp,
                        file: Some(file),
                        partial_path,
                        final_path,
                        finalized: false,
                        preserved: false,
                        bytes_written: 0,
                    });
                }
                Err(SftpError::Status(status))
                    if status.status_code == StatusCode::Failure
                        || status.status_code == StatusCode::PermissionDenied =>
                {
                    // Some servers collapse create-exclusive collisions into a
                    // generic failure. Recheck before deciding whether to retry.
                    match sftp.symlink_metadata(partial_path.clone()).await {
                        Ok(_) => continue,
                        Err(SftpError::Status(missing))
                            if missing.status_code == StatusCode::NoSuchFile =>
                        {
                            return Err(map_remote_transfer_error(SftpError::Status(status)));
                        }
                        Err(error) => return Err(map_remote_transfer_error(error)),
                    }
                }
                Err(error) => return Err(map_remote_transfer_error(error)),
            }
        }

        Err(ExplorerError::DestinationUnavailable(
            "Explora could not allocate an owned partial file on the remote destination."
                .to_owned(),
        ))
    }

    pub(crate) fn partial_path(&self) -> &str {
        &self.partial_path
    }

    pub(crate) fn final_path(&self) -> &str {
        &self.final_path
    }

    pub(crate) fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    pub(crate) async fn write_chunk(&mut self, chunk: &[u8]) -> Result<u64, ExplorerError> {
        if self.finalized || chunk.len() > TRANSFER_CHUNK_BYTES {
            return Err(ExplorerError::InvalidConfiguration(
                "The remote transfer chunk is not valid for this partial file.".to_owned(),
            ));
        }
        self.file
            .as_mut()
            .ok_or(ExplorerError::StateUnavailable)?
            .write_all(chunk)
            .await
            .map_err(map_remote_write_error)?;
        self.bytes_written = self
            .bytes_written
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| {
                ExplorerError::InvalidConfiguration(
                    "The remote transfer exceeded the supported size.".to_owned(),
                )
            })?;
        Ok(self.bytes_written)
    }

    pub(crate) async fn close_for_verification(&mut self) -> Result<(), ExplorerError> {
        let mut file = self.file.take().ok_or(ExplorerError::StateUnavailable)?;
        file.flush().await.map_err(map_remote_write_error)?;
        file.sync_all().await.map_err(map_remote_transfer_error)?;
        file.shutdown().await.map_err(map_remote_write_error)
    }

    pub(crate) async fn finalize_no_replace(&mut self) -> Result<&str, ExplorerError> {
        if self.finalized {
            return Ok(&self.final_path);
        }
        if self.file.is_some() {
            return Err(ExplorerError::StateUnavailable);
        }
        ensure_remote_path_absent(&self.sftp, &self.final_path).await?;
        let rename = self
            .sftp
            .rename(self.partial_path.clone(), self.final_path.clone())
            .await;
        match rename {
            Ok(()) => {
                self.finalized = true;
                Ok(&self.final_path)
            }
            Err(error) => {
                let partial_exists = remote_path_exists(&self.sftp, &self.partial_path).await;
                let final_exists = remote_path_exists(&self.sftp, &self.final_path).await;
                match (partial_exists, final_exists) {
                    (Ok(true), Ok(true)) => Err(ExplorerError::Conflict),
                    (Ok(false), Ok(true)) => {
                        self.finalized = true;
                        Err(ExplorerError::OutcomeUncertain(
                            "The server did not confirm remote transfer finalization. Refresh the destination before trying another action."
                                .to_owned(),
                        ))
                    }
                    (Ok(true), Ok(false)) => Err(map_remote_transfer_error(error)),
                    _ => Err(ExplorerError::OutcomeUncertain(
                        "The SSH connection ended while finalizing the remote transfer. Reconnect and refresh before trying another action."
                            .to_owned(),
                    )),
                }
            }
        }
    }

    pub(crate) fn preserve(mut self) -> String {
        self.preserved = true;
        self.final_path.clone()
    }

    pub(crate) async fn abandon(mut self) -> Result<(), ExplorerError> {
        self.file.take();
        if self.preserved {
            return Ok(());
        }
        if self.finalized {
            // A final path cannot be removed safely through SFTP because the
            // protocol exposes no stable identity for replacement checks.
            self.preserved = true;
            return Ok(());
        }
        match self.sftp.remove_file(self.partial_path.clone()).await {
            Ok(()) => {
                self.preserved = true;
                Ok(())
            }
            Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => {
                self.preserved = true;
                Ok(())
            }
            Err(error) => Err(map_remote_transfer_error(error)),
        }
    }
}

impl Drop for OwnedRemoteFileArtifact {
    fn drop(&mut self) {
        if self.finalized || self.preserved {
            return;
        }
        self.file.take();
        let sftp = self.sftp.clone();
        let partial_path = self.partial_path.clone();
        // Explicit `abandon` is authoritative and awaited. This best-effort
        // fallback handles early returns and panics while the runtime remains
        // alive; a disconnect may still leave a clearly named remote partial.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = sftp.remove_file(partial_path).await;
            });
        }
    }
}

async fn ensure_remote_path_absent(sftp: &SftpSession, path: &str) -> Result<(), ExplorerError> {
    match sftp.symlink_metadata(path.to_owned()).await {
        Ok(_) => Err(ExplorerError::Conflict),
        Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => Ok(()),
        Err(error) => Err(map_remote_transfer_error(error)),
    }
}

async fn remote_path_exists(sftp: &SftpSession, path: &str) -> Result<bool, ExplorerError> {
    match sftp.symlink_metadata(path.to_owned()).await {
        Ok(_) => Ok(true),
        Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => Ok(false),
        Err(error) => Err(map_remote_transfer_error(error)),
    }
}

fn map_remote_write_error(_error: TokioIoError) -> ExplorerError {
    ExplorerError::Offline(
        "The SSH connection was lost while writing the remote partial file.".to_owned(),
    )
}

fn map_remote_transfer_error(error: SftpError) -> ExplorerError {
    match error {
        SftpError::Status(status) => match status.status_code {
            StatusCode::NoSuchFile => ExplorerError::SourceChanged,
            StatusCode::PermissionDenied => ExplorerError::Io {
                message: "The remote server denied access to the transfer destination.".to_owned(),
                kind: std::io::ErrorKind::PermissionDenied,
            },
            _ => ExplorerError::Unexpected(
                "The remote server rejected the transfer request.".to_owned(),
            ),
        },
        SftpError::Timeout => {
            ExplorerError::Offline("The remote transfer request timed out.".to_owned())
        }
        SftpError::IO(_) | SftpError::UnexpectedBehavior(_) => ExplorerError::Offline(
            "The SSH connection was lost during the remote transfer.".to_owned(),
        ),
        SftpError::Limited(_) => ExplorerError::InvalidConfiguration(
            "The remote server's SFTP limits rejected the transfer request.".to_owned(),
        ),
        SftpError::UnexpectedPacket => ExplorerError::Unexpected(
            "The remote server returned an unexpected SFTP response.".to_owned(),
        ),
    }
}

fn remote_join(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{}/{name}", parent.trim_end_matches('/'))
    }
}
