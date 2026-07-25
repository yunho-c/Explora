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

use russh::{
    client::{self, AuthResult, DisconnectReason, Handle, KeyboardInteractiveAuthResponse},
    keys::{self, agent::client::AgentClient, HashAlg, PrivateKeyWithHashAlg, PublicKey},
    MethodKind,
};
use russh_sftp::{
    client::{error::Error as SftpError, SftpSession},
    protocol::StatusCode,
};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tokio::sync::{oneshot, Mutex as AsyncMutex};
use uuid::Uuid;
use zeroize::Zeroize;

use crate::{
    filesystem::{
        BreadcrumbSegmentDto, DirectoryListingEvent, DirectoryRefDto, EntryRefDto, ExplorerError,
        FileEntrySummaryDto, LocationRole, LocationSummaryDto, CONNECTION_TIMEOUT,
        LISTING_BATCH_SIZE, PROMPT_TIMEOUT, SFTP_REQUEST_TIMEOUT_SECONDS, SSH_KEEPALIVE_INTERVAL,
        SSH_KEEPALIVE_MAX,
    },
    ssh_targets::{location_id, ResolvedSshTarget, SshTargetSummaryDto},
    terminal::types::TerminalSizeDto,
};

pub(crate) struct OpenedSshTerminal {
    pub location_id: String,
    pub title: String,
    pub context_label: String,
    pub channel: russh::Channel<client::Msg>,
}

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
}

struct SshSession {
    target: ResolvedSshTarget,
    location_id: String,
    root: DirectoryRefDto,
    paths: Arc<RemotePathRegistry>,
    handle: AsyncMutex<Handle<HostKeyHandler>>,
    sftp: Arc<SftpSession>,
    lifecycle: Arc<AtomicU8>,
    events: Arc<Channel<SshConnectionEventDto>>,
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
        Ok(DirectoryRefDto {
            id: self.paths.register(path.to_owned())?,
            location_id: self.location_id.clone(),
            name: name.unwrap_or_else(|| remote_name(path)),
            display_path: format!("{}:{path}", self.target.name),
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
        let directory = self.directory_ref(&path, None)?;
        let parent = remote_parent(&path)
            .map(|parent| self.directory_ref(&parent, None))
            .transpose()?;
        let breadcrumbs = remote_breadcrumbs(self, &path)?;
        emit(DirectoryListingEvent::Started {
            directory,
            parent,
            breadcrumbs,
        })?;

        let read_dir = tokio::select! {
            result = self.sftp.read_dir(path.clone()) => result.map_err(map_sftp_error)?,
            () = wait_for_cancellation(cancelled) => return Err(ExplorerError::Cancelled),
        };
        let mut batch = Vec::with_capacity(LISTING_BATCH_SIZE);
        let mut first_batch = true;
        for entry in read_dir {
            ensure_not_cancelled(cancelled)?;
            let name = entry.file_name();
            let entry_path = entry.path();
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
            let directory = is_directory
                .then(|| self.directory_ref(&entry_path, Some(name.clone())))
                .transpose()?;
            batch.push(FileEntrySummaryDto {
                reference: EntryRefDto {
                    id: self.paths.register(entry_path.clone())?,
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
        emit(DirectoryListingEvent::Complete { skipped_entries: 0 })
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

    pub(crate) async fn open_terminal(
        &self,
        location_id: &str,
        size: TerminalSizeDto,
    ) -> Result<OpenedSshTerminal, ExplorerError> {
        let size = size.validate()?;
        let session = self
            .sessions
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .values()
            .find(|session| session.location_id == location_id)
            .cloned()
            .ok_or_else(|| {
                ExplorerError::Offline(
                    "Reconnect this SSH location before opening a terminal.".to_owned(),
                )
            })?;
        if session.lifecycle.load(Ordering::SeqCst) != SESSION_ACTIVE {
            return Err(ExplorerError::Offline(
                "Reconnect this SSH location before opening a terminal.".to_owned(),
            ));
        }

        let handle = session.handle.lock().await;
        let channel = tokio::time::timeout(CONNECTION_TIMEOUT, handle.channel_open_session())
            .await
            .map_err(|_| {
                ExplorerError::Offline(
                    "The SSH server timed out while opening a terminal channel.".to_owned(),
                )
            })?
            .map_err(|_| {
                ExplorerError::Offline("The SSH server did not open a terminal channel.".to_owned())
            })?;
        drop(handle);
        tokio::time::timeout(
            CONNECTION_TIMEOUT,
            channel.request_pty(
                true,
                "xterm-256color",
                u32::from(size.columns),
                u32::from(size.rows),
                u32::from(size.pixel_width.unwrap_or_default()),
                u32::from(size.pixel_height.unwrap_or_default()),
                &[],
            ),
        )
        .await
        .map_err(|_| {
            ExplorerError::Offline(
                "The SSH server timed out while opening a pseudo-terminal.".to_owned(),
            )
        })?
        .map_err(|_| {
            ExplorerError::Unsupported(
                "The SSH server did not accept a pseudo-terminal request.".to_owned(),
            )
        })?;
        let _ = channel.set_env(false, "COLORTERM", "truecolor").await;
        tokio::time::timeout(CONNECTION_TIMEOUT, channel.request_shell(true))
            .await
            .map_err(|_| {
                ExplorerError::Offline(
                    "The SSH server timed out while starting its default shell.".to_owned(),
                )
            })?
            .map_err(|_| {
                ExplorerError::Unsupported(
                    "The SSH server did not start the account's default shell.".to_owned(),
                )
            })?;

        Ok(OpenedSshTerminal {
            location_id: session.location_id.clone(),
            title: session.target.name.clone(),
            context_label: format!("{} · server home", session.target.name),
            channel,
        })
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
        let root = DirectoryRefDto {
            id: paths.register(initial_path.clone())?,
            location_id: location_id.clone(),
            name: target.name.clone(),
            display_path: format!("{}:{initial_path}", target.name),
        };
        let target_id = target.id.clone();
        let session = Arc::new(SshSession {
            target,
            location_id,
            root,
            paths,
            handle: AsyncMutex::new(handle),
            sftp,
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
    use russh::ChannelMsg;
    use std::{sync::mpsc, time::Duration};

    use serde_json::Value;
    use tauri::ipc::{InvokeResponseBody, Response};

    use crate::ssh_test_server::{TestAuthMode, TestSshServer};
    use crate::terminal::{
        types::{TerminalCloseReason, TERMINAL_OUTPUT_FRAME_HEADER_BYTES},
        SshTerminalLaunch, TerminalCoordinator,
    };

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
    async fn verified_connection_opens_an_interactive_pty_channel() {
        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let manager = Arc::new(SshConnectionManager::default());
        let target = target_for(&server, vec![server.identity_file().to_owned()], true);
        let answers = PromptAnswers {
            accept_host_key: true,
            ..PromptAnswers::default()
        };
        let (events, _) = event_channel(manager.clone(), "terminal-connect", answers);
        let location = manager
            .connect(target, "terminal-connect".to_owned(), events)
            .await
            .expect("connect through verified SSH");
        let opened = manager
            .open_terminal(
                &location.id,
                TerminalSizeDto {
                    columns: 80,
                    rows: 24,
                    pixel_width: None,
                    pixel_height: None,
                },
            )
            .await
            .expect("open remote PTY");
        assert_eq!(opened.location_id, location.id);
        assert_eq!(opened.title, "Test server");
        assert_eq!(opened.context_label, "Test server · server home");

        let mut channel = opened.channel;
        let ready = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(ChannelMsg::Data { data }) = channel.wait().await {
                    break data;
                }
            }
        })
        .await
        .expect("remote shell banner");
        assert_eq!(ready.as_ref(), b"remote-shell-ready\r\n");

        channel
            .window_change(120, 40, 0, 0)
            .await
            .expect("resize remote PTY");
        channel
            .data_bytes(b"exit\n".to_vec())
            .await
            .expect("write remote PTY");
        let (mut output, mut exit_code) = (Vec::new(), None);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match channel.wait().await {
                    Some(ChannelMsg::Data { data }) => output.extend_from_slice(&data),
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        exit_code = Some(exit_status);
                    }
                    Some(ChannelMsg::Close) | None => break,
                    Some(_) => {}
                }
            }
        })
        .await
        .expect("remote shell exit");
        assert!(output
            .windows(b"remote-resize-120x40".len())
            .any(|part| part == b"remote-resize-120x40"));
        assert!(output.windows(4).any(|part| part == b"exit"));
        assert_eq!(exit_code, Some(17));

        manager.disconnect("test-target").await.expect("disconnect");
        server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remote_terminal_coordinator_streams_and_accounts_for_output() {
        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let manager = Arc::new(SshConnectionManager::default());
        let target = target_for(&server, vec![server.identity_file().to_owned()], true);
        let answers = PromptAnswers {
            accept_host_key: true,
            ..PromptAnswers::default()
        };
        let (events, _) = event_channel(manager.clone(), "coordinator-connect", answers);
        let location = manager
            .connect(target, "coordinator-connect".to_owned(), events)
            .await
            .expect("connect through verified SSH");
        let opened = manager
            .open_terminal(
                &location.id,
                TerminalSizeDto {
                    columns: 80,
                    rows: 24,
                    pixel_width: None,
                    pixel_height: None,
                },
            )
            .await
            .expect("open remote PTY");
        let (event_sender, event_receiver) = mpsc::channel();
        let terminal_channel: Channel<Response> = Channel::new(move |body| {
            let _ = event_sender.send(body);
            Ok(())
        });
        let coordinator = TerminalCoordinator::default();
        let summary = coordinator
            .create_ssh(SshTerminalLaunch {
                window_label: "main",
                location_id: &opened.location_id,
                title: &opened.title,
                context_label: &opened.context_label,
                channel: opened.channel,
                on_event: terminal_channel,
            })
            .expect("register remote terminal");
        assert_eq!(
            summary.kind,
            crate::terminal::types::TerminalSessionKind::Ssh
        );

        coordinator
            .resize(
                "main",
                &summary.id,
                TerminalSizeDto {
                    columns: 120,
                    rows: 40,
                    pixel_width: Some(1_200),
                    pixel_height: Some(800),
                },
            )
            .expect("queue remote resize");
        coordinator
            .write("main", &summary.id, 0, b"exit\n")
            .expect("queue remote input");

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut output = Vec::new();
        let mut started = false;
        let mut exit_code = None;
        while std::time::Instant::now() < deadline && exit_code.is_none() {
            match event_receiver
                .recv_timeout(Duration::from_millis(100))
                .expect("remote terminal event")
            {
                InvokeResponseBody::Json(json) => {
                    let event: Value = serde_json::from_str(&json).expect("control event");
                    match event["event"].as_str() {
                        Some("started") => started = true,
                        Some("exited") => exit_code = event["exitCode"].as_u64(),
                        other => panic!("unexpected remote control event: {other:?}"),
                    }
                }
                InvokeResponseBody::Raw(frame) => {
                    let sequence =
                        u64::from_be_bytes(frame[2..10].try_into().expect("sequence bytes"));
                    output.extend_from_slice(&frame[TERMINAL_OUTPUT_FRAME_HEADER_BYTES..]);
                    coordinator
                        .acknowledge("main", &summary.id, sequence)
                        .expect("acknowledge remote output");
                }
            }
        }
        assert!(started);
        assert_eq!(exit_code, Some(17));
        assert!(output
            .windows(b"remote-shell-ready".len())
            .any(|part| part == b"remote-shell-ready"));
        assert!(output
            .windows(b"remote-resize-120x40".len())
            .any(|part| part == b"remote-resize-120x40"));
        assert!(output.windows(4).any(|part| part == b"exit"));
        assert!(matches!(
            coordinator.write("other-window", &summary.id, 1, b"x"),
            Err(ExplorerError::InvalidReference)
        ));
        coordinator
            .close("main", &summary.id, TerminalCloseReason::User)
            .expect("close completed remote terminal");
        manager.disconnect("test-target").await.expect("disconnect");
        server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ssh_disconnect_ends_remote_terminal_without_replay() {
        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let manager = Arc::new(SshConnectionManager::default());
        let target = target_for(&server, vec![server.identity_file().to_owned()], true);
        let answers = PromptAnswers {
            accept_host_key: true,
            ..PromptAnswers::default()
        };
        let (connection_channel, connection_events) =
            event_channel(manager.clone(), "disconnect-terminal", answers);
        let location = manager
            .connect(target, "disconnect-terminal".to_owned(), connection_channel)
            .await
            .expect("connect through verified SSH");
        let opened = manager
            .open_terminal(
                &location.id,
                TerminalSizeDto {
                    columns: 80,
                    rows: 24,
                    pixel_width: None,
                    pixel_height: None,
                },
            )
            .await
            .expect("open remote PTY");
        let (event_sender, event_receiver) = mpsc::channel();
        let terminal_channel: Channel<Response> = Channel::new(move |body| {
            let _ = event_sender.send(body);
            Ok(())
        });
        let coordinator = TerminalCoordinator::default();
        let summary = coordinator
            .create_ssh(SshTerminalLaunch {
                window_label: "main",
                location_id: &opened.location_id,
                title: &opened.title,
                context_label: &opened.context_label,
                channel: opened.channel,
                on_event: terminal_channel,
            })
            .expect("register remote terminal");

        server.disconnect_clients().await;
        wait_for_event(&connection_events, "disconnected").await;
        let reason = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let body = event_receiver
                    .recv_timeout(Duration::from_millis(100))
                    .expect("remote disconnect event");
                match body {
                    InvokeResponseBody::Json(json) => {
                        let event: Value = serde_json::from_str(&json).expect("control event");
                        if event["event"] == "exited" {
                            break event["reason"]
                                .as_str()
                                .expect("remote exit reason")
                                .to_owned();
                        }
                    }
                    InvokeResponseBody::Raw(frame) => {
                        let sequence =
                            u64::from_be_bytes(frame[2..10].try_into().expect("sequence bytes"));
                        coordinator
                            .acknowledge("main", &summary.id, sequence)
                            .expect("acknowledge remote output");
                    }
                }
            }
        })
        .await
        .expect("terminal disconnect lifecycle");
        assert_eq!(reason, "transportClosed");
        assert!(matches!(
            coordinator.write("main", &summary.id, 0, b"not replayed"),
            Err(ExplorerError::InvalidReference)
        ));

        coordinator
            .close("main", &summary.id, TerminalCloseReason::User)
            .expect("close disconnected terminal");
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
