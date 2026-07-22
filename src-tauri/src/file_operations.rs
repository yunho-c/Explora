use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use uuid::Uuid;

use crate::{
    filesystem::{
        DirectoryRefDto, EntryRefDto, ExplorerError, ExplorerErrorDto, FileEntrySummaryDto,
        PROMPT_TIMEOUT,
    },
    local_filesystem::{
        LocalFilesystem, LocalMoveConflictPolicy, MovedLocalEntry, RemovedLocalEntry,
    },
    platform_trash::{PlatformTrash, SystemPlatformTrash},
    ssh::{MovedRemoteEntry, RemoteMoveConflictPolicy, RemovedRemoteEntry, SshConnectionManager},
};

const MAX_OPERATION_SOURCES: usize = 1;
const PROMPT_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileOperationRequestDto {
    pub sources: Vec<EntryRefDto>,
    pub action: FileOperationActionDto,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum FileOperationActionDto {
    Rename { new_name: String },
    Move { destination: DirectoryRefDto },
    Trash {},
    DeletePermanently {},
}

impl FileOperationActionDto {
    fn kind(&self) -> FileOperationKindDto {
        match self {
            Self::Rename { .. } => FileOperationKindDto::Rename,
            Self::Move { .. } => FileOperationKindDto::Move,
            Self::Trash {} => FileOperationKindDto::Trash,
            Self::DeletePermanently {} => FileOperationKindDto::DeletePermanently,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FileOperationKindDto {
    Rename,
    Move,
    Trash,
    DeletePermanently,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FileOperationPromptResponseDto {
    Confirm,
    KeepBoth,
    Skip,
    Cancel,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FileOperationPromptDto {
    PermanentDelete {
        id: String,
        title: String,
        message: String,
        target_name: String,
        location_name: String,
        confirm_label: &'static str,
    },
    MoveConflict {
        id: String,
        title: String,
        message: String,
        target_name: String,
        destination_name: String,
        decisions: Vec<FileOperationPromptResponseDto>,
    },
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FileOperationOutcomeDto {
    Renamed {
        entry: Box<FileEntrySummaryDto>,
    },
    Moved {
        entry: Box<FileEntrySummaryDto>,
        source_parent: DirectoryRefDto,
        destination: DirectoryRefDto,
        rebased_entry_ids: Vec<String>,
    },
    MoveSkipped {
        entry: EntryRefDto,
        name: String,
    },
    Trashed {
        entry: EntryRefDto,
        name: String,
        invalidated_entry_ids: Vec<String>,
    },
    DeletedPermanently {
        entry: EntryRefDto,
        name: String,
        invalidated_entry_ids: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(
    tag = "event",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FileOperationEventDto {
    Queued {
        operation_id: String,
        sequence: u64,
        action: FileOperationKindDto,
        completed_items: u64,
        total_items: u64,
    },
    Running {
        operation_id: String,
        sequence: u64,
        action: FileOperationKindDto,
        completed_items: u64,
        total_items: u64,
    },
    AwaitingConfirmation {
        operation_id: String,
        sequence: u64,
        action: FileOperationKindDto,
        completed_items: u64,
        total_items: u64,
        prompt: FileOperationPromptDto,
    },
    AwaitingConflict {
        operation_id: String,
        sequence: u64,
        action: FileOperationKindDto,
        completed_items: u64,
        total_items: u64,
        prompt: FileOperationPromptDto,
    },
    Completed {
        operation_id: String,
        sequence: u64,
        action: FileOperationKindDto,
        completed_items: u64,
        total_items: u64,
        outcome: FileOperationOutcomeDto,
    },
    Cancelled {
        operation_id: String,
        sequence: u64,
        action: FileOperationKindDto,
        completed_items: u64,
        total_items: u64,
    },
    Failed {
        operation_id: String,
        sequence: u64,
        action: FileOperationKindDto,
        completed_items: u64,
        total_items: u64,
        error: ExplorerErrorDto,
    },
}

struct PendingPrompt {
    id: String,
    allowed_responses: Vec<FileOperationPromptResponseDto>,
    response: mpsc::Sender<FileOperationPromptResponseDto>,
}

#[derive(Default)]
struct ActiveOperation {
    cancelled: AtomicBool,
    prompt: Mutex<Option<PendingPrompt>>,
}

impl ActiveOperation {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    fn ensure_not_cancelled(&self) -> Result<(), ExplorerError> {
        if self.cancelled.load(Ordering::Relaxed) {
            Err(ExplorerError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn begin_prompt(
        &self,
        prompt_id: String,
        allowed_responses: Vec<FileOperationPromptResponseDto>,
    ) -> Result<mpsc::Receiver<FileOperationPromptResponseDto>, ExplorerError> {
        let (sender, receiver) = mpsc::channel();
        {
            let mut prompt = self
                .prompt
                .lock()
                .map_err(|_| ExplorerError::StateUnavailable)?;
            if prompt.is_some() {
                return Err(ExplorerError::StateUnavailable);
            }
            *prompt = Some(PendingPrompt {
                id: prompt_id,
                allowed_responses,
                response: sender,
            });
        }
        Ok(receiver)
    }

    fn await_prompt(
        &self,
        receiver: mpsc::Receiver<FileOperationPromptResponseDto>,
    ) -> Result<FileOperationPromptResponseDto, ExplorerError> {
        let started = Instant::now();
        loop {
            if let Err(error) = self.ensure_not_cancelled() {
                self.clear_prompt();
                return Err(error);
            }
            if started.elapsed() >= PROMPT_TIMEOUT {
                self.clear_prompt();
                return Err(ExplorerError::Cancelled);
            }
            match receiver.recv_timeout(PROMPT_POLL_INTERVAL) {
                Ok(response) => return Ok(response),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.clear_prompt();
                    return Err(ExplorerError::StateUnavailable);
                }
            }
        }
    }

    async fn await_prompt_async(
        &self,
        receiver: mpsc::Receiver<FileOperationPromptResponseDto>,
    ) -> Result<FileOperationPromptResponseDto, ExplorerError> {
        let started = Instant::now();
        loop {
            if let Err(error) = self.ensure_not_cancelled() {
                self.clear_prompt();
                return Err(error);
            }
            if started.elapsed() >= PROMPT_TIMEOUT {
                self.clear_prompt();
                return Err(ExplorerError::Cancelled);
            }
            match receiver.try_recv() {
                Ok(response) => return Ok(response),
                Err(mpsc::TryRecvError::Empty) => {
                    tokio::time::sleep(PROMPT_POLL_INTERVAL).await;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.clear_prompt();
                    return Err(ExplorerError::StateUnavailable);
                }
            }
        }
    }

    fn respond(
        &self,
        prompt_id: &str,
        response: FileOperationPromptResponseDto,
    ) -> Result<(), ExplorerError> {
        let mut pending = self
            .prompt
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        if pending.as_ref().map(|prompt| prompt.id.as_str()) != Some(prompt_id) {
            return Err(ExplorerError::InvalidReference);
        }
        if !pending
            .as_ref()
            .is_some_and(|prompt| prompt.allowed_responses.contains(&response))
        {
            return Err(ExplorerError::InvalidConfiguration(
                "That response is not available for this filesystem decision.".to_owned(),
            ));
        }
        let prompt = pending.take().ok_or(ExplorerError::InvalidReference)?;
        drop(pending);
        prompt
            .response
            .send(response)
            .map_err(|_| ExplorerError::InvalidReference)
    }

    fn clear_prompt(&self) {
        if let Ok(mut prompt) = self.prompt.lock() {
            prompt.take();
        }
    }
}

struct OperationEventEmitter {
    operation_id: String,
    action: FileOperationKindDto,
    sequence: u64,
    completed_items: u64,
    total_items: u64,
    channel: Channel<FileOperationEventDto>,
}

impl OperationEventEmitter {
    fn next_sequence(&mut self) -> u64 {
        let sequence = self.sequence;
        self.sequence = self.sequence.saturating_add(1);
        sequence
    }

    fn running(&mut self) -> Result<(), ExplorerError> {
        let sequence = self.next_sequence();
        self.channel
            .send(FileOperationEventDto::Running {
                operation_id: self.operation_id.clone(),
                sequence,
                action: self.action,
                completed_items: self.completed_items,
                total_items: self.total_items,
            })
            .map_err(|_| ExplorerError::ChannelClosed)
    }

    fn awaiting_confirmation(
        &mut self,
        prompt: FileOperationPromptDto,
    ) -> Result<(), ExplorerError> {
        let sequence = self.next_sequence();
        self.channel
            .send(FileOperationEventDto::AwaitingConfirmation {
                operation_id: self.operation_id.clone(),
                sequence,
                action: self.action,
                completed_items: self.completed_items,
                total_items: self.total_items,
                prompt,
            })
            .map_err(|_| ExplorerError::ChannelClosed)
    }

    fn awaiting_conflict(&mut self, prompt: FileOperationPromptDto) -> Result<(), ExplorerError> {
        let sequence = self.next_sequence();
        self.channel
            .send(FileOperationEventDto::AwaitingConflict {
                operation_id: self.operation_id.clone(),
                sequence,
                action: self.action,
                completed_items: self.completed_items,
                total_items: self.total_items,
                prompt,
            })
            .map_err(|_| ExplorerError::ChannelClosed)
    }

    fn progress(&mut self, completed_items: u64, total_items: u64) -> Result<(), ExplorerError> {
        if total_items == 0 || completed_items > total_items {
            return Err(ExplorerError::StateUnavailable);
        }
        self.completed_items = completed_items;
        self.total_items = total_items;
        self.running()
    }

    fn terminal(&mut self, result: Result<FileOperationOutcomeDto, ExplorerError>) {
        let sequence = self.next_sequence();
        let event = match result {
            Ok(outcome) => FileOperationEventDto::Completed {
                operation_id: self.operation_id.clone(),
                sequence,
                action: self.action,
                completed_items: self.total_items,
                total_items: self.total_items,
                outcome,
            },
            Err(ExplorerError::Cancelled) => FileOperationEventDto::Cancelled {
                operation_id: self.operation_id.clone(),
                sequence,
                action: self.action,
                completed_items: self.completed_items,
                total_items: self.total_items,
            },
            Err(error) => FileOperationEventDto::Failed {
                operation_id: self.operation_id.clone(),
                sequence,
                action: self.action,
                completed_items: self.completed_items,
                total_items: self.total_items,
                error: ExplorerErrorDto::from(error),
            },
        };
        let _ = self.channel.send(event);
    }
}

pub struct FileOperationCoordinator {
    active: Mutex<HashMap<String, Arc<ActiveOperation>>>,
    // The first slices serialize mutations. Later transfer phases can replace
    // this with subtree-aware guards without changing the operation contract.
    execution_guard: Mutex<()>,
    platform_trash: Arc<dyn PlatformTrash>,
}

impl Default for FileOperationCoordinator {
    fn default() -> Self {
        Self::with_platform_trash(Arc::new(SystemPlatformTrash))
    }
}

impl FileOperationCoordinator {
    pub(crate) fn with_platform_trash(platform_trash: Arc<dyn PlatformTrash>) -> Self {
        Self {
            active: Mutex::new(HashMap::new()),
            execution_guard: Mutex::new(()),
            platform_trash,
        }
    }

    #[cfg(test)]
    pub fn start(
        self: &Arc<Self>,
        local: Arc<LocalFilesystem>,
        request: FileOperationRequestDto,
        on_event: Channel<FileOperationEventDto>,
    ) -> Result<String, ExplorerError> {
        self.start_with_backends(
            local,
            Arc::new(SshConnectionManager::default()),
            request,
            on_event,
        )
    }

    pub fn start_with_backends(
        self: &Arc<Self>,
        local: Arc<LocalFilesystem>,
        ssh: Arc<SshConnectionManager>,
        request: FileOperationRequestDto,
        on_event: Channel<FileOperationEventDto>,
    ) -> Result<String, ExplorerError> {
        validate_request(&request)?;
        let action = request.action.kind();
        let operation_id = Uuid::new_v4().to_string();
        let active = Arc::new(ActiveOperation::default());
        self.active
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .insert(operation_id.clone(), active.clone());
        if on_event
            .send(FileOperationEventDto::Queued {
                operation_id: operation_id.clone(),
                sequence: 0,
                action,
                completed_items: 0,
                total_items: 1,
            })
            .is_err()
        {
            self.finish(&operation_id);
            return Err(ExplorerError::ChannelClosed);
        }

        let coordinator = self.clone();
        let task_operation_id = operation_id.clone();
        if request.sources[0].location_id.starts_with("ssh:") {
            tauri::async_runtime::spawn(async move {
                let mut events = OperationEventEmitter {
                    operation_id: task_operation_id.clone(),
                    action,
                    sequence: 1,
                    completed_items: 0,
                    total_items: 1,
                    channel: on_event,
                };
                let result = coordinator
                    .run_remote(&ssh, &request, &active, &mut events)
                    .await;
                active.clear_prompt();
                events.terminal(result);
                coordinator.finish(&task_operation_id);
            });
        } else {
            tauri::async_runtime::spawn_blocking(move || {
                let mut events = OperationEventEmitter {
                    operation_id: task_operation_id.clone(),
                    action,
                    sequence: 1,
                    completed_items: 0,
                    total_items: 1,
                    channel: on_event,
                };
                let result = coordinator.run_local(&local, &request, &active, &mut events);
                active.clear_prompt();
                events.terminal(result);
                coordinator.finish(&task_operation_id);
            });
        }

        Ok(operation_id)
    }

    pub fn cancel(&self, operation_id: &str) -> Result<(), ExplorerError> {
        if let Some(active) = self
            .active
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .get(operation_id)
        {
            active.cancel();
        }
        Ok(())
    }

    pub fn respond(
        &self,
        operation_id: &str,
        prompt_id: &str,
        response: FileOperationPromptResponseDto,
    ) -> Result<(), ExplorerError> {
        self.active
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .get(operation_id)
            .cloned()
            .ok_or(ExplorerError::InvalidReference)?
            .respond(prompt_id, response)
    }

    fn run_local(
        &self,
        local: &LocalFilesystem,
        request: &FileOperationRequestDto,
        active: &ActiveOperation,
        events: &mut OperationEventEmitter,
    ) -> Result<FileOperationOutcomeDto, ExplorerError> {
        active.ensure_not_cancelled()?;
        events.running()?;

        let source = &request.sources[0];
        match &request.action {
            FileOperationActionDto::Rename { new_name } => self
                .with_execution_guard(|| local.rename_entry(source, new_name, &active.cancelled))
                .map(|entry| FileOperationOutcomeDto::Renamed {
                    entry: Box::new(entry),
                }),
            FileOperationActionDto::Move { destination } => {
                match self.with_execution_guard(|| {
                    local.move_entry(
                        source,
                        destination,
                        LocalMoveConflictPolicy::Fail,
                        &active.cancelled,
                    )
                }) {
                    Ok(moved) => Ok(moved_outcome(moved)),
                    Err(ExplorerError::Conflict) => {
                        let (target_name, destination_name) =
                            local.describe_move_conflict(source, destination)?;
                        let prompt_id = Uuid::new_v4().to_string();
                        let decisions = vec![
                            FileOperationPromptResponseDto::KeepBoth,
                            FileOperationPromptResponseDto::Skip,
                            FileOperationPromptResponseDto::Cancel,
                        ];
                        let response = active.begin_prompt(prompt_id.clone(), decisions.clone())?;
                        events.awaiting_conflict(FileOperationPromptDto::MoveConflict {
                            id: prompt_id,
                            title: format!("“{target_name}” already exists"),
                            message: format!(
                                "Choose how to handle the existing item in “{destination_name}”. Nothing will be replaced."
                            ),
                            target_name: target_name.clone(),
                            destination_name,
                            decisions,
                        })?;
                        match active.await_prompt(response)? {
                            FileOperationPromptResponseDto::KeepBoth => {
                                events.running()?;
                                active.ensure_not_cancelled()?;
                                self.with_execution_guard(|| {
                                    local.move_entry(
                                        source,
                                        destination,
                                        LocalMoveConflictPolicy::KeepBoth,
                                        &active.cancelled,
                                    )
                                })
                                .map(moved_outcome)
                            }
                            FileOperationPromptResponseDto::Skip => {
                                events.running()?;
                                Ok(FileOperationOutcomeDto::MoveSkipped {
                                    entry: source.clone(),
                                    name: target_name,
                                })
                            }
                            FileOperationPromptResponseDto::Cancel => Err(ExplorerError::Cancelled),
                            FileOperationPromptResponseDto::Confirm => {
                                Err(ExplorerError::InvalidConfiguration(
                                    "That response is not valid for a move conflict.".to_owned(),
                                ))
                            }
                        }
                    }
                    Err(error) => Err(error),
                }
            }
            FileOperationActionDto::Trash {} => self
                .with_execution_guard(|| {
                    local.trash_entry(source, &active.cancelled, self.platform_trash.as_ref())
                })
                .map(trashed_outcome),
            FileOperationActionDto::DeletePermanently {} => {
                let (target_name, location_name) = local.describe_operation_target(source)?;
                let prompt_id = Uuid::new_v4().to_string();
                let response = active.begin_prompt(
                    prompt_id.clone(),
                    vec![
                        FileOperationPromptResponseDto::Confirm,
                        FileOperationPromptResponseDto::Cancel,
                    ],
                )?;
                events.awaiting_confirmation(FileOperationPromptDto::PermanentDelete {
                    id: prompt_id,
                    title: format!("Delete “{target_name}” permanently?"),
                    message:
                        "This item will be removed immediately and cannot be recovered from Trash."
                            .to_owned(),
                    target_name,
                    location_name,
                    confirm_label: "Delete Permanently",
                })?;
                match active.await_prompt(response)? {
                    FileOperationPromptResponseDto::Confirm => {}
                    FileOperationPromptResponseDto::Cancel => return Err(ExplorerError::Cancelled),
                    FileOperationPromptResponseDto::KeepBoth
                    | FileOperationPromptResponseDto::Skip => {
                        return Err(ExplorerError::InvalidConfiguration(
                            "That response is not valid for permanent deletion.".to_owned(),
                        ));
                    }
                }
                events.running()?;
                active.ensure_not_cancelled()?;
                self.with_execution_guard(|| {
                    local.permanently_delete_entry(source, &active.cancelled)
                })
                .map(deleted_outcome)
            }
        }
    }

    async fn run_remote(
        &self,
        ssh: &SshConnectionManager,
        request: &FileOperationRequestDto,
        active: &ActiveOperation,
        events: &mut OperationEventEmitter,
    ) -> Result<FileOperationOutcomeDto, ExplorerError> {
        active.ensure_not_cancelled()?;
        events.running()?;

        let source = &request.sources[0];
        match &request.action {
            FileOperationActionDto::Rename { new_name } => ssh
                .rename_entry(source, new_name, &active.cancelled)
                .await
                .map(|entry| FileOperationOutcomeDto::Renamed {
                    entry: Box::new(entry),
                }),
            FileOperationActionDto::Move { destination } => {
                match ssh
                    .move_entry(
                        source,
                        destination,
                        RemoteMoveConflictPolicy::Fail,
                        &active.cancelled,
                    )
                    .await
                {
                    Ok(moved) => Ok(moved_remote_outcome(moved)),
                    Err(ExplorerError::Conflict) => {
                        let (target_name, destination_name) =
                            ssh.describe_move_conflict(source, destination).await?;
                        let prompt_id = Uuid::new_v4().to_string();
                        let decisions = vec![
                            FileOperationPromptResponseDto::KeepBoth,
                            FileOperationPromptResponseDto::Skip,
                            FileOperationPromptResponseDto::Cancel,
                        ];
                        let response = active.begin_prompt(prompt_id.clone(), decisions.clone())?;
                        events.awaiting_conflict(FileOperationPromptDto::MoveConflict {
                            id: prompt_id,
                            title: format!("“{target_name}” already exists"),
                            message: format!(
                                "Choose how to handle the existing remote item in “{destination_name}”. Nothing will be replaced."
                            ),
                            target_name: target_name.clone(),
                            destination_name,
                            decisions,
                        })?;
                        match active.await_prompt_async(response).await? {
                            FileOperationPromptResponseDto::KeepBoth => {
                                events.running()?;
                                active.ensure_not_cancelled()?;
                                ssh.move_entry(
                                    source,
                                    destination,
                                    RemoteMoveConflictPolicy::KeepBoth,
                                    &active.cancelled,
                                )
                                .await
                                .map(moved_remote_outcome)
                            }
                            FileOperationPromptResponseDto::Skip => {
                                events.running()?;
                                Ok(FileOperationOutcomeDto::MoveSkipped {
                                    entry: source.clone(),
                                    name: target_name,
                                })
                            }
                            FileOperationPromptResponseDto::Cancel => Err(ExplorerError::Cancelled),
                            FileOperationPromptResponseDto::Confirm => {
                                Err(ExplorerError::InvalidConfiguration(
                                    "That response is not valid for a move conflict.".to_owned(),
                                ))
                            }
                        }
                    }
                    Err(error) => Err(error),
                }
            }
            FileOperationActionDto::Trash {} => Err(ExplorerError::Unsupported(
                "Remote items cannot be moved to the local operating-system Trash.".to_owned(),
            )),
            FileOperationActionDto::DeletePermanently {} => {
                let (target_name, location_name) = ssh.describe_operation_target(source).await?;
                let prompt_id = Uuid::new_v4().to_string();
                let response = active.begin_prompt(
                    prompt_id.clone(),
                    vec![
                        FileOperationPromptResponseDto::Confirm,
                        FileOperationPromptResponseDto::Cancel,
                    ],
                )?;
                events.awaiting_confirmation(FileOperationPromptDto::PermanentDelete {
                    id: prompt_id,
                    title: format!("Delete “{target_name}” permanently?"),
                    message: format!(
                        "This remote item on {location_name} will be removed immediately. It cannot be recovered from Trash."
                    ),
                    target_name,
                    location_name,
                    confirm_label: "Delete Permanently",
                })?;
                match active.await_prompt_async(response).await? {
                    FileOperationPromptResponseDto::Confirm => {}
                    FileOperationPromptResponseDto::Cancel => return Err(ExplorerError::Cancelled),
                    FileOperationPromptResponseDto::KeepBoth
                    | FileOperationPromptResponseDto::Skip => {
                        return Err(ExplorerError::InvalidConfiguration(
                            "That response is not valid for permanent deletion.".to_owned(),
                        ));
                    }
                }
                events.running()?;
                active.ensure_not_cancelled()?;
                ssh.permanently_delete_entry(source, &active.cancelled, |completed, total| {
                    events.progress(completed, total)
                })
                .await
                .map(deleted_remote_outcome)
            }
        }
    }

    fn with_execution_guard<T>(
        &self,
        execute: impl FnOnce() -> Result<T, ExplorerError>,
    ) -> Result<T, ExplorerError> {
        let _guard = self
            .execution_guard
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        execute()
    }

    fn finish(&self, operation_id: &str) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(operation_id);
        }
    }
}

fn validate_request(request: &FileOperationRequestDto) -> Result<(), ExplorerError> {
    if request.sources.is_empty() || request.sources.len() > MAX_OPERATION_SOURCES {
        return Err(ExplorerError::InvalidConfiguration(
            "Filesystem actions currently require exactly one selected item.".to_owned(),
        ));
    }
    let source = &request.sources[0];
    if source.id.is_empty()
        || source.id.len() > 256
        || source.location_id.is_empty()
        || source.location_id.len() > 256
    {
        return Err(ExplorerError::InvalidReference);
    }
    if let FileOperationActionDto::Move { destination } = &request.action {
        if destination.id.is_empty()
            || destination.id.len() > 256
            || destination.location_id.is_empty()
            || destination.location_id.len() > 256
            || destination.name.len() > 1_024
            || destination.display_path.len() > 4_096
        {
            return Err(ExplorerError::InvalidReference);
        }
    }
    Ok(())
}

fn moved_outcome(entry: MovedLocalEntry) -> FileOperationOutcomeDto {
    FileOperationOutcomeDto::Moved {
        entry: Box::new(entry.entry),
        source_parent: entry.source_parent,
        destination: entry.destination,
        rebased_entry_ids: entry.rebased_entry_ids,
    }
}

fn moved_remote_outcome(entry: MovedRemoteEntry) -> FileOperationOutcomeDto {
    FileOperationOutcomeDto::Moved {
        entry: Box::new(entry.entry),
        source_parent: entry.source_parent,
        destination: entry.destination,
        rebased_entry_ids: entry.rebased_entry_ids,
    }
}

fn trashed_outcome(entry: RemovedLocalEntry) -> FileOperationOutcomeDto {
    FileOperationOutcomeDto::Trashed {
        entry: entry.reference,
        name: entry.name,
        invalidated_entry_ids: entry.invalidated_entry_ids,
    }
}

fn deleted_outcome(entry: RemovedLocalEntry) -> FileOperationOutcomeDto {
    FileOperationOutcomeDto::DeletedPermanently {
        entry: entry.reference,
        name: entry.name,
        invalidated_entry_ids: entry.invalidated_entry_ids,
    }
}

fn deleted_remote_outcome(entry: RemovedRemoteEntry) -> FileOperationOutcomeDto {
    FileOperationOutcomeDto::DeletedPermanently {
        entry: entry.reference,
        name: entry.name,
        invalidated_entry_ids: entry.invalidated_entry_ids,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::Mutex as StdMutex,
        time::Duration,
    };

    use serde_json::{json, Value};
    use tauri::ipc::InvokeResponseBody;
    use tempfile::TempDir;

    use crate::{
        filesystem::{DirectoryListingEvent, LocationRole, LocationSummaryDto},
        local_filesystem::LocalRoot,
        ssh::{SshConnectionEventDto, SshPromptResponseDto},
        ssh_targets::ResolvedSshTarget,
        ssh_test_server::{TestAuthMode, TestSshServer},
    };

    use super::*;

    struct FakeTrash {
        destination: PathBuf,
    }

    impl PlatformTrash for FakeTrash {
        fn is_available(&self) -> bool {
            true
        }

        fn move_to_trash(&self, path: &Path) -> Result<(), ExplorerError> {
            let name = path.file_name().ok_or(ExplorerError::InvalidReference)?;
            fs::rename(path, self.destination.join(name))
                .map_err(|error| ExplorerError::io("trash", path, error))
        }
    }

    fn fixture() -> (TempDir, Arc<LocalFilesystem>, EntryRefDto) {
        let temp = TempDir::new().expect("temporary directory");
        fs::write(temp.path().join("notes.md"), b"hello").expect("fixture file");
        let local = Arc::new(
            LocalFilesystem::new_with_trash_support(
                vec![LocalRoot {
                    id: "home",
                    name: "Home",
                    role: LocationRole::Home,
                    path: temp.path().to_path_buf(),
                }],
                true,
            )
            .expect("local filesystem"),
        );
        let root = local.locations().expect("locations")[0].root.clone();
        let mut source = None;
        local
            .list_directory(
                &root.id,
                &root.location_id,
                &AtomicBool::new(false),
                |event| {
                    if let DirectoryListingEvent::Entries { entries, .. } = event {
                        source = entries
                            .into_iter()
                            .find(|entry| entry.name == "notes.md")
                            .map(|entry| entry.reference);
                    }
                    Ok(())
                },
            )
            .expect("directory listing");
        (temp, local, source.expect("source entry"))
    }

    fn channel(events: Arc<StdMutex<Vec<Value>>>) -> Channel<FileOperationEventDto> {
        Channel::new(move |body| {
            let InvokeResponseBody::Json(json) = body else {
                panic!("operation events must be JSON");
            };
            events
                .lock()
                .expect("captured events")
                .push(serde_json::from_str(&json).expect("valid event JSON"));
            Ok(())
        })
    }

    async fn connect_remote_fixture(
        server: &TestSshServer,
        manager: Arc<SshConnectionManager>,
    ) -> LocationSummaryDto {
        let request_id = "operation-remote-connect".to_owned();
        let response_manager = manager.clone();
        let response_request_id = request_id.clone();
        let channel = Channel::<SshConnectionEventDto>::new(move |body| {
            let InvokeResponseBody::Json(json) = body else {
                panic!("SSH events must be JSON");
            };
            let event: Value = serde_json::from_str(&json).expect("valid SSH event");
            if event["event"] == "hostKeyPrompt" {
                response_manager
                    .respond(
                        &response_request_id,
                        event["promptId"].as_str().expect("prompt id"),
                        SshPromptResponseDto::Accept,
                    )
                    .expect("accept disposable host key");
            }
            Ok(())
        });
        manager
            .connect(
                ResolvedSshTarget {
                    id: "operation-test-target".to_owned(),
                    name: "Operation test server".to_owned(),
                    host: server.host().to_owned(),
                    port: server.port(),
                    username: server.username().to_owned(),
                    initial_path: "/".to_owned(),
                    identity_files: vec![server.identity_file().to_owned()],
                    identities_only: true,
                    known_hosts_path: server.known_hosts_path(),
                },
                request_id,
                channel,
            )
            .await
            .expect("connect operation SFTP fixture")
    }

    async fn remote_root_entries(
        manager: &SshConnectionManager,
        location: &LocationSummaryDto,
    ) -> Vec<FileEntrySummaryDto> {
        let entries = Arc::new(StdMutex::new(Vec::new()));
        let captured_entries = entries.clone();
        manager
            .list_directory(
                &location.id,
                &location.root.id,
                &AtomicBool::new(false),
                move |event| {
                    if let DirectoryListingEvent::Entries { entries, .. } = event {
                        captured_entries
                            .lock()
                            .map_err(|_| ExplorerError::StateUnavailable)?
                            .extend(entries);
                    }
                    Ok(())
                },
            )
            .await
            .expect("list operation SFTP fixture");
        let result = entries.lock().expect("remote entries").clone();
        result
    }

    fn destination_fixture(temp: &TempDir, local: &LocalFilesystem, name: &str) -> DirectoryRefDto {
        fs::create_dir(temp.path().join(name)).expect("destination fixture");
        let root = local.locations().expect("locations")[0].root.clone();
        let mut destination = None;
        local
            .list_directory(
                &root.id,
                &root.location_id,
                &AtomicBool::new(false),
                |event| {
                    if let DirectoryListingEvent::Entries { entries, .. } = event {
                        destination = entries
                            .into_iter()
                            .find(|entry| entry.name == name)
                            .and_then(|entry| entry.directory);
                    }
                    Ok(())
                },
            )
            .expect("directory listing");
        destination.expect("destination directory")
    }

    fn listed_entry_ref(local: &LocalFilesystem, name: &str) -> EntryRefDto {
        let root = local.locations().expect("locations")[0].root.clone();
        let mut source = None;
        local
            .list_directory(
                &root.id,
                &root.location_id,
                &AtomicBool::new(false),
                |event| {
                    if let DirectoryListingEvent::Entries { entries, .. } = event {
                        source = entries
                            .into_iter()
                            .find(|entry| entry.name == name)
                            .map(|entry| entry.reference);
                    }
                    Ok(())
                },
            )
            .expect("directory listing");
        source.expect("listed entry")
    }

    async fn wait_for_event(events: &Arc<StdMutex<Vec<Value>>>, expected: &str) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if events
                    .lock()
                    .expect("events")
                    .iter()
                    .any(|event| event.get("event").and_then(Value::as_str) == Some(expected))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("operation event");
    }

    #[test]
    fn request_rejects_unknown_fields_and_preserves_typed_actions() {
        let rename: FileOperationRequestDto = serde_json::from_value(json!({
            "sources": [{ "id": "entry", "locationId": "home" }],
            "action": { "kind": "rename", "newName": "renamed.txt" }
        }))
        .expect("valid request");
        assert_eq!(
            rename.action,
            FileOperationActionDto::Rename {
                new_name: "renamed.txt".to_owned()
            }
        );
        let delete: FileOperationRequestDto = serde_json::from_value(json!({
            "sources": [{ "id": "entry", "locationId": "home" }],
            "action": { "kind": "deletePermanently" }
        }))
        .expect("valid request");
        assert_eq!(delete.action, FileOperationActionDto::DeletePermanently {});
        let move_request: FileOperationRequestDto = serde_json::from_value(json!({
            "sources": [{ "id": "entry", "locationId": "home" }],
            "action": {
                "kind": "move",
                "destination": {
                    "id": "folder",
                    "locationId": "home",
                    "name": "Folder",
                    "displayPath": "/untrusted/presentation",
                    "capabilities": { "acceptMove": true, "atomicReplace": false }
                }
            }
        }))
        .expect("valid move request");
        assert!(matches!(
            move_request.action,
            FileOperationActionDto::Move { .. }
        ));

        assert!(serde_json::from_value::<FileOperationRequestDto>(json!({
            "sources": [{ "id": "entry", "locationId": "home" }],
            "action": { "kind": "trash", "force": true }
        }))
        .is_err());
    }

    #[test]
    fn terminal_events_keep_identity_action_progress_and_sequence() {
        let event = FileOperationEventDto::Cancelled {
            operation_id: "operation-1".to_owned(),
            sequence: 2,
            action: FileOperationKindDto::Trash,
            completed_items: 0,
            total_items: 1,
        };
        assert_eq!(
            serde_json::to_value(event).expect("serializable event"),
            json!({
                "event": "cancelled",
                "operationId": "operation-1",
                "sequence": 2,
                "action": "trash",
                "completedItems": 0,
                "totalItems": 1
            })
        );
    }

    #[tokio::test]
    async fn coordinator_streams_a_complete_local_rename_lifecycle() {
        let (temp, local, source) = fixture();
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));
        let coordinator = Arc::new(FileOperationCoordinator::default());

        let operation_id = coordinator
            .start(
                local,
                FileOperationRequestDto {
                    sources: vec![source],
                    action: FileOperationActionDto::Rename {
                        new_name: "renamed.md".to_owned(),
                    },
                },
                channel(events.clone()),
            )
            .expect("start operation");

        wait_for_event(&events, "completed").await;
        let events = events.lock().expect("events");
        assert_eq!(
            events
                .iter()
                .map(|event| event["sequence"].as_u64().expect("sequence"))
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert!(events
            .iter()
            .all(|event| event["operationId"] == operation_id));
        assert_eq!(events[2]["outcome"]["kind"], "renamed");
        assert_eq!(
            fs::read(temp.path().join("renamed.md")).expect("renamed file"),
            b"hello"
        );
    }

    #[tokio::test]
    async fn trash_uses_the_platform_adapter_without_a_confirmation() {
        let (temp, local, source) = fixture();
        let trash_destination = temp.path().join("native-trash");
        fs::create_dir(&trash_destination).expect("trash fixture");
        let coordinator = Arc::new(FileOperationCoordinator::with_platform_trash(Arc::new(
            FakeTrash {
                destination: trash_destination.clone(),
            },
        )));
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));

        coordinator
            .start(
                local,
                FileOperationRequestDto {
                    sources: vec![source],
                    action: FileOperationActionDto::Trash {},
                },
                channel(events.clone()),
            )
            .expect("start operation");
        wait_for_event(&events, "completed").await;

        assert!(trash_destination.join("notes.md").is_file());
        assert!(!events
            .lock()
            .expect("events")
            .iter()
            .any(|event| event["event"] == "awaitingConfirmation"));
    }

    #[tokio::test]
    async fn coordinator_moves_locally_with_a_typed_terminal_result() {
        let (temp, local, source) = fixture();
        let destination = destination_fixture(&temp, &local, "destination");
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));

        coordinator
            .start(
                local,
                FileOperationRequestDto {
                    sources: vec![source],
                    action: FileOperationActionDto::Move {
                        destination: destination.clone(),
                    },
                },
                channel(events.clone()),
            )
            .expect("start move");
        wait_for_event(&events, "completed").await;

        let events = events.lock().expect("events");
        assert_eq!(events[2]["action"], "move");
        assert_eq!(events[2]["outcome"]["kind"], "moved");
        assert_eq!(events[2]["outcome"]["destination"]["id"], destination.id);
        assert!(temp.path().join("destination/notes.md").is_file());
        assert!(!temp.path().join("notes.md").exists());
    }

    #[tokio::test]
    async fn move_conflict_allows_only_matching_keep_both_skip_or_cancel_responses() {
        let (temp, local, source) = fixture();
        let destination = destination_fixture(&temp, &local, "destination");
        fs::write(temp.path().join("destination/notes.md"), b"existing").expect("conflict fixture");
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));
        let operation_id = coordinator
            .start(
                local,
                FileOperationRequestDto {
                    sources: vec![source],
                    action: FileOperationActionDto::Move { destination },
                },
                channel(events.clone()),
            )
            .expect("start move");
        wait_for_event(&events, "awaitingConflict").await;
        let prompt_id = events.lock().expect("events")[2]["prompt"]["id"]
            .as_str()
            .expect("prompt id")
            .to_owned();
        assert_eq!(
            events.lock().expect("events")[2]["prompt"]["decisions"],
            json!(["keepBoth", "skip", "cancel"])
        );
        assert!(matches!(
            coordinator.respond(
                &operation_id,
                &prompt_id,
                FileOperationPromptResponseDto::Confirm
            ),
            Err(ExplorerError::InvalidConfiguration(_))
        ));
        coordinator
            .respond(
                &operation_id,
                &prompt_id,
                FileOperationPromptResponseDto::KeepBoth,
            )
            .expect("keep both response");
        wait_for_event(&events, "completed").await;

        let events = events.lock().expect("events");
        assert_eq!(
            events
                .iter()
                .map(|event| event["sequence"].as_u64().expect("sequence"))
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );
        assert_eq!(events[4]["outcome"]["entry"]["name"], "notes copy.md");
        assert_eq!(
            fs::read(temp.path().join("destination/notes.md")).expect("existing target"),
            b"existing"
        );
        assert_eq!(
            fs::read(temp.path().join("destination/notes copy.md")).expect("moved source"),
            b"hello"
        );
    }

    #[tokio::test]
    async fn skipping_a_move_conflict_preserves_the_source() {
        let (temp, local, source) = fixture();
        let destination = destination_fixture(&temp, &local, "destination");
        fs::write(temp.path().join("destination/notes.md"), b"existing").expect("conflict fixture");
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));
        let operation_id = coordinator
            .start(
                local,
                FileOperationRequestDto {
                    sources: vec![source],
                    action: FileOperationActionDto::Move { destination },
                },
                channel(events.clone()),
            )
            .expect("start move");
        wait_for_event(&events, "awaitingConflict").await;
        let prompt_id = events.lock().expect("events")[2]["prompt"]["id"]
            .as_str()
            .expect("prompt id")
            .to_owned();
        coordinator
            .respond(
                &operation_id,
                &prompt_id,
                FileOperationPromptResponseDto::Skip,
            )
            .expect("skip response");
        wait_for_event(&events, "completed").await;

        assert!(temp.path().join("notes.md").is_file());
        assert_eq!(
            events
                .lock()
                .expect("events")
                .last()
                .expect("terminal event")["outcome"]["kind"],
            "moveSkipped"
        );
    }

    #[tokio::test]
    async fn an_unanswered_prompt_does_not_block_an_unrelated_quick_operation() {
        let (temp, local, source) = fixture();
        let destination = destination_fixture(&temp, &local, "destination");
        fs::write(temp.path().join("destination/notes.md"), b"existing").expect("conflict fixture");
        fs::write(temp.path().join("other.txt"), b"other").expect("unrelated fixture");
        let unrelated = listed_entry_ref(&local, "other.txt");
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let move_events = Arc::new(StdMutex::new(Vec::<Value>::new()));
        let move_operation_id = coordinator
            .start(
                local.clone(),
                FileOperationRequestDto {
                    sources: vec![source],
                    action: FileOperationActionDto::Move { destination },
                },
                channel(move_events.clone()),
            )
            .expect("start move");
        wait_for_event(&move_events, "awaitingConflict").await;

        let rename_events = Arc::new(StdMutex::new(Vec::<Value>::new()));
        coordinator
            .start(
                local,
                FileOperationRequestDto {
                    sources: vec![unrelated],
                    action: FileOperationActionDto::Rename {
                        new_name: "renamed-other.txt".to_owned(),
                    },
                },
                channel(rename_events.clone()),
            )
            .expect("start unrelated rename");
        wait_for_event(&rename_events, "completed").await;
        assert!(temp.path().join("renamed-other.txt").is_file());

        let prompt_id = move_events.lock().expect("events")[2]["prompt"]["id"]
            .as_str()
            .expect("prompt id")
            .to_owned();
        coordinator
            .respond(
                &move_operation_id,
                &prompt_id,
                FileOperationPromptResponseDto::Cancel,
            )
            .expect("cancel move prompt");
        wait_for_event(&move_events, "cancelled").await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn coordinator_confirms_and_deletes_a_remote_entry_with_authoritative_host_context() {
        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let ssh = Arc::new(SshConnectionManager::default());
        let location = connect_remote_fixture(&server, ssh.clone()).await;
        let remote_entry = remote_root_entries(&ssh, &location)
            .await
            .into_iter()
            .find(|entry| entry.name == "README.md")
            .expect("remote file");
        let (_temp, local, _) = fixture();
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));
        let operation_id = coordinator
            .start_with_backends(
                local,
                ssh.clone(),
                FileOperationRequestDto {
                    sources: vec![remote_entry.reference],
                    action: FileOperationActionDto::DeletePermanently {},
                },
                channel(events.clone()),
            )
            .expect("start remote delete");
        wait_for_event(&events, "awaitingConfirmation").await;
        let prompt = events.lock().expect("events")[2]["prompt"].clone();
        assert!(prompt["message"]
            .as_str()
            .expect("message")
            .contains("remote item"));
        assert!(prompt["locationName"]
            .as_str()
            .expect("location")
            .contains(server.host()));
        coordinator
            .respond(
                &operation_id,
                prompt["id"].as_str().expect("prompt id"),
                FileOperationPromptResponseDto::Confirm,
            )
            .expect("confirm remote delete");
        wait_for_event(&events, "completed").await;

        {
            let events = events.lock().expect("events");
            let terminal = events.last().expect("terminal event");
            assert_eq!(terminal["outcome"]["kind"], "deletedPermanently");
            assert_eq!(terminal["completedItems"], 1);
        }
        assert!(!server.path_exists("/README.md").await);
        ssh.disconnect("operation-test-target")
            .await
            .expect("disconnect");
        server.shutdown().await;
    }

    #[tokio::test]
    async fn permanent_delete_requires_the_matching_single_use_confirmation() {
        let (temp, local, source) = fixture();
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));
        let operation_id = coordinator
            .start(
                local,
                FileOperationRequestDto {
                    sources: vec![source],
                    action: FileOperationActionDto::DeletePermanently {},
                },
                channel(events.clone()),
            )
            .expect("start operation");
        wait_for_event(&events, "awaitingConfirmation").await;
        assert!(temp.path().join("notes.md").is_file());
        let prompt_id = events.lock().expect("events")[2]["prompt"]["id"]
            .as_str()
            .expect("prompt id")
            .to_owned();

        assert!(matches!(
            coordinator.respond(
                &operation_id,
                "wrong-prompt",
                FileOperationPromptResponseDto::Confirm
            ),
            Err(ExplorerError::InvalidReference)
        ));
        coordinator
            .respond(
                &operation_id,
                &prompt_id,
                FileOperationPromptResponseDto::Confirm,
            )
            .expect("confirm delete");
        wait_for_event(&events, "completed").await;

        assert!(!temp.path().join("notes.md").exists());
        assert!(matches!(
            coordinator.respond(
                &operation_id,
                &prompt_id,
                FileOperationPromptResponseDto::Confirm
            ),
            Err(ExplorerError::InvalidReference)
        ));
    }

    #[tokio::test]
    async fn cancelling_a_permanent_delete_prompt_preserves_the_source() {
        let (temp, local, source) = fixture();
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));
        let operation_id = coordinator
            .start(
                local,
                FileOperationRequestDto {
                    sources: vec![source],
                    action: FileOperationActionDto::DeletePermanently {},
                },
                channel(events.clone()),
            )
            .expect("start operation");
        wait_for_event(&events, "awaitingConfirmation").await;
        let prompt_id = events.lock().expect("events")[2]["prompt"]["id"]
            .as_str()
            .expect("prompt id")
            .to_owned();
        coordinator
            .respond(
                &operation_id,
                &prompt_id,
                FileOperationPromptResponseDto::Cancel,
            )
            .expect("cancel delete");
        wait_for_event(&events, "cancelled").await;

        assert!(temp.path().join("notes.md").is_file());
    }
}
