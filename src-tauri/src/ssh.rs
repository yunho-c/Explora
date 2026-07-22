use std::{
    collections::HashMap,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        Arc, Mutex,
    },
};

const SESSION_CONNECTING: u8 = 0;
const SESSION_ACTIVE: u8 = 1;
const SESSION_DISCONNECTING: u8 = 2;
const SESSION_DISCONNECTED: u8 = 3;
const MAX_REMOTE_NAME_BYTES: usize = 1_024;
const MAX_REMOTE_DELETE_ENTRIES: usize = 100_000;
const MAX_REMOTE_DELETE_DEPTH: usize = 256;
const MAX_REMOTE_TRANSFER_PATH_BYTES: usize = 32 * 1024;
const MAX_KEEP_BOTH_ATTEMPTS: usize = 10_000;

use russh::{
    client::{self, AuthResult, DisconnectReason, Handle, KeyboardInteractiveAuthResponse},
    keys::{self, agent::client::AgentClient, HashAlg, PrivateKeyWithHashAlg, PublicKey},
    MethodKind,
};
use russh_sftp::{
    client::{error::Error as SftpError, SftpSession},
    protocol::{FileAttributes, StatusCode},
};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tokio::sync::{oneshot, Mutex as AsyncMutex, OwnedMutexGuard};
use uuid::Uuid;
use zeroize::Zeroize;

use crate::{
    filesystem::{
        BreadcrumbSegmentDto, DirectoryCapabilitiesDto, DirectoryListingEvent, DirectoryRefDto,
        EntryCapabilitiesDto, EntryRefDto, ExplorerError, FileEntrySummaryDto, LocationRole,
        LocationSummaryDto, CONNECTION_TIMEOUT, LISTING_BATCH_SIZE, PROMPT_TIMEOUT,
        SFTP_REQUEST_TIMEOUT_SECONDS, SSH_KEEPALIVE_INTERVAL, SSH_KEEPALIVE_MAX,
    },
    remote_transfer::OwnedRemoteTransferArtifact,
    ssh_targets::{location_id, ResolvedSshTarget, SshTargetSummaryDto},
};

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "event",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SshConnectionEventDto {
    State {
        state: &'static str,
    },
    HostKeyPrompt {
        prompt_id: String,
        host: String,
        port: u16,
        algorithm: String,
        fingerprint: String,
    },
    AuthenticationPrompt {
        prompt_id: String,
        kind: &'static str,
        title: String,
        instructions: String,
        fields: Vec<SshPromptFieldDto>,
    },
    Disconnected {
        target_id: String,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshPromptFieldDto {
    pub label: String,
    pub secret: bool,
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "response",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SshPromptResponseDto {
    Accept,
    Reject,
    Answers { answers: Vec<String> },
}

struct PendingPrompt {
    request_id: String,
    sender: oneshot::Sender<SshPromptResponseDto>,
}

#[derive(Default)]
struct PromptBroker {
    pending: Mutex<HashMap<String, PendingPrompt>>,
}

impl PromptBroker {
    async fn request(
        &self,
        request_id: &str,
        prompt_id: &str,
        event: SshConnectionEventDto,
        channel: &Channel<SshConnectionEventDto>,
    ) -> Result<SshPromptResponseDto, ExplorerError> {
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .insert(
                prompt_id.to_owned(),
                PendingPrompt {
                    request_id: request_id.to_owned(),
                    sender,
                },
            );
        if channel.send(event).is_err() {
            self.remove(prompt_id);
            return Err(ExplorerError::ChannelClosed);
        }
        let response = tokio::time::timeout(PROMPT_TIMEOUT, receiver).await;
        self.remove(prompt_id);
        match response {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(ExplorerError::Cancelled),
            Err(_) => Err(ExplorerError::AuthenticationFailed(
                "The SSH authentication prompt expired.".to_owned(),
            )),
        }
    }

    fn respond(
        &self,
        request_id: &str,
        prompt_id: &str,
        response: SshPromptResponseDto,
    ) -> Result<(), ExplorerError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        if pending
            .get(prompt_id)
            .is_none_or(|pending| pending.request_id != request_id)
        {
            return Err(ExplorerError::InvalidReference);
        }
        let pending = pending
            .remove(prompt_id)
            .ok_or(ExplorerError::InvalidReference)?;
        pending
            .sender
            .send(response)
            .map_err(|_| ExplorerError::InvalidReference)
    }

    fn cancel(&self, request_id: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            let ids = pending
                .iter()
                .filter(|(_, prompt)| prompt.request_id == request_id)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            for id in ids {
                if let Some(prompt) = pending.remove(&id) {
                    let _ = prompt.sender.send(SshPromptResponseDto::Reject);
                }
            }
        }
    }

    fn remove(&self, prompt_id: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(prompt_id);
        }
    }
}

struct HostKeyHandler {
    request_id: String,
    target: ResolvedSshTarget,
    prompts: Arc<PromptBroker>,
    events: Arc<Channel<SshConnectionEventDto>>,
    lifecycle: Arc<AtomicU8>,
}

impl client::Handler for HostKeyHandler {
    type Error = ExplorerError;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        let known = keys::known_hosts::known_host_keys_path(
            &self.target.host,
            self.target.port,
            &self.target.known_hosts_path,
        )
        .map_err(|error| {
            ExplorerError::HostKeyFailure(format!(
                "Explora could not check the SSH host key: {error}"
            ))
        })?;
        if known.iter().any(|(_, key)| key == server_public_key) {
            return Ok(true);
        }
        if known
            .iter()
            .any(|(_, key)| key.algorithm() == server_public_key.algorithm())
        {
            return Err(ExplorerError::HostKeyFailure(format!(
                "The SSH host key for {} has changed. Connection was blocked.",
                self.target.host
            )));
        }

        let prompt_id = Uuid::new_v4().to_string();
        let response = self
            .prompts
            .request(
                &self.request_id,
                &prompt_id,
                SshConnectionEventDto::HostKeyPrompt {
                    prompt_id: prompt_id.clone(),
                    host: self.target.host.clone(),
                    port: self.target.port,
                    algorithm: server_public_key.algorithm().to_string(),
                    fingerprint: server_public_key.fingerprint(HashAlg::Sha256).to_string(),
                },
                &self.events,
            )
            .await?;
        if !matches!(response, SshPromptResponseDto::Accept) {
            return Err(ExplorerError::Cancelled);
        }
        keys::known_hosts::learn_known_hosts_path(
            &self.target.host,
            self.target.port,
            server_public_key,
            &self.target.known_hosts_path,
        )
        .map_err(|error| {
            ExplorerError::HostKeyFailure(format!(
                "Explora could not record the accepted SSH host key: {error}"
            ))
        })?;
        Ok(true)
    }

    async fn disconnected(
        &mut self,
        reason: DisconnectReason<Self::Error>,
    ) -> Result<(), Self::Error> {
        let previous = self.lifecycle.swap(SESSION_DISCONNECTED, Ordering::SeqCst);
        if previous == SESSION_ACTIVE {
            let _ = self.events.send(SshConnectionEventDto::Disconnected {
                target_id: self.target.id.clone(),
                message: "The SSH connection was lost. Reconnect to continue browsing.".to_owned(),
            });
        }

        match reason {
            DisconnectReason::ReceivedDisconnect(_) => Ok(()),
            DisconnectReason::Error(error) => Err(error),
        }
    }
}

impl From<russh::Error> for ExplorerError {
    fn from(error: russh::Error) -> Self {
        ExplorerError::Unexpected(format!("The SSH protocol failed: {error}"))
    }
}

#[derive(Default)]
struct RemotePathRegistry {
    inner: Mutex<RemotePathRegistryInner>,
}

#[derive(Default)]
struct RemotePathRegistryInner {
    paths_by_id: HashMap<String, String>,
    ids_by_path: HashMap<String, String>,
    fingerprints_by_id: HashMap<String, RemoteEntryFingerprint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteEntryFingerprint {
    size: Option<u64>,
    uid: Option<u32>,
    gid: Option<u32>,
    permissions: Option<u32>,
    mtime: Option<u32>,
}

impl From<&FileAttributes> for RemoteEntryFingerprint {
    fn from(metadata: &FileAttributes) -> Self {
        Self {
            size: metadata.size,
            uid: metadata.uid,
            gid: metadata.gid,
            permissions: metadata.permissions,
            mtime: metadata.mtime,
        }
    }
}

impl RemotePathRegistry {
    fn register(&self, path: String) -> Result<String, ExplorerError> {
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

    fn resolve(&self, id: &str) -> Result<String, ExplorerError> {
        self.inner
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .paths_by_id
            .get(id)
            .cloned()
            .ok_or(ExplorerError::InvalidReference)
    }

    fn register_with_metadata(
        &self,
        path: String,
        metadata: &FileAttributes,
    ) -> Result<String, ExplorerError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let id = if let Some(id) = inner.ids_by_path.get(&path) {
            id.clone()
        } else {
            let id = Uuid::new_v4().to_string();
            inner.paths_by_id.insert(id.clone(), path.clone());
            inner.ids_by_path.insert(path, id.clone());
            id
        };
        inner
            .fingerprints_by_id
            .insert(id.clone(), RemoteEntryFingerprint::from(metadata));
        Ok(id)
    }

    fn resolve_for_mutation(
        &self,
        id: &str,
    ) -> Result<(String, RemoteEntryFingerprint), ExplorerError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let path = inner
            .paths_by_id
            .get(id)
            .cloned()
            .ok_or(ExplorerError::InvalidReference)?;
        let fingerprint = inner
            .fingerprints_by_id
            .get(id)
            .cloned()
            .ok_or(ExplorerError::InvalidReference)?;
        Ok((path, fingerprint))
    }

    fn rebase_subtree(&self, old_path: &str, new_path: &str) -> Result<Vec<String>, ExplorerError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let rebased = inner
            .paths_by_id
            .iter()
            .filter(|(_, path)| remote_is_same_or_descendant(path, old_path))
            .map(|(id, path)| {
                let suffix = path.strip_prefix(old_path).unwrap_or_default();
                (id.clone(), path.clone(), format!("{new_path}{suffix}"))
            })
            .collect::<Vec<_>>();
        let rebased_ids = rebased
            .iter()
            .map(|(id, _, _)| id.clone())
            .collect::<Vec<_>>();

        let stale_destination_ids = inner
            .ids_by_path
            .iter()
            .filter_map(|(path, id)| {
                (remote_is_same_or_descendant(path, new_path) && !rebased_ids.contains(id))
                    .then_some(id.clone())
            })
            .collect::<Vec<_>>();
        for id in stale_destination_ids {
            if let Some(path) = inner.paths_by_id.remove(&id) {
                inner.ids_by_path.remove(&path);
            }
            inner.fingerprints_by_id.remove(&id);
        }

        for (id, old_registered_path, new_registered_path) in rebased {
            inner.ids_by_path.remove(&old_registered_path);
            inner
                .paths_by_id
                .insert(id.clone(), new_registered_path.clone());
            inner.ids_by_path.insert(new_registered_path, id);
        }
        Ok(rebased_ids)
    }

    fn invalidate_subtree(&self, removed_path: &str) -> Result<Vec<String>, ExplorerError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let removed_ids = inner
            .paths_by_id
            .iter()
            .filter_map(|(id, path)| {
                remote_is_same_or_descendant(path, removed_path).then_some(id.clone())
            })
            .collect::<Vec<_>>();
        for id in &removed_ids {
            if let Some(path) = inner.paths_by_id.remove(id) {
                inner.ids_by_path.remove(&path);
            }
            inner.fingerprints_by_id.remove(id);
        }
        Ok(removed_ids)
    }
}

struct SshSession {
    target: ResolvedSshTarget,
    location_id: String,
    root: DirectoryRefDto,
    paths: Arc<RemotePathRegistry>,
    handle: AsyncMutex<Handle<HostKeyHandler>>,
    sftp: Arc<SftpSession>,
    mutation_guard: Arc<AsyncMutex<()>>,
    lifecycle: Arc<AtomicU8>,
    events: Arc<Channel<SshConnectionEventDto>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteMoveConflictPolicy {
    Fail,
    KeepBoth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovedRemoteEntry {
    pub entry: FileEntrySummaryDto,
    pub source_parent: DirectoryRefDto,
    pub destination: DirectoryRefDto,
    pub rebased_entry_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedRemoteEntry {
    pub reference: EntryRefDto,
    pub name: String,
    pub invalidated_entry_ids: Vec<String>,
}

pub(crate) struct PreparedRemoteDestination {
    session: Arc<SshSession>,
    artifact: Option<OwnedRemoteTransferArtifact>,
    pub(crate) destination: DirectoryRefDto,
    _mutation_guard: OwnedMutexGuard<()>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemoteTransferDestinationKind {
    File,
    Directory,
    Symlink { target: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteTransferEntryKind {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteTransferPlanEntry {
    path: String,
    pub(crate) relative_path: String,
    pub(crate) kind: RemoteTransferEntryKind,
    fingerprint: RemoteEntryFingerprint,
    pub(crate) link_target: Option<String>,
    pub(crate) permissions: Option<u32>,
    pub(crate) len: u64,
}

pub(crate) struct PreparedRemoteTransfer {
    session: Arc<SshSession>,
    entries: Vec<RemoteTransferPlanEntry>,
    pub(crate) source: EntryRefDto,
    pub(crate) name: String,
    pub(crate) source_parent: DirectoryRefDto,
    pub(crate) total_bytes: u64,
    pub(crate) permissions: Option<u32>,
    _mutation_guard: OwnedMutexGuard<()>,
}

impl PreparedRemoteTransfer {
    pub(crate) fn connection_error(&self, message: &str) -> ExplorerError {
        self.session.mark_offline();
        ExplorerError::Offline(message.to_owned())
    }

    pub(crate) fn root_is_file(&self) -> bool {
        self.entries
            .first()
            .is_some_and(|entry| entry.kind == RemoteTransferEntryKind::File)
    }

    pub(crate) fn root_is_symlink(&self) -> bool {
        self.entries
            .first()
            .is_some_and(|entry| entry.kind == RemoteTransferEntryKind::Symlink)
    }

    pub(crate) fn root_is_directory(&self) -> bool {
        self.entries
            .first()
            .is_some_and(|entry| entry.kind == RemoteTransferEntryKind::Directory)
    }

    pub(crate) fn entries(&self) -> &[RemoteTransferPlanEntry] {
        &self.entries
    }

    pub(crate) fn root_link_target(&self) -> Option<&str> {
        self.entries.first()?.link_target.as_deref()
    }

    fn root(&self) -> Result<&RemoteTransferPlanEntry, ExplorerError> {
        self.entries.first().ok_or(ExplorerError::StateUnavailable)
    }

    pub(crate) async fn open_for_read(
        &self,
    ) -> Result<russh_sftp::client::fs::File, ExplorerError> {
        let root = self.root()?;
        if root.kind != RemoteTransferEntryKind::File {
            return Err(ExplorerError::Unsupported(
                "Only regular-file transfer entries can be opened for streaming.".to_owned(),
            ));
        }
        let result = self
            .session
            .sftp
            .open(root.path.clone())
            .await
            .map_err(map_sftp_error);
        finish_remote_result(&self.session, result)
    }

    pub(crate) async fn open_entry_for_read(
        &self,
        relative_path: &str,
    ) -> Result<russh_sftp::client::fs::File, ExplorerError> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.relative_path == relative_path)
            .ok_or(ExplorerError::InvalidReference)?;
        if entry.kind != RemoteTransferEntryKind::File {
            return Err(ExplorerError::Unsupported(
                "Only regular-file transfer entries can be opened for streaming.".to_owned(),
            ));
        }
        let result = self
            .session
            .sftp
            .open(entry.path.clone())
            .await
            .map_err(map_sftp_error);
        finish_remote_result(&self.session, result)
    }

    pub(crate) async fn revalidate(&self) -> Result<(), ExplorerError> {
        let root = self.root()?;
        let result = self.session.revalidate_source(&self.source).await;
        let (path, metadata) = finish_remote_result(&self.session, result)?;
        if path != root.path || RemoteEntryFingerprint::from(&metadata) != root.fingerprint {
            return Err(ExplorerError::SourceChanged);
        }
        let result = plan_remote_transfer(&self.session, &path, &AtomicBool::new(false)).await;
        let current = finish_remote_result(&self.session, result)?;
        if current != self.entries {
            return Err(ExplorerError::SourceChanged);
        }
        Ok(())
    }

    pub(crate) async fn remove_after_verified_transfer(self) -> Result<Vec<String>, ExplorerError> {
        let root_path = self.root()?.path.clone();
        let mut removed_any = false;
        for entry in self.entries.iter().rev() {
            if entry.kind != RemoteTransferEntryKind::Directory {
                let metadata = match self.session.sftp.symlink_metadata(entry.path.clone()).await {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        return Err(remote_source_removal_error(
                            &self.session,
                            error,
                            removed_any,
                        ));
                    }
                };
                if RemoteEntryFingerprint::from(&metadata) != entry.fingerprint {
                    return Err(if removed_any {
                        ExplorerError::PartialCompletion(
                            "The verified destination was kept, but the remote source changed after cleanup began. Refresh the source location before continuing."
                                .to_owned(),
                        )
                    } else {
                        ExplorerError::SourceChanged
                    });
                }
                if entry.kind == RemoteTransferEntryKind::Symlink {
                    let target = match self.session.sftp.read_link(entry.path.clone()).await {
                        Ok(target) => target,
                        Err(error) => {
                            return Err(remote_source_removal_error(
                                &self.session,
                                error,
                                removed_any,
                            ));
                        }
                    };
                    if entry.link_target.as_deref() != Some(target.as_str()) {
                        return Err(if removed_any {
                            ExplorerError::PartialCompletion(
                                "The verified destination was kept, but a remote symbolic link changed after cleanup began. Refresh the source location before continuing."
                                    .to_owned(),
                            )
                        } else {
                            ExplorerError::SourceChanged
                        });
                    }
                }
            }

            let removal = match entry.kind {
                RemoteTransferEntryKind::Directory => {
                    self.session.sftp.remove_dir(entry.path.clone()).await
                }
                RemoteTransferEntryKind::File | RemoteTransferEntryKind::Symlink => {
                    self.session.sftp.remove_file(entry.path.clone()).await
                }
            };
            if let Err(error) = removal {
                return Err(remote_source_removal_error(
                    &self.session,
                    error,
                    removed_any,
                ));
            }
            removed_any = true;
        }
        self.session.paths.invalidate_subtree(&root_path)
    }
}

fn remote_source_removal_error(
    session: &SshSession,
    error: SftpError,
    removed_any: bool,
) -> ExplorerError {
    if is_sftp_connectivity_error(&error) {
        session.mark_offline();
        return ExplorerError::OutcomeUncertain(
            "The verified destination was kept, but the SSH connection ended while removing the remote source. Reconnect and refresh before retrying."
                .to_owned(),
        );
    }
    let detail = if removed_any {
        "The verified destination was kept, but the remote source was only partially removed."
    } else {
        "The verified destination was kept, but the remote source could not be removed."
    };
    ExplorerError::PartialCompletion(detail.to_owned())
}

impl PreparedRemoteDestination {
    pub(crate) fn connection_error(&self, message: &str) -> ExplorerError {
        self.session.mark_offline();
        ExplorerError::Offline(message.to_owned())
    }

    pub(crate) fn partial_path(&self) -> Result<&str, ExplorerError> {
        self.artifact
            .as_ref()
            .map(OwnedRemoteTransferArtifact::partial_path)
            .ok_or(ExplorerError::StateUnavailable)
    }

    pub(crate) async fn write_chunk(&mut self, chunk: &[u8]) -> Result<u64, ExplorerError> {
        let result = self
            .artifact
            .as_mut()
            .ok_or(ExplorerError::StateUnavailable)?
            .write_chunk(chunk)
            .await;
        finish_remote_result(&self.session, result)
    }

    pub(crate) fn bytes_written(&self) -> Result<u64, ExplorerError> {
        self.artifact
            .as_ref()
            .map(OwnedRemoteTransferArtifact::bytes_written)
            .ok_or(ExplorerError::StateUnavailable)
    }

    pub(crate) async fn close_for_verification(&mut self) -> Result<(), ExplorerError> {
        let result = self
            .artifact
            .as_mut()
            .ok_or(ExplorerError::StateUnavailable)?
            .close_for_verification()
            .await;
        finish_remote_result(&self.session, result)
    }

    pub(crate) async fn open_partial_for_verification(
        &self,
    ) -> Result<russh_sftp::client::fs::File, ExplorerError> {
        let result = self
            .session
            .sftp
            .open(self.partial_path()?.to_owned())
            .await
            .map_err(map_sftp_error);
        finish_remote_result(&self.session, result)
    }

    pub(crate) async fn read_partial_link(&self) -> Result<String, ExplorerError> {
        let result = self
            .session
            .sftp
            .read_link(self.partial_path()?.to_owned())
            .await
            .map_err(map_sftp_error);
        finish_remote_result(&self.session, result)
    }

    pub(crate) async fn create_directory_entry(
        &mut self,
        relative_path: &str,
    ) -> Result<(), ExplorerError> {
        let result = self
            .artifact
            .as_mut()
            .ok_or(ExplorerError::StateUnavailable)?
            .create_directory_entry(relative_path)
            .await;
        finish_remote_result(&self.session, result)
    }

    pub(crate) async fn create_symlink_entry(
        &mut self,
        relative_path: &str,
        target: &str,
    ) -> Result<(), ExplorerError> {
        let result = self
            .artifact
            .as_mut()
            .ok_or(ExplorerError::StateUnavailable)?
            .create_symlink_entry(relative_path, target)
            .await;
        finish_remote_result(&self.session, result)
    }

    pub(crate) async fn begin_file_entry(
        &mut self,
        relative_path: &str,
    ) -> Result<(), ExplorerError> {
        let result = self
            .artifact
            .as_mut()
            .ok_or(ExplorerError::StateUnavailable)?
            .begin_file_entry(relative_path)
            .await;
        finish_remote_result(&self.session, result)
    }

    pub(crate) async fn open_entry_for_verification(
        &self,
        relative_path: &str,
    ) -> Result<russh_sftp::client::fs::File, ExplorerError> {
        let result = self
            .artifact
            .as_ref()
            .ok_or(ExplorerError::StateUnavailable)?
            .open_entry_for_verification(relative_path)
            .await;
        finish_remote_result(&self.session, result)
    }

    pub(crate) async fn entry_metadata(
        &self,
        relative_path: &str,
    ) -> Result<FileAttributes, ExplorerError> {
        let result = self
            .artifact
            .as_ref()
            .ok_or(ExplorerError::StateUnavailable)?
            .entry_metadata(relative_path)
            .await;
        finish_remote_result(&self.session, result)
    }

    pub(crate) async fn read_link_entry(
        &self,
        relative_path: &str,
    ) -> Result<String, ExplorerError> {
        let result = self
            .artifact
            .as_ref()
            .ok_or(ExplorerError::StateUnavailable)?
            .read_link_entry(relative_path)
            .await;
        finish_remote_result(&self.session, result)
    }

    pub(crate) async fn set_entry_permissions(
        &self,
        relative_path: &str,
        permissions: Option<u32>,
    ) -> Result<(), ExplorerError> {
        let result = self
            .artifact
            .as_ref()
            .ok_or(ExplorerError::StateUnavailable)?
            .set_entry_permissions(relative_path, permissions)
            .await;
        finish_remote_result(&self.session, result)
    }

    pub(crate) async fn finalize(mut self) -> Result<FileEntrySummaryDto, ExplorerError> {
        let final_path = match self
            .artifact
            .as_mut()
            .ok_or(ExplorerError::StateUnavailable)?
            .finalize_no_replace()
            .await
        {
            Ok(path) => path.to_owned(),
            Err(error) => {
                if let Some(artifact) = self.artifact.take() {
                    let _ = artifact.abandon().await;
                }
                return finish_remote_result(&self.session, Err(error));
            }
        };
        let artifact = self
            .artifact
            .take()
            .ok_or(ExplorerError::StateUnavailable)?;
        artifact.preserve();
        let result = self.session.summary_for_registered_path(&final_path).await;
        finish_remote_result(&self.session, result)
    }

    pub(crate) async fn abandon(mut self) -> Result<(), ExplorerError> {
        let result = self
            .artifact
            .take()
            .ok_or(ExplorerError::StateUnavailable)?
            .abandon()
            .await;
        finish_remote_result(&self.session, result)
    }
}

#[derive(Debug)]
struct RemoteDeleteItem {
    path: String,
    is_directory: bool,
}

impl SshSession {
    fn mark_offline(&self) {
        if self
            .lifecycle
            .compare_exchange(
                SESSION_ACTIVE,
                SESSION_DISCONNECTED,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
        {
            let _ = self.events.send(SshConnectionEventDto::Disconnected {
                target_id: self.target.id.clone(),
                message: "The SSH connection was lost. Reconnect to continue browsing.".to_owned(),
            });
        }
    }

    fn location(&self) -> LocationSummaryDto {
        LocationSummaryDto {
            id: self.location_id.clone(),
            name: self.target.name.clone(),
            kind: "ssh",
            role: LocationRole::Ssh,
            status: "connected",
            display_path: self.root.display_path.clone(),
            detail: format!("SSH · {}", endpoint(&self.target)),
            root: self.root.clone(),
        }
    }

    fn directory_ref(
        &self,
        path: &str,
        name: Option<String>,
    ) -> Result<DirectoryRefDto, ExplorerError> {
        self.directory_ref_with_capabilities(path, name, DirectoryCapabilitiesDto::SFTP)
    }

    fn directory_ref_with_capabilities(
        &self,
        path: &str,
        name: Option<String>,
        capabilities: DirectoryCapabilitiesDto,
    ) -> Result<DirectoryRefDto, ExplorerError> {
        Ok(DirectoryRefDto {
            id: self.paths.register(path.to_owned())?,
            location_id: self.location_id.clone(),
            name: name.unwrap_or_else(|| remote_name(path)),
            display_path: format!("{}:{path}", self.target.name),
            capabilities,
        })
    }

    async fn list_directory<F>(
        &self,
        directory_id: &str,
        cancelled: &AtomicBool,
        mut emit: F,
    ) -> Result<(), ExplorerError>
    where
        F: FnMut(DirectoryListingEvent) -> Result<(), ExplorerError>,
    {
        ensure_not_cancelled(cancelled)?;
        let path = self.paths.resolve(directory_id)?;
        let directory_metadata = tokio::select! {
            metadata = self.sftp.symlink_metadata(path.clone()) => metadata.map_err(map_sftp_error)?,
            () = wait_for_cancellation(cancelled) => return Err(ExplorerError::Cancelled),
        };
        self.paths
            .register_with_metadata(path.clone(), &directory_metadata)?;
        let directory = self.directory_ref_with_capabilities(
            &path,
            None,
            if directory_metadata.is_symlink() {
                DirectoryCapabilitiesDto::READ_ONLY
            } else {
                DirectoryCapabilitiesDto::SFTP
            },
        )?;
        let parent = remote_parent(&path)
            .map(|parent| self.directory_ref(&parent, None))
            .transpose()?;
        let breadcrumbs = remote_breadcrumbs(self, &path)?;
        emit(DirectoryListingEvent::Started {
            directory: Box::new(directory),
            parent: parent.map(Box::new),
            breadcrumbs,
        })?;

        let read_dir = tokio::select! {
            result = self.sftp.read_dir(path.clone()) => result.map_err(map_sftp_error)?,
            () = wait_for_cancellation(cancelled) => return Err(ExplorerError::Cancelled),
        };
        let mut batch = Vec::with_capacity(LISTING_BATCH_SIZE);
        let mut first_batch = true;
        let mut skipped_entries = 0;
        for entry in read_dir {
            ensure_not_cancelled(cancelled)?;
            let name = entry.file_name();
            if validate_remote_name(&name).is_err() {
                skipped_entries += 1;
                continue;
            }
            let entry_path = remote_join(&path, &name);
            let metadata = entry.metadata();
            let is_symlink = metadata.is_symlink();
            let is_directory = if is_symlink {
                tokio::select! {
                    metadata = self.sftp.metadata(entry_path.clone()) => {
                        metadata.map(|metadata| metadata.is_dir()).unwrap_or(false)
                    }
                    () = wait_for_cancellation(cancelled) => {
                        return Err(ExplorerError::Cancelled);
                    }
                }
            } else {
                metadata.is_dir()
            };
            let kind = if is_symlink {
                "symlink"
            } else if is_directory {
                "directory"
            } else if metadata.is_regular() {
                "file"
            } else {
                "other"
            };
            let entry_id = self
                .paths
                .register_with_metadata(entry_path.clone(), &metadata)?;
            let directory = is_directory
                .then(|| {
                    self.directory_ref_with_capabilities(
                        &entry_path,
                        Some(name.clone()),
                        if is_symlink {
                            DirectoryCapabilitiesDto::READ_ONLY
                        } else {
                            DirectoryCapabilitiesDto::SFTP
                        },
                    )
                })
                .transpose()?;
            batch.push(FileEntrySummaryDto {
                reference: EntryRefDto {
                    id: entry_id,
                    location_id: self.location_id.clone(),
                },
                name: name.clone(),
                kind,
                content_kind: content_kind(&name, is_directory),
                size: (!is_directory).then(|| metadata.size.unwrap_or(0).to_string()),
                modified_at: metadata.mtime.map(|seconds| u64::from(seconds) * 1000),
                display_path: format!("{}:{entry_path}", self.target.name),
                directory,
                detail: None,
                capabilities: EntryCapabilitiesDto::SFTP,
            });
            if batch.len() == LISTING_BATCH_SIZE {
                emit(DirectoryListingEvent::Entries {
                    entries: std::mem::take(&mut batch),
                    replace: first_batch,
                })?;
                first_batch = false;
            }
        }
        if !batch.is_empty() {
            emit(DirectoryListingEvent::Entries {
                entries: batch,
                replace: first_batch,
            })?;
        }
        emit(DirectoryListingEvent::Complete { skipped_entries })
    }

    async fn rename_entry(
        &self,
        source: &EntryRefDto,
        new_name: &str,
        cancelled: &AtomicBool,
    ) -> Result<FileEntrySummaryDto, ExplorerError> {
        validate_remote_name(new_name)?;
        let _guard = self.mutation_guard.lock().await;
        ensure_not_cancelled(cancelled)?;
        let (source_path, _) = self.revalidate_source(source).await?;
        let parent_path = remote_parent(&source_path).ok_or(ExplorerError::InvalidReference)?;
        let destination_path = remote_join(&parent_path, new_name);
        if destination_path == source_path {
            return self.summary_for_registered_path(&source_path).await;
        }
        self.ensure_remote_destination_absent(&destination_path)
            .await?;
        ensure_not_cancelled(cancelled)?;
        self.rename_no_replace(&source_path, &destination_path)
            .await?;
        self.paths.rebase_subtree(&source_path, &destination_path)?;
        self.summary_for_registered_path(&destination_path).await
    }

    async fn move_entry(
        &self,
        source: &EntryRefDto,
        destination: &DirectoryRefDto,
        conflict_policy: RemoteMoveConflictPolicy,
        cancelled: &AtomicBool,
    ) -> Result<MovedRemoteEntry, ExplorerError> {
        if destination.location_id != self.location_id {
            return Err(ExplorerError::DestinationUnavailable(
                "Moving between locations requires a verified transfer.".to_owned(),
            ));
        }
        let _guard = self.mutation_guard.lock().await;
        ensure_not_cancelled(cancelled)?;
        let (source_path, source_metadata) = self.revalidate_source(source).await?;
        let source_parent_path =
            remote_parent(&source_path).ok_or(ExplorerError::InvalidReference)?;
        let destination_path = self.revalidate_directory(destination).await?;
        if source_parent_path == destination_path {
            let entry = self.summary_for_registered_path(&source_path).await?;
            return Ok(MovedRemoteEntry {
                entry,
                source_parent: self.directory_ref(&source_parent_path, None)?,
                destination: self.directory_ref(&destination_path, None)?,
                rebased_entry_ids: vec![source.id.clone()],
            });
        }
        if source_metadata.is_dir() && remote_is_same_or_descendant(&destination_path, &source_path)
        {
            return Err(ExplorerError::DestinationUnavailable(
                "A folder cannot be moved into itself or one of its subfolders.".to_owned(),
            ));
        }

        let original_name = remote_name(&source_path);
        let destination_entry_path = match conflict_policy {
            RemoteMoveConflictPolicy::Fail => {
                let path = remote_join(&destination_path, &original_name);
                self.ensure_remote_destination_absent(&path).await?;
                ensure_not_cancelled(cancelled)?;
                self.rename_no_replace(&source_path, &path).await?;
                path
            }
            RemoteMoveConflictPolicy::KeepBoth => {
                let mut moved_path = None;
                for attempt in 1..=MAX_KEEP_BOTH_ATTEMPTS {
                    let candidate =
                        remote_keep_both_name(&original_name, source_metadata.is_dir(), attempt)?;
                    let path = remote_join(&destination_path, &candidate);
                    match self.ensure_remote_destination_absent(&path).await {
                        Ok(()) => {}
                        Err(ExplorerError::Conflict) => continue,
                        Err(error) => return Err(error),
                    }
                    ensure_not_cancelled(cancelled)?;
                    match self.rename_no_replace(&source_path, &path).await {
                        Ok(()) => {
                            moved_path = Some(path);
                            break;
                        }
                        Err(ExplorerError::Conflict) => continue,
                        Err(error) => return Err(error),
                    }
                }
                moved_path.ok_or(ExplorerError::Conflict)?
            }
        };
        let rebased_entry_ids = self
            .paths
            .rebase_subtree(&source_path, &destination_entry_path)?;
        let entry = self
            .summary_for_registered_path(&destination_entry_path)
            .await?;
        Ok(MovedRemoteEntry {
            entry,
            source_parent: self.directory_ref(&source_parent_path, None)?,
            destination: self.directory_ref(&destination_path, None)?,
            rebased_entry_ids,
        })
    }

    async fn describe_move_conflict(
        &self,
        source: &EntryRefDto,
        destination: &DirectoryRefDto,
    ) -> Result<(String, String), ExplorerError> {
        let _guard = self.mutation_guard.lock().await;
        let (source_path, _) = self.revalidate_source(source).await?;
        let destination_path = self.revalidate_directory(destination).await?;
        Ok((remote_name(&source_path), remote_name(&destination_path)))
    }

    async fn describe_operation_target(
        &self,
        source: &EntryRefDto,
    ) -> Result<(String, String), ExplorerError> {
        let _guard = self.mutation_guard.lock().await;
        let (source_path, _) = self.revalidate_source(source).await?;
        Ok((
            remote_name(&source_path),
            format!("{} ({})", self.target.name, endpoint(&self.target)),
        ))
    }

    async fn permanently_delete_entry<F>(
        &self,
        source: &EntryRefDto,
        cancelled: &AtomicBool,
        mut on_progress: F,
    ) -> Result<RemovedRemoteEntry, ExplorerError>
    where
        F: FnMut(u64, u64) -> Result<(), ExplorerError>,
    {
        let _guard = self.mutation_guard.lock().await;
        ensure_not_cancelled(cancelled)?;
        let (source_path, _) = self.revalidate_source(source).await?;
        let name = remote_name(&source_path);
        let plan = self.plan_remote_delete(&source_path, cancelled).await?;
        let total_items = u64::try_from(plan.len()).map_err(|_| {
            ExplorerError::InvalidConfiguration(
                "The remote deletion contains too many entries.".to_owned(),
            )
        })?;
        on_progress(0, total_items)?;
        ensure_not_cancelled(cancelled)?;

        let mut completed = 0_u64;
        let mut invalidated_entry_ids = Vec::new();
        for item in plan {
            let result = if item.is_directory {
                self.sftp.remove_dir(item.path.clone()).await
            } else {
                self.sftp.remove_file(item.path.clone()).await
            };
            if let Err(error) = result {
                return Err(self.remote_delete_error(error, completed));
            }
            completed = completed.saturating_add(1);
            if let Ok(ids) = self.paths.invalidate_subtree(&item.path) {
                invalidated_entry_ids.extend(ids);
            }
            let _ = on_progress(completed, total_items);
        }

        invalidated_entry_ids.extend(self.paths.invalidate_subtree(&source_path)?);
        if !invalidated_entry_ids.iter().any(|id| id == &source.id) {
            invalidated_entry_ids.push(source.id.clone());
        }
        invalidated_entry_ids.sort();
        invalidated_entry_ids.dedup();
        Ok(RemovedRemoteEntry {
            reference: source.clone(),
            name,
            invalidated_entry_ids,
        })
    }

    async fn revalidate_source(
        &self,
        source: &EntryRefDto,
    ) -> Result<(String, FileAttributes), ExplorerError> {
        if source.location_id != self.location_id || source.id == self.root.id {
            return Err(ExplorerError::InvalidReference);
        }
        let (path, expected) = self.paths.resolve_for_mutation(&source.id)?;
        let metadata = match self.sftp.symlink_metadata(path.clone()).await {
            Ok(metadata) => metadata,
            Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => {
                return Err(ExplorerError::SourceChanged);
            }
            Err(error) => return Err(map_sftp_error(error)),
        };
        if RemoteEntryFingerprint::from(&metadata) != expected {
            return Err(ExplorerError::SourceChanged);
        }
        Ok((path, metadata))
    }

    async fn revalidate_directory(
        &self,
        destination: &DirectoryRefDto,
    ) -> Result<String, ExplorerError> {
        if destination.location_id != self.location_id {
            return Err(ExplorerError::InvalidReference);
        }
        let (path, expected) = self.paths.resolve_for_mutation(&destination.id)?;
        let metadata = self
            .sftp
            .symlink_metadata(path.clone())
            .await
            .map_err(map_sftp_error)?;
        if RemoteEntryFingerprint::from(&metadata) != expected {
            return Err(ExplorerError::SourceChanged);
        }
        if metadata.is_symlink() || !metadata.is_dir() {
            return Err(ExplorerError::DestinationUnavailable(
                "The destination is not an available folder.".to_owned(),
            ));
        }
        Ok(path)
    }

    async fn ensure_remote_destination_absent(&self, path: &str) -> Result<(), ExplorerError> {
        match self.sftp.symlink_metadata(path.to_owned()).await {
            Ok(_) => Err(ExplorerError::Conflict),
            Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => {
                Ok(())
            }
            Err(error) => Err(map_sftp_error(error)),
        }
    }

    async fn rename_no_replace(
        &self,
        source_path: &str,
        destination_path: &str,
    ) -> Result<(), ExplorerError> {
        match self
            .sftp
            .rename(source_path.to_owned(), destination_path.to_owned())
            .await
        {
            Ok(()) => Ok(()),
            Err(error) if is_sftp_connectivity_error(&error) => {
                self.mark_offline();
                Err(ExplorerError::OutcomeUncertain(
                    "The SSH connection was lost during the remote rename. Reconnect and refresh before trying another action.".to_owned(),
                ))
            }
            Err(error) => {
                let source_exists = self
                    .sftp
                    .symlink_metadata(source_path.to_owned())
                    .await
                    .is_ok();
                let destination_exists = self
                    .sftp
                    .symlink_metadata(destination_path.to_owned())
                    .await
                    .is_ok();
                if source_exists && destination_exists {
                    Err(ExplorerError::Conflict)
                } else if !source_exists {
                    Err(ExplorerError::OutcomeUncertain(
                        "The server did not confirm the remote rename. Refresh the location before trying another action.".to_owned(),
                    ))
                } else {
                    Err(map_sftp_error(error))
                }
            }
        }
    }

    async fn summary_for_registered_path(
        &self,
        path: &str,
    ) -> Result<FileEntrySummaryDto, ExplorerError> {
        let metadata = self
            .sftp
            .symlink_metadata(path.to_owned())
            .await
            .map_err(map_sftp_error)?;
        let is_symlink = metadata.is_symlink();
        let is_directory = if is_symlink {
            self.sftp
                .metadata(path.to_owned())
                .await
                .map(|metadata| metadata.is_dir())
                .unwrap_or(false)
        } else {
            metadata.is_dir()
        };
        let name = remote_name(path);
        let id = self
            .paths
            .register_with_metadata(path.to_owned(), &metadata)?;
        Ok(FileEntrySummaryDto {
            reference: EntryRefDto {
                id,
                location_id: self.location_id.clone(),
            },
            name: name.clone(),
            kind: if is_symlink {
                "symlink"
            } else if is_directory {
                "directory"
            } else if metadata.is_regular() {
                "file"
            } else {
                "other"
            },
            content_kind: content_kind(&name, is_directory),
            size: (!is_directory).then(|| metadata.size.unwrap_or(0).to_string()),
            modified_at: metadata.mtime.map(|seconds| u64::from(seconds) * 1000),
            display_path: format!("{}:{path}", self.target.name),
            directory: is_directory
                .then(|| {
                    self.directory_ref_with_capabilities(
                        path,
                        Some(name),
                        if is_symlink {
                            DirectoryCapabilitiesDto::READ_ONLY
                        } else {
                            DirectoryCapabilitiesDto::SFTP
                        },
                    )
                })
                .transpose()?,
            detail: None,
            capabilities: EntryCapabilitiesDto::SFTP,
        })
    }

    async fn plan_remote_delete(
        &self,
        source_path: &str,
        cancelled: &AtomicBool,
    ) -> Result<Vec<RemoteDeleteItem>, ExplorerError> {
        let mut plan = Vec::new();
        let mut stack = vec![(source_path.to_owned(), 0_usize, false)];
        while let Some((path, depth, visited)) = stack.pop() {
            ensure_not_cancelled(cancelled)?;
            if depth > MAX_REMOTE_DELETE_DEPTH
                || plan.len() + stack.len() >= MAX_REMOTE_DELETE_ENTRIES
            {
                return Err(ExplorerError::InvalidConfiguration(
                    "The remote directory is too large or deeply nested to delete safely in one operation.".to_owned(),
                ));
            }
            let metadata = self
                .sftp
                .symlink_metadata(path.clone())
                .await
                .map_err(map_sftp_error)?;
            if metadata.is_dir() && !metadata.is_symlink() {
                if visited {
                    plan.push(RemoteDeleteItem {
                        path,
                        is_directory: true,
                    });
                } else {
                    stack.push((path.clone(), depth, true));
                    let entries = self
                        .sftp
                        .read_dir(path.clone())
                        .await
                        .map_err(map_sftp_error)?;
                    for entry in entries.collect::<Vec<_>>().into_iter().rev() {
                        let name = entry.file_name();
                        validate_remote_name(&name).map_err(|_| {
                            ExplorerError::Unexpected(
                                "The SFTP server returned an invalid directory entry name."
                                    .to_owned(),
                            )
                        })?;
                        stack.push((remote_join(&path, &name), depth.saturating_add(1), false));
                    }
                }
            } else {
                plan.push(RemoteDeleteItem {
                    path,
                    is_directory: false,
                });
            }
        }
        if plan.is_empty() {
            return Err(ExplorerError::SourceChanged);
        }
        Ok(plan)
    }

    fn remote_delete_error(&self, error: SftpError, completed: u64) -> ExplorerError {
        if is_sftp_connectivity_error(&error) {
            self.mark_offline();
            if completed == 0 {
                ExplorerError::OutcomeUncertain(
                    "The SSH connection was lost during permanent deletion. Reconnect and refresh to inspect the actual state.".to_owned(),
                )
            } else {
                ExplorerError::PartialCompletion(
                    "The SSH connection was lost after part of the remote deletion completed. Reconnect and refresh; Explora will not retry automatically.".to_owned(),
                )
            }
        } else if completed > 0 {
            ExplorerError::PartialCompletion(
                "Part of the remote directory was deleted before the server rejected the operation. Refresh to inspect what remains.".to_owned(),
            )
        } else {
            map_sftp_error(error)
        }
    }
}

pub struct SshConnectionManager {
    sessions: Mutex<HashMap<String, Arc<SshSession>>>,
    path_registries: Mutex<HashMap<String, Arc<RemotePathRegistry>>>,
    connections: Mutex<HashMap<String, ActiveConnection>>,
    prompts: Arc<PromptBroker>,
}

struct ActiveConnection {
    target_id: String,
    cancelled: Arc<AtomicBool>,
}

impl Default for SshConnectionManager {
    fn default() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            path_registries: Mutex::new(HashMap::new()),
            connections: Mutex::new(HashMap::new()),
            prompts: Arc::new(PromptBroker::default()),
        }
    }
}

impl SshConnectionManager {
    pub fn locations(&self) -> Vec<LocationSummaryDto> {
        self.sessions
            .lock()
            .map(|sessions| {
                sessions
                    .values()
                    .filter(|session| session.lifecycle.load(Ordering::SeqCst) == SESSION_ACTIVE)
                    .map(|session| session.location())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn apply_statuses(&self, targets: &mut [SshTargetSummaryDto]) {
        let sessions = self.sessions.lock().ok();
        let connections = self.connections.lock().ok();
        for target in targets {
            if let Some(session) = sessions
                .as_ref()
                .and_then(|sessions| sessions.get(&target.id))
                .filter(|session| session.lifecycle.load(Ordering::SeqCst) == SESSION_ACTIVE)
            {
                target.status = "connected";
                target.connected_location_id = Some(session.location_id.clone());
            } else if connections.as_ref().is_some_and(|connections| {
                connections
                    .values()
                    .any(|connection| connection.target_id == target.id)
            }) {
                target.status = "connecting";
            }
        }
    }

    pub async fn connect(
        &self,
        target: ResolvedSshTarget,
        request_id: String,
        events: Channel<SshConnectionEventDto>,
    ) -> Result<LocationSummaryDto, ExplorerError> {
        if request_id.is_empty() || request_id.len() > 128 {
            return Err(ExplorerError::InvalidReference);
        }
        {
            let mut sessions = self
                .sessions
                .lock()
                .map_err(|_| ExplorerError::StateUnavailable)?;
            if let Some(session) = sessions.get(&target.id) {
                if session.lifecycle.load(Ordering::SeqCst) == SESSION_ACTIVE {
                    return Ok(session.location());
                }
                sessions.remove(&target.id);
            }
        }
        let cancellation = Arc::new(AtomicBool::new(false));
        {
            let mut connections = self
                .connections
                .lock()
                .map_err(|_| ExplorerError::StateUnavailable)?;
            if connections.contains_key(&request_id)
                || connections
                    .values()
                    .any(|connection| connection.target_id == target.id)
            {
                return Err(ExplorerError::InvalidReference);
            }
            connections.insert(
                request_id.clone(),
                ActiveConnection {
                    target_id: target.id.clone(),
                    cancelled: cancellation.clone(),
                },
            );
        }
        let events = Arc::new(events);
        let result = tokio::select! {
            result = self.connect_inner(target, &request_id, events, &cancellation) => result,
            () = wait_for_cancellation(&cancellation) => Err(ExplorerError::Cancelled),
        };
        if let Ok(mut connections) = self.connections.lock() {
            connections.remove(&request_id);
        }
        result
    }

    async fn connect_inner(
        &self,
        target: ResolvedSshTarget,
        request_id: &str,
        events: Arc<Channel<SshConnectionEventDto>>,
        cancelled: &AtomicBool,
    ) -> Result<LocationSummaryDto, ExplorerError> {
        emit_state(&events, "connecting")?;
        let lifecycle = Arc::new(AtomicU8::new(SESSION_CONNECTING));
        let handler = HostKeyHandler {
            request_id: request_id.to_owned(),
            target: target.clone(),
            prompts: self.prompts.clone(),
            events: events.clone(),
            lifecycle: lifecycle.clone(),
        };
        let config = Arc::new(ssh_client_config());
        let connect = client::connect(config, (target.host.as_str(), target.port), handler);
        let mut handle = tokio::time::timeout(CONNECTION_TIMEOUT, connect)
            .await
            .map_err(|_| {
                ExplorerError::Offline(format!("The SSH connection to {} timed out.", target.host))
            })?
            .map_err(|error| match error {
                ExplorerError::Cancelled | ExplorerError::HostKeyFailure(_) => error,
                other => ExplorerError::Offline(format!(
                    "Explora could not connect to {}: {other}",
                    target.host
                )),
            })?;
        ensure_not_cancelled(cancelled)?;
        emit_state(&events, "authenticating")?;

        let none = handle.authenticate_none(target.username.clone()).await?;
        let mut authenticated = none.success();
        let methods = match none {
            AuthResult::Failure {
                remaining_methods, ..
            } => remaining_methods,
            AuthResult::Success => russh::MethodSet::empty(),
        };

        if !authenticated && !target.identities_only {
            authenticated = try_agent_auth(&mut handle, &target.username).await;
        }
        if !authenticated {
            authenticated = self
                .try_identity_files(&mut handle, &target, request_id, &events, cancelled)
                .await?;
        }
        if !authenticated && methods.contains(&MethodKind::Password) {
            authenticated = self
                .try_password(&mut handle, &target, request_id, &events)
                .await?;
        }
        if !authenticated && methods.contains(&MethodKind::KeyboardInteractive) {
            authenticated = self
                .try_keyboard_interactive(&mut handle, &target, request_id, &events)
                .await?;
        }
        if !authenticated {
            return Err(ExplorerError::AuthenticationFailed(format!(
                "Authentication failed for {}.",
                endpoint(&target)
            )));
        }
        ensure_not_cancelled(cancelled)?;
        emit_state(&events, "openingSftp")?;
        let channel = handle.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await.map_err(|_| {
            ExplorerError::Unsupported(
                "The SSH server does not provide an SFTP subsystem.".to_owned(),
            )
        })?;
        let sftp = Arc::new(SftpSession::new(channel.into_stream()).await.map_err(|_| {
            ExplorerError::Unsupported(
                "The SSH server did not provide a usable SFTP subsystem.".to_owned(),
            )
        })?);
        sftp.set_timeout(SFTP_REQUEST_TIMEOUT_SECONDS);
        let initial_path = sftp
            .canonicalize(target.initial_path.clone())
            .await
            .map_err(map_sftp_error)?;
        let location_id = location_id(&target.id);
        let paths = {
            let mut registries = self
                .path_registries
                .lock()
                .map_err(|_| ExplorerError::StateUnavailable)?;
            registries
                .entry(target.id.clone())
                .or_insert_with(|| Arc::new(RemotePathRegistry::default()))
                .clone()
        };
        let root_metadata = sftp
            .symlink_metadata(initial_path.clone())
            .await
            .map_err(map_sftp_error)?;
        let root = DirectoryRefDto {
            id: paths.register_with_metadata(initial_path.clone(), &root_metadata)?,
            location_id: location_id.clone(),
            name: target.name.clone(),
            display_path: format!("{}:{initial_path}", target.name),
            capabilities: DirectoryCapabilitiesDto::SFTP,
        };
        let target_id = target.id.clone();
        let session = Arc::new(SshSession {
            target,
            location_id,
            root,
            paths,
            handle: AsyncMutex::new(handle),
            sftp,
            mutation_guard: Arc::new(AsyncMutex::new(())),
            lifecycle: lifecycle.clone(),
            events: events.clone(),
        });
        lifecycle
            .compare_exchange(
                SESSION_CONNECTING,
                SESSION_ACTIVE,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .map_err(|_| {
                ExplorerError::Offline(
                    "The SSH connection closed while Explora was opening SFTP.".to_owned(),
                )
            })?;
        let location = session.location();
        self.sessions
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .insert(target_id, session);
        emit_state(&events, "connected")?;
        Ok(location)
    }

    async fn try_identity_files(
        &self,
        handle: &mut Handle<HostKeyHandler>,
        target: &ResolvedSshTarget,
        request_id: &str,
        events: &Channel<SshConnectionEventDto>,
        cancelled: &AtomicBool,
    ) -> Result<bool, ExplorerError> {
        for path in &target.identity_files {
            ensure_not_cancelled(cancelled)?;
            if !path.is_file() {
                continue;
            }
            let key = match keys::load_secret_key(path, None) {
                Ok(key) => key,
                Err(keys::Error::KeyIsEncrypted) => {
                    let prompt_id = Uuid::new_v4().to_string();
                    let response = self
                        .prompts
                        .request(
                            request_id,
                            &prompt_id,
                            SshConnectionEventDto::AuthenticationPrompt {
                                prompt_id: prompt_id.clone(),
                                kind: "passphrase",
                                title: "Unlock private key".to_owned(),
                                instructions: format!(
                                    "Enter the passphrase for {}.",
                                    path.file_name()
                                        .and_then(|name| name.to_str())
                                        .unwrap_or("the selected identity")
                                ),
                                fields: vec![SshPromptFieldDto {
                                    label: "Passphrase".to_owned(),
                                    secret: true,
                                }],
                            },
                            events,
                        )
                        .await?;
                    let mut answers = require_answers(response, 1)?;
                    let passphrase = answers.first().map(String::as_str).unwrap_or_default();
                    let result = keys::load_secret_key(path, Some(passphrase));
                    answers.zeroize();
                    result.map_err(|_| {
                        ExplorerError::AuthenticationFailed(
                            "The private-key passphrase was not accepted.".to_owned(),
                        )
                    })?
                }
                Err(_) => continue,
            };
            let key = Arc::new(key);
            let hash = if key.algorithm().is_rsa() {
                handle
                    .best_supported_rsa_hash()
                    .await?
                    .flatten()
                    .or(Some(HashAlg::Sha256))
            } else {
                None
            };
            if handle
                .authenticate_publickey(
                    target.username.clone(),
                    PrivateKeyWithHashAlg::new(key, hash),
                )
                .await?
                .success()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn try_password(
        &self,
        handle: &mut Handle<HostKeyHandler>,
        target: &ResolvedSshTarget,
        request_id: &str,
        events: &Channel<SshConnectionEventDto>,
    ) -> Result<bool, ExplorerError> {
        let prompt_id = Uuid::new_v4().to_string();
        let response = self
            .prompts
            .request(
                request_id,
                &prompt_id,
                SshConnectionEventDto::AuthenticationPrompt {
                    prompt_id: prompt_id.clone(),
                    kind: "password",
                    title: format!("Sign in to {}", target.name),
                    instructions: endpoint(target),
                    fields: vec![SshPromptFieldDto {
                        label: "Password".to_owned(),
                        secret: true,
                    }],
                },
                events,
            )
            .await?;
        let mut answers = require_answers(response, 1)?;
        let mut password = answers.first().cloned().unwrap_or_default();
        let authentication = handle
            .authenticate_password(target.username.clone(), password.clone())
            .await;
        password.zeroize();
        answers.zeroize();
        Ok(authentication?.success())
    }

    async fn try_keyboard_interactive(
        &self,
        handle: &mut Handle<HostKeyHandler>,
        target: &ResolvedSshTarget,
        request_id: &str,
        events: &Channel<SshConnectionEventDto>,
    ) -> Result<bool, ExplorerError> {
        let mut response = handle
            .authenticate_keyboard_interactive_start(target.username.clone(), None)
            .await?;
        loop {
            match response {
                KeyboardInteractiveAuthResponse::Success => return Ok(true),
                KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
                KeyboardInteractiveAuthResponse::InfoRequest {
                    name,
                    instructions,
                    prompts,
                } => {
                    let prompt_id = Uuid::new_v4().to_string();
                    let expected = prompts.len();
                    let answer = self
                        .prompts
                        .request(
                            request_id,
                            &prompt_id,
                            SshConnectionEventDto::AuthenticationPrompt {
                                prompt_id: prompt_id.clone(),
                                kind: "keyboardInteractive",
                                title: if name.is_empty() {
                                    format!("Verify {}", target.name)
                                } else {
                                    name
                                },
                                instructions,
                                fields: prompts
                                    .into_iter()
                                    .map(|prompt| SshPromptFieldDto {
                                        label: prompt.prompt,
                                        secret: !prompt.echo,
                                    })
                                    .collect(),
                            },
                            events,
                        )
                        .await?;
                    let mut answers = require_answers(answer, expected)?;
                    let authentication = handle
                        .authenticate_keyboard_interactive_respond(answers.clone())
                        .await;
                    answers.zeroize();
                    response = authentication?;
                }
            }
        }
    }

    pub fn respond(
        &self,
        request_id: &str,
        prompt_id: &str,
        response: SshPromptResponseDto,
    ) -> Result<(), ExplorerError> {
        self.prompts.respond(request_id, prompt_id, response)
    }

    pub fn cancel_connection(&self, request_id: &str) -> Result<(), ExplorerError> {
        if let Some(connection) = self
            .connections
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .get(request_id)
        {
            connection.cancelled.store(true, Ordering::Relaxed);
        }
        self.prompts.cancel(request_id);
        Ok(())
    }

    pub async fn disconnect(&self, target_id: &str) -> Result<(), ExplorerError> {
        let session = {
            let mut sessions = self
                .sessions
                .lock()
                .map_err(|_| ExplorerError::StateUnavailable)?;
            let Some(session) = sessions.get(target_id) else {
                return Ok(());
            };
            session
                .lifecycle
                .store(SESSION_DISCONNECTING, Ordering::SeqCst);
            sessions
                .remove(target_id)
                .ok_or(ExplorerError::StateUnavailable)?
        };
        let _ = session.sftp.close().await;
        let handle = session.handle.lock().await;
        let _ = handle
            .disconnect(
                russh::Disconnect::ByApplication,
                "Disconnected by Explora",
                "en",
            )
            .await;
        Ok(())
    }

    pub async fn forget_target(&self, target_id: &str) -> Result<(), ExplorerError> {
        self.disconnect(target_id).await?;
        self.path_registries
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .remove(target_id);
        Ok(())
    }

    pub async fn list_directory<F>(
        &self,
        location_id: &str,
        directory_id: &str,
        cancelled: &AtomicBool,
        emit: F,
    ) -> Result<(), ExplorerError>
    where
        F: FnMut(DirectoryListingEvent) -> Result<(), ExplorerError>,
    {
        let session = self
            .sessions
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .values()
            .find(|session| session.location_id == location_id)
            .cloned()
            .ok_or_else(|| ExplorerError::Offline("This SSH target is disconnected.".to_owned()))?;
        if session.lifecycle.load(Ordering::SeqCst) != SESSION_ACTIVE {
            return Err(ExplorerError::Offline(
                "This SSH target is disconnected.".to_owned(),
            ));
        }
        let result = session.list_directory(directory_id, cancelled, emit).await;
        if matches!(result, Err(ExplorerError::Offline(_)))
            || (result.is_err() && session.lifecycle.load(Ordering::SeqCst) != SESSION_ACTIVE)
        {
            session.mark_offline();
            return Err(ExplorerError::Offline(
                "The SSH connection was lost. Reconnect to continue browsing.".to_owned(),
            ));
        }
        result
    }

    pub async fn rename_entry(
        &self,
        source: &EntryRefDto,
        new_name: &str,
        cancelled: &AtomicBool,
    ) -> Result<FileEntrySummaryDto, ExplorerError> {
        let session = self.active_session(&source.location_id)?;
        let result = session.rename_entry(source, new_name, cancelled).await;
        finish_remote_result(&session, result)
    }

    pub async fn move_entry(
        &self,
        source: &EntryRefDto,
        destination: &DirectoryRefDto,
        conflict_policy: RemoteMoveConflictPolicy,
        cancelled: &AtomicBool,
    ) -> Result<MovedRemoteEntry, ExplorerError> {
        let session = self.active_session(&source.location_id)?;
        let result = session
            .move_entry(source, destination, conflict_policy, cancelled)
            .await;
        finish_remote_result(&session, result)
    }

    pub(crate) async fn prepare_transfer_source(
        &self,
        source: &EntryRefDto,
        cancelled: &AtomicBool,
    ) -> Result<PreparedRemoteTransfer, ExplorerError> {
        let session = self.active_session(&source.location_id)?;
        let mutation_guard = session.mutation_guard.clone().lock_owned().await;
        ensure_not_cancelled(cancelled)?;
        let (path, metadata) = match session.revalidate_source(source).await {
            Ok(source) => source,
            Err(error) => return finish_remote_result(&session, Err(error)),
        };
        let entries = finish_remote_result(
            &session,
            plan_remote_transfer(&session, &path, cancelled).await,
        )?;
        let root = entries.first().ok_or(ExplorerError::StateUnavailable)?;
        if root.fingerprint != RemoteEntryFingerprint::from(&metadata) {
            return Err(ExplorerError::SourceChanged);
        }
        let source_parent_path = remote_parent(&path).ok_or(ExplorerError::InvalidReference)?;
        let total_bytes = entries.iter().try_fold(0_u64, |total, entry| {
            total.checked_add(entry.len).ok_or_else(|| {
                ExplorerError::InvalidConfiguration(
                    "The remote transfer exceeds the supported size.".to_owned(),
                )
            })
        })?;
        Ok(PreparedRemoteTransfer {
            session: session.clone(),
            entries,
            source: source.clone(),
            name: remote_name(&path),
            source_parent: session.directory_ref(&source_parent_path, None)?,
            total_bytes,
            permissions: metadata.permissions,
            _mutation_guard: mutation_guard,
        })
    }

    pub(crate) async fn prepare_transfer_destination(
        &self,
        destination: &DirectoryRefDto,
        source_name: &str,
        kind: &RemoteTransferDestinationKind,
        conflict_policy: RemoteMoveConflictPolicy,
        cancelled: &AtomicBool,
    ) -> Result<PreparedRemoteDestination, ExplorerError> {
        validate_remote_name(source_name)?;
        let session = self.active_session(&destination.location_id)?;
        let mutation_guard = session.mutation_guard.clone().lock_owned().await;
        ensure_not_cancelled(cancelled)?;
        let destination_path = match session.revalidate_directory(destination).await {
            Ok(path) => path,
            Err(error) => return finish_remote_result(&session, Err(error)),
        };
        let (artifact, final_name) = match conflict_policy {
            RemoteMoveConflictPolicy::Fail => {
                let artifact = create_remote_transfer_artifact(
                    session.sftp.clone(),
                    &destination_path,
                    source_name,
                    kind,
                )
                .await;
                (
                    finish_remote_result(&session, artifact)?,
                    source_name.to_owned(),
                )
            }
            RemoteMoveConflictPolicy::KeepBoth => {
                let mut prepared = None;
                for attempt in 1..=MAX_KEEP_BOTH_ATTEMPTS {
                    ensure_not_cancelled(cancelled)?;
                    let candidate = remote_keep_both_name(
                        source_name,
                        matches!(kind, RemoteTransferDestinationKind::Directory),
                        attempt,
                    )?;
                    match create_remote_transfer_artifact(
                        session.sftp.clone(),
                        &destination_path,
                        &candidate,
                        kind,
                    )
                    .await
                    {
                        Ok(artifact) => {
                            prepared = Some((artifact, candidate));
                            break;
                        }
                        Err(ExplorerError::Conflict) => continue,
                        Err(error) => return finish_remote_result(&session, Err(error)),
                    }
                }
                prepared.ok_or(ExplorerError::Conflict)?
            }
        };
        let authoritative_destination = session.directory_ref(&destination_path, None)?;
        debug_assert_eq!(
            artifact.final_path(),
            remote_join(&destination_path, &final_name)
        );
        Ok(PreparedRemoteDestination {
            session,
            artifact: Some(artifact),
            destination: authoritative_destination,
            _mutation_guard: mutation_guard,
        })
    }

    pub async fn describe_move_conflict(
        &self,
        source: &EntryRefDto,
        destination: &DirectoryRefDto,
    ) -> Result<(String, String), ExplorerError> {
        let session = self.active_session(&source.location_id)?;
        let result = session.describe_move_conflict(source, destination).await;
        finish_remote_result(&session, result)
    }

    pub(crate) async fn describe_transfer_destination(
        &self,
        destination: &DirectoryRefDto,
    ) -> Result<String, ExplorerError> {
        let session = self.active_session(&destination.location_id)?;
        let result = session
            .revalidate_directory(destination)
            .await
            .map(|path| remote_name(&path));
        finish_remote_result(&session, result)
    }

    pub async fn describe_operation_target(
        &self,
        source: &EntryRefDto,
    ) -> Result<(String, String), ExplorerError> {
        let session = self.active_session(&source.location_id)?;
        let result = session.describe_operation_target(source).await;
        finish_remote_result(&session, result)
    }

    pub(crate) fn validate_batch_sources(
        &self,
        sources: &[EntryRefDto],
    ) -> Result<(), ExplorerError> {
        let first = sources.first().ok_or_else(|| {
            ExplorerError::InvalidConfiguration(
                "A filesystem action requires at least one selected item.".to_owned(),
            )
        })?;
        let session = self.active_session(&first.location_id)?;
        let mut paths = Vec::with_capacity(sources.len());
        for source in sources {
            if source.location_id != first.location_id {
                return Err(ExplorerError::InvalidConfiguration(
                    "A batch filesystem action must use items from one location.".to_owned(),
                ));
            }
            paths.push(session.paths.resolve(&source.id)?);
        }
        for (index, path) in paths.iter().enumerate() {
            if paths
                .iter()
                .skip(index + 1)
                .any(|other| remote_path_contains(path, other) || remote_path_contains(other, path))
            {
                return Err(ExplorerError::InvalidConfiguration(
                    "A batch action cannot include both a folder and one of its descendants."
                        .to_owned(),
                ));
            }
        }
        Ok(())
    }

    pub async fn permanently_delete_entry<F>(
        &self,
        source: &EntryRefDto,
        cancelled: &AtomicBool,
        on_progress: F,
    ) -> Result<RemovedRemoteEntry, ExplorerError>
    where
        F: FnMut(u64, u64) -> Result<(), ExplorerError>,
    {
        let session = self.active_session(&source.location_id)?;
        let result = session
            .permanently_delete_entry(source, cancelled, on_progress)
            .await;
        finish_remote_result(&session, result)
    }

    fn active_session(&self, location_id: &str) -> Result<Arc<SshSession>, ExplorerError> {
        let session = self
            .sessions
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .values()
            .find(|session| session.location_id == location_id)
            .cloned()
            .ok_or_else(|| ExplorerError::Offline("This SSH target is disconnected.".to_owned()))?;
        if session.lifecycle.load(Ordering::SeqCst) != SESSION_ACTIVE {
            return Err(ExplorerError::Offline(
                "This SSH target is disconnected.".to_owned(),
            ));
        }
        Ok(session)
    }
}

fn remote_path_contains(ancestor: &str, candidate: &str) -> bool {
    if ancestor == candidate {
        return true;
    }
    if ancestor == "/" {
        return candidate.starts_with('/');
    }
    candidate
        .strip_prefix(ancestor)
        .is_some_and(|suffix| suffix.starts_with('/'))
}

fn finish_remote_result<T>(
    session: &SshSession,
    result: Result<T, ExplorerError>,
) -> Result<T, ExplorerError> {
    if matches!(result, Err(ExplorerError::Offline(_))) {
        session.mark_offline();
    }
    result
}

fn require_answers(
    response: SshPromptResponseDto,
    expected: usize,
) -> Result<Vec<String>, ExplorerError> {
    match response {
        SshPromptResponseDto::Answers { answers } if answers.len() == expected => Ok(answers),
        SshPromptResponseDto::Reject => Err(ExplorerError::Cancelled),
        _ => Err(ExplorerError::InvalidReference),
    }
}

fn ssh_client_config() -> client::Config {
    client::Config {
        keepalive_interval: Some(SSH_KEEPALIVE_INTERVAL),
        keepalive_max: SSH_KEEPALIVE_MAX,
        nodelay: true,
        ..client::Config::default()
    }
}

#[cfg(unix)]
async fn try_agent_auth(handle: &mut Handle<HostKeyHandler>, username: &str) -> bool {
    let Ok(mut agent) = AgentClient::connect_env().await else {
        return false;
    };
    authenticate_with_agent(handle, username, &mut agent).await
}

#[cfg(windows)]
async fn try_agent_auth(handle: &mut Handle<HostKeyHandler>, username: &str) -> bool {
    if let Ok(path) = std::env::var("SSH_AUTH_SOCK") {
        if let Ok(mut agent) = AgentClient::connect_named_pipe(path).await {
            if authenticate_with_agent(handle, username, &mut agent).await {
                return true;
            }
        }
    }
    if let Ok(mut agent) = AgentClient::connect_pageant().await {
        return authenticate_with_agent(handle, username, &mut agent).await;
    }
    false
}

async fn authenticate_with_agent<S>(
    handle: &mut Handle<HostKeyHandler>,
    username: &str,
    agent: &mut AgentClient<S>,
) -> bool
where
    S: russh::keys::agent::client::AgentStream + Send + Unpin,
{
    let Ok(identities) = agent.request_identities().await else {
        return false;
    };
    for identity in identities {
        let key = identity.public_key().into_owned();
        let hash = if key.algorithm().is_rsa() {
            Some(HashAlg::Sha256)
        } else {
            None
        };
        if handle
            .authenticate_publickey_with(username.to_owned(), key, hash, agent)
            .await
            .map(|result| result.success())
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

fn emit_state(
    channel: &Channel<SshConnectionEventDto>,
    state: &'static str,
) -> Result<(), ExplorerError> {
    channel
        .send(SshConnectionEventDto::State { state })
        .map_err(|_| ExplorerError::ChannelClosed)
}

fn ensure_not_cancelled(cancelled: &AtomicBool) -> Result<(), ExplorerError> {
    if cancelled.load(Ordering::Relaxed) {
        Err(ExplorerError::Cancelled)
    } else {
        Ok(())
    }
}

async fn wait_for_cancellation(cancelled: &AtomicBool) {
    while !cancelled.load(Ordering::Relaxed) {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

fn endpoint(target: &ResolvedSshTarget) -> String {
    if target.port == 22 {
        format!("{}@{}", target.username, target.host)
    } else {
        format!("{}@{}:{}", target.username, target.host, target.port)
    }
}

fn map_sftp_error(error: SftpError) -> ExplorerError {
    match error {
        SftpError::Status(status) => match status.status_code {
            StatusCode::NoSuchFile => ExplorerError::Io {
                message: "The remote path no longer exists.".to_owned(),
                kind: std::io::ErrorKind::NotFound,
            },
            StatusCode::PermissionDenied => ExplorerError::Io {
                message: "Permission was denied for the remote path.".to_owned(),
                kind: std::io::ErrorKind::PermissionDenied,
            },
            StatusCode::NoConnection | StatusCode::ConnectionLost => {
                ExplorerError::Offline("The SSH connection was lost.".to_owned())
            }
            StatusCode::OpUnsupported => ExplorerError::Unsupported(
                "The SSH server does not support this SFTP operation.".to_owned(),
            ),
            _ => ExplorerError::Unexpected(format!(
                "The SFTP server returned status {}.",
                status.status_code
            )),
        },
        SftpError::Timeout => ExplorerError::Offline("The SFTP request timed out.".to_owned()),
        SftpError::IO(_) | SftpError::UnexpectedBehavior(_) => {
            ExplorerError::Offline("The SSH connection was lost.".to_owned())
        }
        other => ExplorerError::Unexpected(format!("The SFTP request failed: {other}")),
    }
}

fn is_sftp_connectivity_error(error: &SftpError) -> bool {
    matches!(
        error,
        SftpError::Timeout | SftpError::IO(_) | SftpError::UnexpectedBehavior(_)
    ) || matches!(
        error,
        SftpError::Status(status)
            if matches!(
                status.status_code,
                StatusCode::NoConnection | StatusCode::ConnectionLost
            )
    )
}

async fn create_remote_transfer_artifact(
    sftp: Arc<SftpSession>,
    destination_directory: &str,
    final_name: &str,
    kind: &RemoteTransferDestinationKind,
) -> Result<OwnedRemoteTransferArtifact, ExplorerError> {
    match kind {
        RemoteTransferDestinationKind::File => {
            OwnedRemoteTransferArtifact::create_file(sftp, destination_directory, final_name).await
        }
        RemoteTransferDestinationKind::Directory => {
            OwnedRemoteTransferArtifact::create_directory(sftp, destination_directory, final_name)
                .await
        }
        RemoteTransferDestinationKind::Symlink { target } => {
            OwnedRemoteTransferArtifact::create_symlink(
                sftp,
                destination_directory,
                final_name,
                target,
            )
            .await
        }
    }
}

async fn plan_remote_transfer(
    session: &SshSession,
    source_root: &str,
    cancelled: &AtomicBool,
) -> Result<Vec<RemoteTransferPlanEntry>, ExplorerError> {
    if source_root.len() > MAX_REMOTE_TRANSFER_PATH_BYTES {
        return Err(ExplorerError::Unsupported(
            "The remote source path is too long to transfer safely.".to_owned(),
        ));
    }

    let mut pending = vec![(source_root.to_owned(), String::new(), 0_usize)];
    let mut entries = Vec::new();
    while let Some((path, relative_path, depth)) = pending.pop() {
        ensure_not_cancelled(cancelled)?;
        if entries.len() >= MAX_REMOTE_DELETE_ENTRIES {
            return Err(ExplorerError::Unsupported(format!(
                "Remote transfers are limited to {MAX_REMOTE_DELETE_ENTRIES} entries."
            )));
        }
        if depth > MAX_REMOTE_DELETE_DEPTH {
            return Err(ExplorerError::Unsupported(format!(
                "Remote transfers are limited to {MAX_REMOTE_DELETE_DEPTH} directory levels."
            )));
        }

        let metadata = session
            .sftp
            .symlink_metadata(path.clone())
            .await
            .map_err(map_sftp_error)?;
        let (kind, link_target, len) = if metadata.is_symlink() {
            let target = session
                .sftp
                .read_link(path.clone())
                .await
                .map_err(map_sftp_error)?;
            if target.len() > MAX_REMOTE_TRANSFER_PATH_BYTES || target.contains('\0') {
                return Err(ExplorerError::Unsupported(
                    "A remote symbolic-link target is too long or invalid to transfer safely."
                        .to_owned(),
                ));
            }
            (RemoteTransferEntryKind::Symlink, Some(target), 0)
        } else if metadata.is_dir() {
            let mut child_names = session
                .sftp
                .read_dir(path.clone())
                .await
                .map_err(map_sftp_error)?
                .map(|entry| entry.file_name())
                .collect::<Vec<_>>();
            child_names.sort_unstable();
            if child_names.windows(2).any(|names| names[0] == names[1]) {
                return Err(ExplorerError::Unexpected(
                    "The SFTP server returned duplicate directory entries.".to_owned(),
                ));
            }
            for child_name in child_names.into_iter().rev() {
                validate_remote_manifest_name(&child_name)?;
                let child_path = remote_join(&path, &child_name);
                if child_path.len() > MAX_REMOTE_TRANSFER_PATH_BYTES {
                    return Err(ExplorerError::Unsupported(
                        "A remote transfer path is too long to process safely.".to_owned(),
                    ));
                }
                let child_relative = if relative_path.is_empty() {
                    child_name
                } else {
                    format!("{relative_path}/{child_name}")
                };
                pending.push((child_path, child_relative, depth + 1));
            }
            (RemoteTransferEntryKind::Directory, None, 0)
        } else if metadata.is_regular() {
            (
                RemoteTransferEntryKind::File,
                None,
                metadata.size.unwrap_or(0),
            )
        } else {
            return Err(ExplorerError::Unsupported(
                "This remote item type cannot be transferred safely.".to_owned(),
            ));
        };
        entries.push(RemoteTransferPlanEntry {
            path,
            relative_path,
            kind,
            fingerprint: RemoteEntryFingerprint::from(&metadata),
            link_target,
            permissions: metadata.permissions,
            len,
        });
    }
    Ok(entries)
}

fn validate_remote_manifest_name(name: &str) -> Result<(), ExplorerError> {
    validate_remote_name(name).map_err(|_| {
        ExplorerError::Unexpected(
            "The SFTP server returned an invalid directory entry name.".to_owned(),
        )
    })
}

fn validate_remote_name(name: &str) -> Result<(), ExplorerError> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > MAX_REMOTE_NAME_BYTES
        || name.contains('/')
        || name.contains('\0')
    {
        return Err(ExplorerError::InvalidName(format!(
            "Enter a remote file name between 1 and {MAX_REMOTE_NAME_BYTES} bytes without ‘/’ or a null character."
        )));
    }
    Ok(())
}

fn remote_join(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{}/{name}", parent.trim_end_matches('/'))
    }
}

fn remote_is_same_or_descendant(path: &str, ancestor: &str) -> bool {
    path == ancestor
        || (ancestor == "/" && path.starts_with('/'))
        || path
            .strip_prefix(ancestor)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn remote_keep_both_name(
    original_name: &str,
    is_directory: bool,
    attempt: usize,
) -> Result<String, ExplorerError> {
    let suffix = if attempt == 1 {
        " copy".to_owned()
    } else {
        format!(" copy {attempt}")
    };
    let extension_index = (!is_directory)
        .then(|| original_name.rfind('.'))
        .flatten()
        .filter(|index| *index > 0);
    let (stem, extension) = extension_index
        .map(|index| original_name.split_at(index))
        .unwrap_or((original_name, ""));
    let extension = truncate_utf8(extension, MAX_REMOTE_NAME_BYTES / 2);
    let stem_capacity = MAX_REMOTE_NAME_BYTES
        .saturating_sub(suffix.len())
        .saturating_sub(extension.len());
    let stem = truncate_utf8(stem, stem_capacity);
    let candidate = format!(
        "{}{}{}",
        if stem.is_empty() { "item" } else { stem },
        suffix,
        extension
    );
    validate_remote_name(&candidate)?;
    Ok(candidate)
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &value[..boundary]
}

fn remote_parent(path: &str) -> Option<String> {
    if path == "/" {
        return None;
    }
    let trimmed = path.trim_end_matches('/');
    let parent = trimmed
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("");
    Some(if parent.is_empty() { "/" } else { parent }.to_owned())
}

fn remote_name(path: &str) -> String {
    if path == "/" {
        return "/".to_owned();
    }
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_owned()
}

fn remote_breadcrumbs(
    session: &SshSession,
    path: &str,
) -> Result<Vec<BreadcrumbSegmentDto>, ExplorerError> {
    let mut result = Vec::new();
    let mut current = if path.starts_with('/') {
        "/".to_owned()
    } else {
        String::new()
    };
    if path.starts_with('/') {
        result.push(BreadcrumbSegmentDto {
            label: session.target.name.clone(),
            directory: session.directory_ref("/", Some(session.target.name.clone()))?,
        });
    }
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        if current == "/" || current.is_empty() {
            current.push_str(segment);
        } else {
            current.push('/');
            current.push_str(segment);
        }
        result.push(BreadcrumbSegmentDto {
            label: segment.to_owned(),
            directory: session.directory_ref(&current, Some(segment.to_owned()))?,
        });
    }
    if result.is_empty() {
        result.push(BreadcrumbSegmentDto {
            label: session.target.name.clone(),
            directory: session.directory_ref(path, Some(session.target.name.clone()))?,
        });
    }
    Ok(result)
}

fn content_kind(name: &str, is_directory: bool) -> &'static str {
    if is_directory {
        return "folder";
    }
    let extension = Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg") => "image",
        Some("md" | "txt" | "pdf" | "doc" | "docx" | "rtf") => "document",
        Some("rs" | "ts" | "js" | "svelte" | "py" | "go" | "json" | "toml" | "yaml" | "yml") => {
            "code"
        }
        Some("mp3" | "m4a" | "wav" | "flac" | "ogg") => "audio",
        Some("mp4" | "mov" | "mkv" | "webm" | "avi") => "video",
        Some("zip" | "tar" | "gz" | "xz" | "zst" | "7z" | "rar") => "archive",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use serde_json::Value;
    use tauri::ipc::InvokeResponseBody;

    use crate::ssh_test_server::{TestAuthMode, TestSshServer};

    #[cfg(unix)]
    static SSH_AGENT_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[cfg(unix)]
    struct EnvironmentVariableGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    #[cfg(unix)]
    impl EnvironmentVariableGuard {
        fn set(key: &'static str, value: &std::ffi::OsStr) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: SSH-agent tests serialize all access to this process-wide variable
            // with SSH_AGENT_ENV_LOCK and restore the previous value in Drop.
            unsafe { std::env::set_var(key, value) };
            Self { key, previous }
        }
    }

    #[cfg(unix)]
    impl Drop for EnvironmentVariableGuard {
        fn drop(&mut self) {
            // SAFETY: The matching lock guard is held until after this value is dropped.
            unsafe {
                if let Some(previous) = &self.previous {
                    std::env::set_var(self.key, previous);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    #[derive(Clone, Default)]
    struct PromptAnswers {
        accept_host_key: bool,
        passphrase: Option<String>,
        password: Option<String>,
        keyboard_interactive: Option<String>,
    }

    fn target_for(
        server: &TestSshServer,
        identity_files: Vec<std::path::PathBuf>,
        identities_only: bool,
    ) -> ResolvedSshTarget {
        ResolvedSshTarget {
            id: "test-target".to_owned(),
            name: "Test server".to_owned(),
            host: server.host().to_owned(),
            port: server.port(),
            username: server.username().to_owned(),
            initial_path: "/".to_owned(),
            identity_files,
            identities_only,
            known_hosts_path: server.known_hosts_path(),
        }
    }

    fn event_channel(
        manager: Arc<SshConnectionManager>,
        request_id: &str,
        answers: PromptAnswers,
    ) -> (Channel<SshConnectionEventDto>, Arc<Mutex<Vec<Value>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured_events = events.clone();
        let request_id = request_id.to_owned();
        let channel = Channel::new(move |body| {
            let InvokeResponseBody::Json(json) = body else {
                panic!("SSH events must be JSON");
            };
            let event: Value = serde_json::from_str(&json).expect("valid SSH event JSON");
            captured_events
                .lock()
                .expect("captured SSH events")
                .push(event.clone());

            let response = match event.get("event").and_then(Value::as_str) {
                Some("hostKeyPrompt") => Some(if answers.accept_host_key {
                    SshPromptResponseDto::Accept
                } else {
                    SshPromptResponseDto::Reject
                }),
                Some("authenticationPrompt") => {
                    let answer = match event.get("kind").and_then(Value::as_str) {
                        Some("passphrase") => answers.passphrase.clone(),
                        Some("password") => answers.password.clone(),
                        Some("keyboardInteractive") => answers.keyboard_interactive.clone(),
                        _ => None,
                    };
                    Some(match answer {
                        Some(answer) => SshPromptResponseDto::Answers {
                            answers: vec![answer],
                        },
                        None => SshPromptResponseDto::Reject,
                    })
                }
                _ => None,
            };
            if let Some(response) = response {
                let prompt_id = event
                    .get("promptId")
                    .and_then(Value::as_str)
                    .expect("prompt id");
                manager
                    .respond(&request_id, prompt_id, response)
                    .expect("respond to test SSH prompt");
            }
            Ok(())
        });
        (channel, events)
    }

    fn has_event(events: &Arc<Mutex<Vec<Value>>>, event_name: &str) -> bool {
        events
            .lock()
            .expect("SSH events")
            .iter()
            .any(|event| event.get("event").and_then(Value::as_str) == Some(event_name))
    }

    async fn wait_for_event(events: &Arc<Mutex<Vec<Value>>>, event_name: &str) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !has_event(events, event_name) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("timely SSH event");
    }

    async fn listing_events(
        manager: &SshConnectionManager,
        location_id: &str,
        directory_id: &str,
    ) -> Result<Vec<DirectoryListingEvent>, ExplorerError> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured_events = events.clone();
        manager
            .list_directory(
                location_id,
                directory_id,
                &AtomicBool::new(false),
                move |event| {
                    captured_events
                        .lock()
                        .map_err(|_| ExplorerError::StateUnavailable)?
                        .push(event);
                    Ok(())
                },
            )
            .await?;
        let result = events
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .clone();
        Ok(result)
    }

    fn listed_entries(events: &[DirectoryListingEvent]) -> Vec<FileEntrySummaryDto> {
        events
            .iter()
            .filter_map(|event| match event {
                DirectoryListingEvent::Entries { entries, .. } => Some(entries.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    #[test]
    fn remote_path_references_are_opaque_and_scoped_to_the_registry() {
        let first = RemotePathRegistry::default();
        let second = RemotePathRegistry::default();
        let reference = first
            .register("/srv/app/private.txt".to_owned())
            .expect("reference");

        assert!(!reference.contains("private.txt"));
        assert_eq!(
            first.resolve(&reference).expect("resolved path"),
            "/srv/app/private.txt"
        );
        assert!(matches!(
            second.resolve(&reference),
            Err(ExplorerError::InvalidReference)
        ));
    }

    #[test]
    fn remote_path_registry_rebases_and_invalidates_only_complete_subtrees() {
        let registry = RemotePathRegistry::default();
        let metadata = FileAttributes {
            size: Some(12),
            ..FileAttributes::empty()
        };
        let mut directory_metadata = FileAttributes::empty();
        directory_metadata.set_dir(true);
        let root = registry
            .register_with_metadata("/projects".to_owned(), &directory_metadata)
            .expect("project reference");
        let child = registry
            .register_with_metadata("/projects/notes.txt".to_owned(), &metadata)
            .expect("child reference");
        let sibling_prefix = registry
            .register_with_metadata("/projects-old/notes.txt".to_owned(), &metadata)
            .expect("prefix sibling reference");

        let mut rebased = registry
            .rebase_subtree("/projects", "/archive/projects")
            .expect("rebase subtree");
        rebased.sort();
        let mut expected = vec![root.clone(), child.clone()];
        expected.sort();
        assert_eq!(rebased, expected);
        assert_eq!(
            registry.resolve(&child).expect("rebased child"),
            "/archive/projects/notes.txt"
        );
        assert_eq!(
            registry.resolve(&sibling_prefix).expect("prefix sibling"),
            "/projects-old/notes.txt"
        );

        let removed = registry
            .invalidate_subtree("/archive/projects")
            .expect("invalidate subtree");
        assert_eq!(removed.len(), 2);
        assert!(matches!(
            registry.resolve(&child),
            Err(ExplorerError::InvalidReference)
        ));
        assert!(registry.resolve(&sibling_prefix).is_ok());
    }

    #[test]
    fn remote_names_reject_traversal_and_keep_both_respects_the_byte_bound() {
        for invalid in ["", ".", "..", "nested/name", "nul\0name"] {
            assert!(matches!(
                validate_remote_name(invalid),
                Err(ExplorerError::InvalidName(_))
            ));
        }
        let oversized = "é".repeat(MAX_REMOTE_NAME_BYTES / 2 + 1);
        assert!(matches!(
            validate_remote_name(&oversized),
            Err(ExplorerError::InvalidName(_))
        ));

        let original = format!("{}.tar.gz", "文".repeat(MAX_REMOTE_NAME_BYTES));
        let candidate =
            remote_keep_both_name(&original, false, 2).expect("bounded UTF-8 keep-both name");
        assert!(candidate.len() <= MAX_REMOTE_NAME_BYTES);
        assert!(candidate.ends_with(".gz"));
        assert!(candidate.contains(" copy 2"));
        assert!(validate_remote_name(&candidate).is_ok());
    }

    #[test]
    fn prompt_responses_require_the_matching_connection_request() {
        let broker = PromptBroker::default();
        let (sender, mut receiver) = oneshot::channel();
        broker.pending.lock().expect("prompt state").insert(
            "prompt-1".to_owned(),
            PendingPrompt {
                request_id: "connection-1".to_owned(),
                sender,
            },
        );

        assert!(matches!(
            broker.respond(
                "another-connection",
                "prompt-1",
                SshPromptResponseDto::Accept
            ),
            Err(ExplorerError::InvalidReference)
        ));
        assert!(matches!(
            receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));

        broker
            .respond("connection-1", "prompt-1", SshPromptResponseDto::Accept)
            .expect("matching response");
        assert!(matches!(
            receiver.try_recv().expect("response"),
            SshPromptResponseDto::Accept
        ));
    }

    #[test]
    fn remote_path_helpers_preserve_root_and_relative_semantics() {
        assert_eq!(remote_parent("/srv/app"), Some("/srv".to_owned()));
        assert_eq!(remote_parent("/"), None);
        assert_eq!(remote_parent("projects/app"), Some("projects".to_owned()));
        assert_eq!(remote_name("/srv/app/"), "app");
        assert_eq!(content_kind("main.rs", false), "code");
        assert_eq!(content_kind("assets", true), "folder");
    }

    #[test]
    fn ssh_transport_uses_bounded_keepalives_and_low_latency_sockets() {
        let config = ssh_client_config();
        assert_eq!(config.keepalive_interval, Some(SSH_KEEPALIVE_INTERVAL));
        assert_eq!(config.keepalive_max, SSH_KEEPALIVE_MAX);
        assert!(config.nodelay);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_sftp_listing_disconnect_and_reconnect_preserve_directory_identity() {
        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let manager = Arc::new(SshConnectionManager::default());
        let target = target_for(&server, vec![server.identity_file().to_owned()], true);
        let answers = PromptAnswers {
            accept_host_key: true,
            ..PromptAnswers::default()
        };
        let (channel, events) = event_channel(manager.clone(), "connect-1", answers.clone());
        let location = manager
            .connect(target.clone(), "connect-1".to_owned(), channel)
            .await
            .expect("connect through a real SSH handshake");
        assert!(has_event(&events, "hostKeyPrompt"));

        let root_events = listing_events(&manager, &location.id, &location.root.id)
            .await
            .expect("list real SFTP root");
        let entries = listed_entries(&root_events);
        assert!(entries.iter().any(|entry| entry.name == "README.md"));
        let symlink = entries
            .iter()
            .find(|entry| entry.name == "project-link")
            .expect("symlink entry");
        assert_eq!(symlink.kind, "symlink");
        assert!(symlink.directory.is_some());

        let private = entries
            .iter()
            .find(|entry| entry.name == "private")
            .and_then(|entry| entry.directory.as_ref())
            .expect("private directory");
        assert!(matches!(
            listing_events(&manager, &location.id, &private.id).await,
            Err(ExplorerError::Io {
                kind: std::io::ErrorKind::PermissionDenied,
                ..
            })
        ));

        let projects = entries
            .iter()
            .find(|entry| entry.name == "projects")
            .and_then(|entry| entry.directory.as_ref())
            .expect("projects directory")
            .clone();
        let project_events = listing_events(&manager, &location.id, &projects.id)
            .await
            .expect("list nested directory");
        assert!(listed_entries(&project_events)
            .iter()
            .any(|entry| entry.name == "notes.txt"));

        server.disconnect_clients().await;
        wait_for_event(&events, "disconnected").await;
        assert!(manager.locations().is_empty());

        let (reconnect_channel, reconnect_events) =
            event_channel(manager.clone(), "connect-2", answers);
        let reconnected = manager
            .connect(target, "connect-2".to_owned(), reconnect_channel)
            .await
            .expect("reconnect to real SSH server");
        assert_eq!(reconnected.id, location.id);
        assert!(!has_event(&reconnect_events, "hostKeyPrompt"));
        let restored_events = listing_events(&manager, &reconnected.id, &projects.id)
            .await
            .expect("reuse opaque directory reference after reconnect");
        assert!(listed_entries(&restored_events)
            .iter()
            .any(|entry| entry.name == "notes.txt"));

        manager.disconnect("test-target").await.expect("disconnect");
        server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remote_transfer_manifest_is_bounded_symlink_safe_and_revalidates_the_tree() {
        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let manager = Arc::new(SshConnectionManager::default());
        let target = target_for(&server, vec![server.identity_file().to_owned()], true);
        let answers = PromptAnswers {
            accept_host_key: true,
            ..PromptAnswers::default()
        };
        let (channel, _) = event_channel(manager.clone(), "transfer-plan", answers);
        let location = manager
            .connect(target, "transfer-plan".to_owned(), channel)
            .await
            .expect("connect to transfer fixture");
        let root_entries = listed_entries(
            &listing_events(&manager, &location.id, &location.root.id)
                .await
                .expect("list transfer root"),
        );
        let projects = root_entries
            .iter()
            .find(|entry| entry.name == "projects")
            .expect("projects directory");
        let symlink = root_entries
            .iter()
            .find(|entry| entry.name == "project-link")
            .expect("project symlink");

        let directory = manager
            .prepare_transfer_source(&projects.reference, &AtomicBool::new(false))
            .await
            .expect("plan directory transfer");
        assert_eq!(directory.total_bytes, 42);
        assert_eq!(directory.entries.len(), 3);
        assert_eq!(directory.entries[0].relative_path, "");
        assert_eq!(
            directory.entries[0].kind,
            RemoteTransferEntryKind::Directory
        );
        assert_eq!(directory.entries[1].relative_path, "explora");
        assert_eq!(
            directory.entries[1].kind,
            RemoteTransferEntryKind::Directory
        );
        assert_eq!(directory.entries[2].relative_path, "notes.txt");
        assert_eq!(directory.entries[2].kind, RemoteTransferEntryKind::File);
        directory
            .revalidate()
            .await
            .expect("unchanged directory manifest remains valid");
        server.write_file("/projects/notes.txt", vec![9; 43]).await;
        assert!(matches!(
            directory.revalidate().await,
            Err(ExplorerError::SourceChanged)
        ));
        drop(directory);

        let link = manager
            .prepare_transfer_source(&symlink.reference, &AtomicBool::new(false))
            .await
            .expect("plan symlink transfer");
        assert_eq!(link.total_bytes, 0);
        assert_eq!(link.entries.len(), 1);
        assert_eq!(link.entries[0].kind, RemoteTransferEntryKind::Symlink);
        assert_eq!(link.entries[0].link_target.as_deref(), Some("/projects"));
        drop(link);

        manager.disconnect("test-target").await.expect("disconnect");
        server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn verified_remote_tree_removal_preserves_an_unplanned_late_child() {
        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let manager = Arc::new(SshConnectionManager::default());
        let target = target_for(&server, vec![server.identity_file().to_owned()], true);
        let answers = PromptAnswers {
            accept_host_key: true,
            ..PromptAnswers::default()
        };
        let (channel, _) = event_channel(manager.clone(), "transfer-remove", answers);
        let location = manager
            .connect(target, "transfer-remove".to_owned(), channel)
            .await
            .expect("connect to transfer fixture");
        let projects = listed_entries(
            &listing_events(&manager, &location.id, &location.root.id)
                .await
                .expect("list transfer root"),
        )
        .into_iter()
        .find(|entry| entry.name == "projects")
        .expect("projects directory");
        let prepared = manager
            .prepare_transfer_source(&projects.reference, &AtomicBool::new(false))
            .await
            .expect("plan directory transfer");
        prepared
            .revalidate()
            .await
            .expect("verify source immediately before cleanup");
        server.write_file("/projects/late.txt", vec![1, 2, 3]).await;

        let error = prepared
            .remove_after_verified_transfer()
            .await
            .expect_err("late child must prevent complete source deletion");
        assert!(matches!(error, ExplorerError::PartialCompletion(_)));
        assert!(server.path_exists("/projects").await);
        assert!(server.path_exists("/projects/late.txt").await);
        assert!(!server.path_exists("/projects/notes.txt").await);

        manager.disconnect("test-target").await.expect("disconnect");
        server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn changed_host_key_is_blocked_without_a_routine_accept_prompt() {
        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let manager = Arc::new(SshConnectionManager::default());
        let target = target_for(&server, vec![server.identity_file().to_owned()], true);
        let answers = PromptAnswers {
            accept_host_key: true,
            ..PromptAnswers::default()
        };
        let rejected_answers = PromptAnswers::default();
        let (channel, rejected_events) =
            event_channel(manager.clone(), "trust-rejected", rejected_answers);
        let error = manager
            .connect(target.clone(), "trust-rejected".to_owned(), channel)
            .await
            .expect_err("unknown host key rejection must cancel");
        assert!(matches!(error, ExplorerError::Cancelled));
        assert!(has_event(&rejected_events, "hostKeyPrompt"));
        assert!(std::fs::read_to_string(&target.known_hosts_path)
            .unwrap_or_default()
            .is_empty());

        let (channel, _) = event_channel(manager.clone(), "trust-1", answers.clone());
        manager
            .connect(target.clone(), "trust-1".to_owned(), channel)
            .await
            .expect("establish trusted host key");
        manager.disconnect("test-target").await.expect("disconnect");
        server.rotate_host_key().await;

        let (channel, events) = event_channel(manager.clone(), "trust-2", answers);
        let error = manager
            .connect(target, "trust-2".to_owned(), channel)
            .await
            .expect_err("changed host key must be blocked");
        assert!(matches!(error, ExplorerError::HostKeyFailure(_)));
        assert!(!has_event(&events, "hostKeyPrompt"));
        server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn encrypted_private_key_prompts_without_exposing_the_passphrase() {
        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let manager = Arc::new(SshConnectionManager::default());
        let target = target_for(
            &server,
            vec![server.encrypted_identity_file().to_owned()],
            true,
        );
        let rejected_passphrase = "wrong private key passphrase";
        let rejected_answers = PromptAnswers {
            accept_host_key: true,
            passphrase: Some(rejected_passphrase.to_owned()),
            ..PromptAnswers::default()
        };
        let (channel, _) = event_channel(manager.clone(), "passphrase-rejected", rejected_answers);
        let error = manager
            .connect(target.clone(), "passphrase-rejected".to_owned(), channel)
            .await
            .expect_err("wrong private-key passphrase must fail");
        assert!(matches!(error, ExplorerError::AuthenticationFailed(_)));
        assert!(!error.to_string().contains(rejected_passphrase));

        let answers = PromptAnswers {
            accept_host_key: true,
            passphrase: Some(server.passphrase().to_owned()),
            ..PromptAnswers::default()
        };
        let (channel, events) = event_channel(manager.clone(), "passphrase", answers);
        manager
            .connect(target, "passphrase".to_owned(), channel)
            .await
            .expect("unlock encrypted identity and connect");
        let serialized_events = serde_json::to_string(&*events.lock().expect("passphrase events"))
            .expect("serialize events");
        assert!(serialized_events.contains("passphrase"));
        assert!(serialized_events.contains("\"secret\":true"));
        assert!(!serialized_events.contains(server.passphrase()));
        manager.disconnect("test-target").await.expect("disconnect");
        server.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ssh_agent_identity_authenticates_without_a_secret_prompt() {
        let _environment_lock = SSH_AGENT_ENV_LOCK.lock().await;
        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let agent = server.start_agent().await;
        let environment =
            EnvironmentVariableGuard::set("SSH_AUTH_SOCK", agent.socket_path().as_os_str());
        let manager = Arc::new(SshConnectionManager::default());
        let target = target_for(&server, Vec::new(), false);
        let answers = PromptAnswers {
            accept_host_key: true,
            ..PromptAnswers::default()
        };
        let (channel, events) = event_channel(manager.clone(), "agent", answers);
        manager
            .connect(target, "agent".to_owned(), channel)
            .await
            .expect("authenticate with the disposable SSH agent");
        assert!(!has_event(&events, "authenticationPrompt"));
        manager.disconnect("test-target").await.expect("disconnect");
        drop(environment);
        agent.shutdown().await;
        server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn password_and_keyboard_interactive_authenticate_without_secret_leakage() {
        let password_server = TestSshServer::start(TestAuthMode::Password).await;
        let password_manager = Arc::new(SshConnectionManager::default());
        let password_target = target_for(&password_server, Vec::new(), true);
        let answers = PromptAnswers {
            accept_host_key: true,
            password: Some(password_server.password().to_owned()),
            ..PromptAnswers::default()
        };
        let (channel, events) = event_channel(password_manager.clone(), "password-ok", answers);
        password_manager
            .connect(password_target.clone(), "password-ok".to_owned(), channel)
            .await
            .expect("password authentication");
        assert!(
            !serde_json::to_string(&*events.lock().expect("password events"))
                .expect("serialize password events")
                .contains(password_server.password())
        );
        password_manager
            .disconnect("test-target")
            .await
            .expect("disconnect");

        let rejected_secret = "definitely-wrong-secret";
        let answers = PromptAnswers {
            accept_host_key: true,
            password: Some(rejected_secret.to_owned()),
            ..PromptAnswers::default()
        };
        let (channel, _) = event_channel(password_manager.clone(), "password-bad", answers);
        let error = password_manager
            .connect(password_target, "password-bad".to_owned(), channel)
            .await
            .expect_err("wrong password must fail");
        assert!(matches!(error, ExplorerError::AuthenticationFailed(_)));
        assert!(!error.to_string().contains(rejected_secret));
        password_server.shutdown().await;

        let challenge_server = TestSshServer::start(TestAuthMode::KeyboardInteractive).await;
        let challenge_manager = Arc::new(SshConnectionManager::default());
        let challenge_target = target_for(&challenge_server, Vec::new(), true);
        let answers = PromptAnswers {
            accept_host_key: true,
            keyboard_interactive: Some(challenge_server.challenge_answer().to_owned()),
            ..PromptAnswers::default()
        };
        let (channel, events) = event_channel(challenge_manager.clone(), "challenge", answers);
        challenge_manager
            .connect(challenge_target, "challenge".to_owned(), channel)
            .await
            .expect("keyboard-interactive authentication");
        assert!(events
            .lock()
            .expect("challenge events")
            .iter()
            .any(|event| {
                event.get("event").and_then(Value::as_str) == Some("authenticationPrompt")
                    && event.get("kind").and_then(Value::as_str) == Some("keyboardInteractive")
            }));
        challenge_manager
            .disconnect("test-target")
            .await
            .expect("disconnect");
        challenge_server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_sftp_rename_move_and_conflict_preserve_remote_entries_and_identity() {
        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let manager = Arc::new(SshConnectionManager::default());
        let target = target_for(&server, vec![server.identity_file().to_owned()], true);
        let answers = PromptAnswers {
            accept_host_key: true,
            ..PromptAnswers::default()
        };
        let (channel, _) = event_channel(manager.clone(), "mutations", answers);
        let location = manager
            .connect(target, "mutations".to_owned(), channel)
            .await
            .expect("connect to mutable SFTP fixture");
        assert_eq!(location.root.capabilities, DirectoryCapabilitiesDto::SFTP);

        let root_events = listing_events(&manager, &location.id, &location.root.id)
            .await
            .expect("list remote root");
        let root_entries = listed_entries(&root_events);
        let readme = root_entries
            .iter()
            .find(|entry| entry.name == "README.md")
            .expect("README entry")
            .clone();
        let projects = root_entries
            .iter()
            .find(|entry| entry.name == "projects")
            .and_then(|entry| entry.directory.clone())
            .expect("projects directory");
        assert_eq!(readme.capabilities, EntryCapabilitiesDto::SFTP);

        let renamed = manager
            .rename_entry(&readme.reference, "notes.txt", &AtomicBool::new(false))
            .await
            .expect("rename remote file");
        assert_eq!(renamed.reference.id, readme.reference.id);
        assert!(server.path_exists("/notes.txt").await);
        assert!(!server.path_exists("/README.md").await);

        let project_events = listing_events(&manager, &location.id, &projects.id)
            .await
            .expect("list projects");
        let project_note = listed_entries(&project_events)
            .into_iter()
            .find(|entry| entry.name == "notes.txt")
            .expect("project note");
        assert!(matches!(
            manager
                .move_entry(
                    &project_note.reference,
                    &location.root,
                    RemoteMoveConflictPolicy::Fail,
                    &AtomicBool::new(false),
                )
                .await,
            Err(ExplorerError::Conflict)
        ));
        let moved = manager
            .move_entry(
                &project_note.reference,
                &location.root,
                RemoteMoveConflictPolicy::KeepBoth,
                &AtomicBool::new(false),
            )
            .await
            .expect("keep both on remote conflict");
        assert_eq!(moved.entry.reference.id, project_note.reference.id);
        assert_eq!(moved.entry.name, "notes copy.txt");
        assert!(server.path_exists("/notes.txt").await);
        assert!(server.path_exists("/notes copy.txt").await);
        assert!(!server.path_exists("/projects/notes.txt").await);

        manager.disconnect("test-target").await.expect("disconnect");
        server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_sftp_recursive_delete_is_bounded_symlink_safe_and_permission_aware() {
        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let manager = Arc::new(SshConnectionManager::default());
        let target = target_for(&server, vec![server.identity_file().to_owned()], true);
        let answers = PromptAnswers {
            accept_host_key: true,
            ..PromptAnswers::default()
        };
        let (channel, _) = event_channel(manager.clone(), "delete", answers);
        let location = manager
            .connect(target, "delete".to_owned(), channel)
            .await
            .expect("connect to mutable SFTP fixture");
        let root_entries = listed_entries(
            &listing_events(&manager, &location.id, &location.root.id)
                .await
                .expect("list remote root"),
        );
        let symlink = root_entries
            .iter()
            .find(|entry| entry.name == "project-link")
            .expect("symlink")
            .clone();
        let projects = root_entries
            .iter()
            .find(|entry| entry.name == "projects")
            .expect("projects")
            .clone();
        let locked = root_entries
            .iter()
            .find(|entry| entry.name == "locked.txt")
            .expect("locked file")
            .clone();
        let partial = root_entries
            .iter()
            .find(|entry| entry.name == "partial")
            .expect("partially removable directory")
            .clone();

        manager
            .permanently_delete_entry(&symlink.reference, &AtomicBool::new(false), |_, _| Ok(()))
            .await
            .expect("delete symlink entry");
        assert!(!server.path_exists("/project-link").await);
        assert!(server.path_exists("/projects/notes.txt").await);

        let progress = Arc::new(Mutex::new(Vec::new()));
        let captured_progress = progress.clone();
        let removed = manager
            .permanently_delete_entry(
                &projects.reference,
                &AtomicBool::new(false),
                move |completed, total| {
                    captured_progress
                        .lock()
                        .map_err(|_| ExplorerError::StateUnavailable)?
                        .push((completed, total));
                    Ok(())
                },
            )
            .await
            .expect("delete remote directory recursively");
        assert!(removed
            .invalidated_entry_ids
            .contains(&projects.reference.id));
        assert!(!server.path_exists("/projects").await);
        assert_eq!(
            progress.lock().expect("progress").last().copied(),
            Some((3, 3))
        );

        let error = manager
            .permanently_delete_entry(&locked.reference, &AtomicBool::new(false), |_, _| Ok(()))
            .await
            .expect_err("permission denied delete");
        assert!(matches!(
            error,
            ExplorerError::Io {
                kind: std::io::ErrorKind::PermissionDenied,
                ..
            }
        ));
        assert!(server.path_exists("/locked.txt").await);

        let partial_error = manager
            .permanently_delete_entry(&partial.reference, &AtomicBool::new(false), |_, _| Ok(()))
            .await
            .expect_err("partial deletion must be reported precisely");
        assert!(matches!(partial_error, ExplorerError::PartialCompletion(_)));
        assert!(!server.path_exists("/partial/a.txt").await);
        assert!(server.path_exists("/partial/locked.txt").await);
        assert!(server.path_exists("/partial").await);

        manager.disconnect("test-target").await.expect("disconnect");
        server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn disconnect_during_remote_mutation_reports_uncertain_without_retrying() {
        let server = TestSshServer::start_with_delays(
            TestAuthMode::PublicKey,
            true,
            Duration::ZERO,
            Duration::from_millis(500),
        )
        .await;
        let manager = Arc::new(SshConnectionManager::default());
        let target = target_for(&server, vec![server.identity_file().to_owned()], true);
        let answers = PromptAnswers {
            accept_host_key: true,
            ..PromptAnswers::default()
        };
        let (channel, _) = event_channel(manager.clone(), "uncertain", answers);
        let location = manager
            .connect(target, "uncertain".to_owned(), channel)
            .await
            .expect("connect to delayed SFTP fixture");
        let readme = listed_entries(
            &listing_events(&manager, &location.id, &location.root.id)
                .await
                .expect("list remote root"),
        )
        .into_iter()
        .find(|entry| entry.name == "README.md")
        .expect("README entry");
        manager
            .active_session(&location.id)
            .expect("active session")
            .sftp
            .set_timeout(1);

        let mutation_manager = manager.clone();
        let mutation = tokio::spawn(async move {
            mutation_manager
                .rename_entry(
                    &readme.reference,
                    "renamed-after-drop.md",
                    &AtomicBool::new(false),
                )
                .await
        });
        server.wait_for_mutation().await;
        server.disconnect_clients().await;
        let error = mutation
            .await
            .expect("mutation task")
            .expect_err("disconnect must make outcome uncertain");
        assert!(matches!(error, ExplorerError::OutcomeUncertain(_)));
        assert!(manager.locations().is_empty());

        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(
            server.path_exists("/README.md").await
                || server.path_exists("/renamed-after-drop.md").await
        );
        server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn timeout_during_remote_mutation_reports_uncertain_and_marks_the_session_offline() {
        let server = TestSshServer::start_with_delays(
            TestAuthMode::PublicKey,
            true,
            Duration::ZERO,
            Duration::from_millis(1_500),
        )
        .await;
        let manager = Arc::new(SshConnectionManager::default());
        let target = target_for(&server, vec![server.identity_file().to_owned()], true);
        let answers = PromptAnswers {
            accept_host_key: true,
            ..PromptAnswers::default()
        };
        let (channel, _) = event_channel(manager.clone(), "timeout", answers);
        let location = manager
            .connect(target, "timeout".to_owned(), channel)
            .await
            .expect("connect to delayed SFTP fixture");
        let readme = listed_entries(
            &listing_events(&manager, &location.id, &location.root.id)
                .await
                .expect("list remote root"),
        )
        .into_iter()
        .find(|entry| entry.name == "README.md")
        .expect("README entry");
        manager
            .active_session(&location.id)
            .expect("active session")
            .sftp
            .set_timeout(1);

        let error = manager
            .rename_entry(
                &readme.reference,
                "renamed-after-timeout.md",
                &AtomicBool::new(false),
            )
            .await
            .expect_err("timeout must make outcome uncertain");
        assert!(matches!(error, ExplorerError::OutcomeUncertain(_)));
        assert!(manager.locations().is_empty());

        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(
            server.path_exists("/README.md").await
                || server.path_exists("/renamed-after-timeout.md").await
        );
        server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn missing_sftp_is_actionable_and_slow_listing_is_cancellable() {
        let unsupported_server =
            TestSshServer::start_with_options(TestAuthMode::PublicKey, false, Duration::ZERO).await;
        let manager = Arc::new(SshConnectionManager::default());
        let target = target_for(
            &unsupported_server,
            vec![unsupported_server.identity_file().to_owned()],
            true,
        );
        let answers = PromptAnswers {
            accept_host_key: true,
            ..PromptAnswers::default()
        };
        let (channel, _) = event_channel(manager.clone(), "no-sftp", answers);
        let error = manager
            .connect(target, "no-sftp".to_owned(), channel)
            .await
            .expect_err("server without SFTP must fail");
        assert!(matches!(error, ExplorerError::Unsupported(_)));
        unsupported_server.shutdown().await;

        let slow_server = TestSshServer::start_with_options(
            TestAuthMode::PublicKey,
            true,
            Duration::from_secs(5),
        )
        .await;
        let manager = Arc::new(SshConnectionManager::default());
        let target = target_for(
            &slow_server,
            vec![slow_server.identity_file().to_owned()],
            true,
        );
        let answers = PromptAnswers {
            accept_host_key: true,
            ..PromptAnswers::default()
        };
        let (channel, _) = event_channel(manager.clone(), "slow-connect", answers);
        let location = manager
            .connect(target, "slow-connect".to_owned(), channel)
            .await
            .expect("connect to slow SFTP fixture");
        let root = listing_events(&manager, &location.id, &location.root.id)
            .await
            .expect("list root");
        let slow = listed_entries(&root)
            .into_iter()
            .find(|entry| entry.name == "slow")
            .and_then(|entry| entry.directory)
            .expect("slow directory");
        let cancelled = Arc::new(AtomicBool::new(false));
        let task_manager = manager.clone();
        let task_cancelled = cancelled.clone();
        let location_id = location.id.clone();
        let slow_id = slow.id.clone();
        let listing = tokio::spawn(async move {
            task_manager
                .list_directory(&location_id, &slow_id, &task_cancelled, |_| Ok(()))
                .await
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancelled.store(true, Ordering::SeqCst);
        let result = tokio::time::timeout(Duration::from_secs(1), listing)
            .await
            .expect("cancellation must not wait for the server delay")
            .expect("listing task");
        assert!(matches!(result, Err(ExplorerError::Cancelled)));
        manager.disconnect("test-target").await.expect("disconnect");
        slow_server.shutdown().await;
    }
}
