use std::{
    collections::HashMap,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use russh::{
    client::{self, AuthResult, Handle, KeyboardInteractiveAuthResponse},
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
        LISTING_BATCH_SIZE, PROMPT_TIMEOUT,
    },
    ssh_targets::{ResolvedSshTarget, SshTargetSummaryDto},
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
    paths: RemotePathRegistry,
    handle: AsyncMutex<Handle<HostKeyHandler>>,
    sftp: Arc<SftpSession>,
}

impl SshSession {
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

        let read_dir = self
            .sftp
            .read_dir(path.clone())
            .await
            .map_err(map_sftp_error)?;
        let mut batch = Vec::with_capacity(LISTING_BATCH_SIZE);
        let mut first_batch = true;
        for entry in read_dir {
            ensure_not_cancelled(cancelled)?;
            let name = entry.file_name();
            let entry_path = entry.path();
            let metadata = entry.metadata();
            let is_symlink = metadata.is_symlink();
            let is_directory = if is_symlink {
                self.sftp
                    .metadata(entry_path.clone())
                    .await
                    .map(|metadata| metadata.is_dir())
                    .unwrap_or(false)
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
        if let Some(session) = self
            .sessions
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .get(&target.id)
            .cloned()
        {
            return Ok(session.location());
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
        let handler = HostKeyHandler {
            request_id: request_id.to_owned(),
            target: target.clone(),
            prompts: self.prompts.clone(),
            events: events.clone(),
        };
        let config = Arc::new(client::Config::default());
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
        channel.request_subsystem(true, "sftp").await?;
        let sftp = Arc::new(
            SftpSession::new(channel.into_stream())
                .await
                .map_err(|error| {
                    ExplorerError::Unsupported(format!(
                        "The SSH server did not provide a usable SFTP subsystem: {error}"
                    ))
                })?,
        );
        sftp.set_timeout(30);
        let initial_path = sftp
            .canonicalize(target.initial_path.clone())
            .await
            .map_err(map_sftp_error)?;
        let location_id = format!("ssh:{}", target.id);
        let paths = RemotePathRegistry::default();
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
        });
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
        let Some(session) = self
            .sessions
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .remove(target_id)
        else {
            return Ok(());
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
        session.list_directory(directory_id, cancelled, emit).await
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
            _ => {
                ExplorerError::Unexpected(format!("The SFTP server returned an error: {status:?}"))
            }
        },
        SftpError::Timeout => ExplorerError::Offline("The SFTP request timed out.".to_owned()),
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
}
