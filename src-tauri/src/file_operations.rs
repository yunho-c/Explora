use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

use crate::{
    filesystem::{
        DirectoryRefDto, EntryRefDto, ExplorerError, ExplorerErrorCode, ExplorerErrorDto,
        FileEntrySummaryDto, PROMPT_TIMEOUT,
    },
    local_filesystem::{
        LocalFilesystem, LocalMoveConflictPolicy, MovedLocalEntry, PreparedLocalFileDestination,
        PreparedLocalFileTransfer, RemovedLocalEntry, TransferredLocalEntry,
    },
    platform_trash::{PlatformTrash, SystemPlatformTrash},
    ssh::{
        MovedRemoteEntry, PreparedRemoteDestination, PreparedRemoteTransfer,
        RemoteMoveConflictPolicy, RemoteTransferDestinationKind, RemoteTransferEntryKind,
        RemovedRemoteEntry, SshConnectionManager,
    },
    transfer::{LocalTransferEntryKind, TRANSFER_CHUNK_BYTES},
};

const MAX_OPERATION_SOURCES: usize = 1_000;
const BYTE_PROGRESS_EVENT_INTERVAL: u64 = 4 * 1024 * 1024;
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
        invalidated_entry_ids: Vec<String>,
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
    Batch {
        status: FileOperationBatchStatusDto,
        items: Vec<FileOperationBatchItemDto>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FileOperationBatchStatusDto {
    Completed,
    Partial,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FileOperationBatchItemDto {
    Completed {
        source: EntryRefDto,
        outcome: Box<FileOperationOutcomeDto>,
    },
    Failed {
        source: EntryRefDto,
        error: ExplorerErrorDto,
    },
    Cancelled {
        source: EntryRefDto,
    },
    NotStarted {
        source: EntryRefDto,
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
        completed_bytes: Option<String>,
        total_bytes: Option<String>,
        current_item_completed: Option<u64>,
        current_item_total: Option<u64>,
    },
    Running {
        operation_id: String,
        sequence: u64,
        action: FileOperationKindDto,
        completed_items: u64,
        total_items: u64,
        completed_bytes: Option<String>,
        total_bytes: Option<String>,
        current_item_completed: Option<u64>,
        current_item_total: Option<u64>,
    },
    AwaitingConfirmation {
        operation_id: String,
        sequence: u64,
        action: FileOperationKindDto,
        completed_items: u64,
        total_items: u64,
        completed_bytes: Option<String>,
        total_bytes: Option<String>,
        current_item_completed: Option<u64>,
        current_item_total: Option<u64>,
        prompt: FileOperationPromptDto,
    },
    AwaitingConflict {
        operation_id: String,
        sequence: u64,
        action: FileOperationKindDto,
        completed_items: u64,
        total_items: u64,
        completed_bytes: Option<String>,
        total_bytes: Option<String>,
        current_item_completed: Option<u64>,
        current_item_total: Option<u64>,
        prompt: FileOperationPromptDto,
    },
    Completed {
        operation_id: String,
        sequence: u64,
        action: FileOperationKindDto,
        completed_items: u64,
        total_items: u64,
        completed_bytes: Option<String>,
        total_bytes: Option<String>,
        current_item_completed: Option<u64>,
        current_item_total: Option<u64>,
        outcome: FileOperationOutcomeDto,
    },
    Cancelled {
        operation_id: String,
        sequence: u64,
        action: FileOperationKindDto,
        completed_items: u64,
        total_items: u64,
        completed_bytes: Option<String>,
        total_bytes: Option<String>,
        current_item_completed: Option<u64>,
        current_item_total: Option<u64>,
        outcome: Option<FileOperationOutcomeDto>,
    },
    Failed {
        operation_id: String,
        sequence: u64,
        action: FileOperationKindDto,
        completed_items: u64,
        total_items: u64,
        completed_bytes: Option<String>,
        total_bytes: Option<String>,
        current_item_completed: Option<u64>,
        current_item_total: Option<u64>,
        error: ExplorerErrorDto,
        outcome: Option<FileOperationOutcomeDto>,
    },
}

enum OperationTerminal {
    Completed(FileOperationOutcomeDto),
    Cancelled(Option<FileOperationOutcomeDto>),
    Failed {
        error: ExplorerErrorDto,
        outcome: Option<FileOperationOutcomeDto>,
    },
}

impl From<Result<FileOperationOutcomeDto, ExplorerError>> for OperationTerminal {
    fn from(result: Result<FileOperationOutcomeDto, ExplorerError>) -> Self {
        match result {
            Ok(outcome) => Self::Completed(outcome),
            Err(ExplorerError::Cancelled) => Self::Cancelled(None),
            Err(error) => Self::Failed {
                error: ExplorerErrorDto::from(error),
                outcome: None,
            },
        }
    }
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
    completed_bytes: Option<u64>,
    total_bytes: Option<u64>,
    current_item_completed: Option<u64>,
    current_item_total: Option<u64>,
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
                completed_bytes: self.completed_bytes.map(|value| value.to_string()),
                total_bytes: self.total_bytes.map(|value| value.to_string()),
                current_item_completed: self.current_item_completed,
                current_item_total: self.current_item_total,
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
                completed_bytes: self.completed_bytes.map(|value| value.to_string()),
                total_bytes: self.total_bytes.map(|value| value.to_string()),
                current_item_completed: self.current_item_completed,
                current_item_total: self.current_item_total,
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
                completed_bytes: self.completed_bytes.map(|value| value.to_string()),
                total_bytes: self.total_bytes.map(|value| value.to_string()),
                current_item_completed: self.current_item_completed,
                current_item_total: self.current_item_total,
                prompt,
            })
            .map_err(|_| ExplorerError::ChannelClosed)
    }

    fn progress(&mut self, completed_items: u64, total_items: u64) -> Result<(), ExplorerError> {
        if total_items == 0 || completed_items > total_items {
            return Err(ExplorerError::StateUnavailable);
        }
        self.current_item_completed = Some(completed_items);
        self.current_item_total = Some(total_items);
        self.running()
    }

    fn begin_item(&mut self) {
        self.completed_bytes = None;
        self.total_bytes = None;
        self.current_item_completed = None;
        self.current_item_total = None;
    }

    fn settle_item(&mut self) -> Result<(), ExplorerError> {
        if self.completed_items >= self.total_items {
            return Err(ExplorerError::StateUnavailable);
        }
        self.completed_items += 1;
        self.begin_item();
        if self.completed_items < self.total_items {
            self.running()?;
        }
        Ok(())
    }

    fn byte_progress(
        &mut self,
        completed_bytes: u64,
        total_bytes: u64,
    ) -> Result<(), ExplorerError> {
        if completed_bytes > total_bytes {
            return Err(ExplorerError::StateUnavailable);
        }
        self.completed_bytes = Some(completed_bytes);
        self.total_bytes = Some(total_bytes);
        self.running()
    }

    fn terminal(&mut self, terminal: OperationTerminal) {
        let sequence = self.next_sequence();
        let event = match terminal {
            OperationTerminal::Completed(outcome) => FileOperationEventDto::Completed {
                operation_id: self.operation_id.clone(),
                sequence,
                action: self.action,
                completed_items: self.total_items,
                total_items: self.total_items,
                completed_bytes: self.total_bytes.map(|value| value.to_string()),
                total_bytes: self.total_bytes.map(|value| value.to_string()),
                current_item_completed: self.current_item_total,
                current_item_total: self.current_item_total,
                outcome,
            },
            OperationTerminal::Cancelled(outcome) => FileOperationEventDto::Cancelled {
                operation_id: self.operation_id.clone(),
                sequence,
                action: self.action,
                completed_items: self.completed_items,
                total_items: self.total_items,
                completed_bytes: self.completed_bytes.map(|value| value.to_string()),
                total_bytes: self.total_bytes.map(|value| value.to_string()),
                current_item_completed: self.current_item_completed,
                current_item_total: self.current_item_total,
                outcome,
            },
            OperationTerminal::Failed { error, outcome } => FileOperationEventDto::Failed {
                operation_id: self.operation_id.clone(),
                sequence,
                action: self.action,
                completed_items: self.completed_items,
                total_items: self.total_items,
                completed_bytes: self.completed_bytes.map(|value| value.to_string()),
                total_bytes: self.total_bytes.map(|value| value.to_string()),
                current_item_completed: self.current_item_completed,
                current_item_total: self.current_item_total,
                error,
                outcome,
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
    transfer_guard: tokio::sync::Mutex<()>,
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
            transfer_guard: tokio::sync::Mutex::new(()),
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
        let total_items = u64::try_from(request.sources.len()).map_err(|_| {
            ExplorerError::InvalidConfiguration("Too many selected items.".to_owned())
        })?;
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
                total_items,
                completed_bytes: None,
                total_bytes: None,
                current_item_completed: None,
                current_item_total: None,
            })
            .is_err()
        {
            self.finish(&operation_id);
            return Err(ExplorerError::ChannelClosed);
        }

        let coordinator = self.clone();
        let task_operation_id = operation_id.clone();
        let uses_remote_backend = request.sources[0].location_id.starts_with("ssh:")
            || matches!(
                &request.action,
                FileOperationActionDto::Move { destination }
                    if destination.location_id.starts_with("ssh:")
            );
        if uses_remote_backend {
            tauri::async_runtime::spawn(async move {
                let mut events = OperationEventEmitter {
                    operation_id: task_operation_id.clone(),
                    action,
                    sequence: 1,
                    completed_items: 0,
                    total_items,
                    completed_bytes: None,
                    total_bytes: None,
                    current_item_completed: None,
                    current_item_total: None,
                    channel: on_event,
                };
                let result = if request.sources[0].location_id.starts_with("ssh:") {
                    coordinator
                        .run_remote_batch(
                            local.clone(),
                            &ssh,
                            &request,
                            active.clone(),
                            &mut events,
                        )
                        .await
                } else {
                    coordinator
                        .run_local_to_remote_batch(
                            local,
                            ssh,
                            &request,
                            active.clone(),
                            &mut events,
                        )
                        .await
                };
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
                    total_items,
                    completed_bytes: None,
                    total_bytes: None,
                    current_item_completed: None,
                    current_item_total: None,
                    channel: on_event,
                };
                let result = coordinator.run_local_batch(&local, &request, &active, &mut events);
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

    fn run_local_batch(
        &self,
        local: &LocalFilesystem,
        request: &FileOperationRequestDto,
        active: &ActiveOperation,
        events: &mut OperationEventEmitter,
    ) -> OperationTerminal {
        if request.sources.len() == 1 {
            return self
                .run_local_single(local, request, active, events, false)
                .into();
        }
        if let Err(error) = local.validate_batch_sources(&request.sources) {
            return OperationTerminal::from(Err(error));
        }

        let delete_confirmed =
            if matches!(request.action, FileOperationActionDto::DeletePermanently {}) {
                match self.confirm_local_batch_delete(local, request, active, events) {
                    Ok(()) => true,
                    Err(ExplorerError::Cancelled) => {
                        return OperationTerminal::Cancelled(Some(cancelled_batch_outcome(
                            &request.sources,
                            Vec::new(),
                            0,
                        )));
                    }
                    Err(error) => return OperationTerminal::from(Err(error)),
                }
            } else {
                false
            };

        let mut items = Vec::with_capacity(request.sources.len());
        let mut successful = 0_usize;
        let mut failures = 0_usize;
        for (index, source) in request.sources.iter().enumerate() {
            events.begin_item();
            if active.ensure_not_cancelled().is_err() {
                return OperationTerminal::Cancelled(Some(cancelled_batch_outcome(
                    &request.sources,
                    items,
                    index,
                )));
            }
            let single = single_source_request(request, source.clone());
            match self.run_local_single(local, &single, active, events, delete_confirmed) {
                Ok(outcome) => {
                    successful += 1;
                    items.push(FileOperationBatchItemDto::Completed {
                        source: source.clone(),
                        outcome: Box::new(outcome),
                    });
                    if let Err(error) = events.settle_item() {
                        return failed_batch_terminal(
                            &request.sources,
                            items,
                            index + 1,
                            successful,
                            ExplorerErrorDto::from(error),
                        );
                    }
                }
                Err(ExplorerError::Cancelled) => {
                    items.push(FileOperationBatchItemDto::Cancelled {
                        source: source.clone(),
                    });
                    return OperationTerminal::Cancelled(Some(cancelled_batch_outcome(
                        &request.sources,
                        items,
                        index + 1,
                    )));
                }
                Err(error) => {
                    let stop = should_stop_batch(&error);
                    let error = ExplorerErrorDto::from(error);
                    failures += 1;
                    items.push(FileOperationBatchItemDto::Failed {
                        source: source.clone(),
                        error: error.clone(),
                    });
                    if let Err(progress_error) = events.settle_item() {
                        return failed_batch_terminal(
                            &request.sources,
                            items,
                            index + 1,
                            successful,
                            ExplorerErrorDto::from(progress_error),
                        );
                    }
                    if stop {
                        return failed_batch_terminal(
                            &request.sources,
                            items,
                            index + 1,
                            successful,
                            error,
                        );
                    }
                }
            }
        }

        completed_batch_terminal(items, successful, failures, request.sources.len())
    }

    fn confirm_local_batch_delete(
        &self,
        local: &LocalFilesystem,
        request: &FileOperationRequestDto,
        active: &ActiveOperation,
        events: &mut OperationEventEmitter,
    ) -> Result<(), ExplorerError> {
        let (_, location_name) = local.describe_operation_target(&request.sources[0])?;
        for source in request.sources.iter().skip(1) {
            local.describe_operation_target(source)?;
        }
        let count = request.sources.len();
        let target_name = format!("{count} selected items");
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
            title: format!("Delete {count} items permanently?"),
            message: "These items will be removed immediately and cannot be recovered from Trash."
                .to_owned(),
            target_name,
            location_name,
            confirm_label: "Delete Permanently",
        })?;
        match active.await_prompt(response)? {
            FileOperationPromptResponseDto::Confirm => Ok(()),
            FileOperationPromptResponseDto::Cancel => Err(ExplorerError::Cancelled),
            FileOperationPromptResponseDto::KeepBoth | FileOperationPromptResponseDto::Skip => {
                Err(ExplorerError::InvalidConfiguration(
                    "That response is not valid for permanent deletion.".to_owned(),
                ))
            }
        }
    }

    fn run_local_single(
        &self,
        local: &LocalFilesystem,
        request: &FileOperationRequestDto,
        active: &ActiveOperation,
        events: &mut OperationEventEmitter,
        delete_confirmed: bool,
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
                if destination.location_id != source.location_id
                    && !destination.location_id.starts_with("ssh:")
                {
                    return self.run_local_file_transfer(
                        local,
                        source,
                        destination,
                        active,
                        events,
                    );
                }
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
                if !delete_confirmed {
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
                        FileOperationPromptResponseDto::Cancel => {
                            return Err(ExplorerError::Cancelled);
                        }
                        FileOperationPromptResponseDto::KeepBoth
                        | FileOperationPromptResponseDto::Skip => {
                            return Err(ExplorerError::InvalidConfiguration(
                                "That response is not valid for permanent deletion.".to_owned(),
                            ));
                        }
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

    fn run_local_file_transfer(
        &self,
        local: &LocalFilesystem,
        source: &EntryRefDto,
        destination: &DirectoryRefDto,
        active: &ActiveOperation,
        events: &mut OperationEventEmitter,
    ) -> Result<FileOperationOutcomeDto, ExplorerError> {
        let transfer = |policy, events: &mut OperationEventEmitter| {
            let mut last_emitted = 0_u64;
            self.with_execution_guard(|| {
                local.transfer_entry_to_local_location(
                    source,
                    destination,
                    policy,
                    &active.cancelled,
                    |completed, total| {
                        if completed == 0
                            || completed == total
                            || completed.saturating_sub(last_emitted)
                                >= BYTE_PROGRESS_EVENT_INTERVAL
                        {
                            last_emitted = completed;
                            events.byte_progress(completed, total)
                        } else {
                            Ok(())
                        }
                    },
                )
            })
        };
        match transfer(LocalMoveConflictPolicy::Fail, events) {
            Ok(moved) => Ok(transferred_local_outcome(moved)),
            Err(ExplorerError::Conflict) => {
                let (target_name, destination_name) =
                    local.describe_transfer_conflict(source, destination)?;
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
                        transfer(LocalMoveConflictPolicy::KeepBoth, events)
                            .map(transferred_local_outcome)
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

    async fn run_local_to_remote_batch(
        &self,
        local: Arc<LocalFilesystem>,
        ssh: Arc<SshConnectionManager>,
        request: &FileOperationRequestDto,
        active: Arc<ActiveOperation>,
        events: &mut OperationEventEmitter,
    ) -> OperationTerminal {
        if request.sources.len() == 1 {
            return self
                .run_local_to_remote_single(local, ssh, request, active, events)
                .await
                .into();
        }
        if let Err(error) = local.validate_batch_sources(&request.sources) {
            return OperationTerminal::from(Err(error));
        }

        let mut items = Vec::with_capacity(request.sources.len());
        let mut successful = 0_usize;
        let mut failures = 0_usize;
        for (index, source) in request.sources.iter().enumerate() {
            events.begin_item();
            if active.ensure_not_cancelled().is_err() {
                return OperationTerminal::Cancelled(Some(cancelled_batch_outcome(
                    &request.sources,
                    items,
                    index,
                )));
            }
            let single = single_source_request(request, source.clone());
            match self
                .run_local_to_remote_single(
                    local.clone(),
                    ssh.clone(),
                    &single,
                    active.clone(),
                    events,
                )
                .await
            {
                Ok(outcome) => {
                    successful += 1;
                    items.push(FileOperationBatchItemDto::Completed {
                        source: source.clone(),
                        outcome: Box::new(outcome),
                    });
                    if let Err(error) = events.settle_item() {
                        return failed_batch_terminal(
                            &request.sources,
                            items,
                            index + 1,
                            successful,
                            ExplorerErrorDto::from(error),
                        );
                    }
                }
                Err(ExplorerError::Cancelled) => {
                    items.push(FileOperationBatchItemDto::Cancelled {
                        source: source.clone(),
                    });
                    return OperationTerminal::Cancelled(Some(cancelled_batch_outcome(
                        &request.sources,
                        items,
                        index + 1,
                    )));
                }
                Err(error) => {
                    let stop = should_stop_batch(&error);
                    let error = ExplorerErrorDto::from(error);
                    failures += 1;
                    items.push(FileOperationBatchItemDto::Failed {
                        source: source.clone(),
                        error: error.clone(),
                    });
                    if let Err(progress_error) = events.settle_item() {
                        return failed_batch_terminal(
                            &request.sources,
                            items,
                            index + 1,
                            successful,
                            ExplorerErrorDto::from(progress_error),
                        );
                    }
                    if stop {
                        return failed_batch_terminal(
                            &request.sources,
                            items,
                            index + 1,
                            successful,
                            error,
                        );
                    }
                }
            }
        }

        completed_batch_terminal(items, successful, failures, request.sources.len())
    }

    async fn run_local_to_remote_single(
        &self,
        local: Arc<LocalFilesystem>,
        ssh: Arc<SshConnectionManager>,
        request: &FileOperationRequestDto,
        active: Arc<ActiveOperation>,
        events: &mut OperationEventEmitter,
    ) -> Result<FileOperationOutcomeDto, ExplorerError> {
        let _transfer_guard = self.transfer_guard.lock().await;
        active.ensure_not_cancelled()?;
        events.running()?;
        let source = request.sources[0].clone();
        let FileOperationActionDto::Move { destination } = &request.action else {
            return Err(ExplorerError::InvalidConfiguration(
                "Only moves can cross filesystem backends.".to_owned(),
            ));
        };
        if !destination.location_id.starts_with("ssh:") {
            return Err(ExplorerError::InvalidConfiguration(
                "The remote transfer destination is not valid.".to_owned(),
            ));
        }

        let prepare_local = local.clone();
        let prepare_active = active.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            prepare_local.prepare_file_transfer_source(&source, &prepare_active.cancelled)
        })
        .await
        .map_err(|_| ExplorerError::StateUnavailable)??;

        match self
            .transfer_prepared_local_file_to_remote(
                local.clone(),
                ssh.clone(),
                &prepared,
                destination,
                RemoteMoveConflictPolicy::Fail,
                active.clone(),
                events,
            )
            .await
        {
            Ok(outcome) => Ok(outcome),
            Err(ExplorerError::Conflict) => {
                let destination_name = ssh.describe_transfer_destination(destination).await?;
                let prompt_id = Uuid::new_v4().to_string();
                let decisions = vec![
                    FileOperationPromptResponseDto::KeepBoth,
                    FileOperationPromptResponseDto::Skip,
                    FileOperationPromptResponseDto::Cancel,
                ];
                let response = active.begin_prompt(prompt_id.clone(), decisions.clone())?;
                events.awaiting_conflict(FileOperationPromptDto::MoveConflict {
                    id: prompt_id,
                    title: format!("“{}” already exists", prepared.name),
                    message: format!(
                        "Choose how to handle the existing item in “{destination_name}”. Nothing will be replaced."
                    ),
                    target_name: prepared.name.clone(),
                    destination_name,
                    decisions,
                })?;
                match active.await_prompt_async(response).await? {
                    FileOperationPromptResponseDto::KeepBoth => {
                        events.running()?;
                        self.transfer_prepared_local_file_to_remote(
                            local,
                            ssh,
                            &prepared,
                            destination,
                            RemoteMoveConflictPolicy::KeepBoth,
                            active,
                            events,
                        )
                        .await
                    }
                    FileOperationPromptResponseDto::Skip => {
                        events.running()?;
                        Ok(FileOperationOutcomeDto::MoveSkipped {
                            entry: prepared.source,
                            name: prepared.name,
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

    #[allow(clippy::too_many_arguments)]
    async fn transfer_prepared_local_file_to_remote(
        &self,
        local: Arc<LocalFilesystem>,
        ssh: Arc<SshConnectionManager>,
        prepared: &PreparedLocalFileTransfer,
        destination: &DirectoryRefDto,
        conflict_policy: RemoteMoveConflictPolicy,
        active: Arc<ActiveOperation>,
        events: &mut OperationEventEmitter,
    ) -> Result<FileOperationOutcomeDto, ExplorerError> {
        active.ensure_not_cancelled()?;
        let destination_kind = if prepared.plan.root_is_file() {
            RemoteTransferDestinationKind::File
        } else if prepared.plan.root_is_symlink() {
            let target = prepared
                .plan
                .root_link_target()
                .and_then(|target| target.to_str())
                .ok_or_else(|| {
                    ExplorerError::Unsupported(
                        "This symbolic-link target cannot be represented on the remote filesystem."
                            .to_owned(),
                    )
                })?;
            RemoteTransferDestinationKind::Symlink {
                target: target.to_owned(),
            }
        } else if prepared.plan.root_is_directory() {
            RemoteTransferDestinationKind::Directory
        } else {
            return Err(ExplorerError::Unsupported(
                "This local item type cannot be transferred remotely.".to_owned(),
            ));
        };
        let mut remote = ssh
            .prepare_transfer_destination(
                destination,
                &prepared.name,
                &destination_kind,
                conflict_policy,
                &active.cancelled,
            )
            .await?;
        let total_bytes = prepared.plan.total_bytes();
        events.byte_progress(0, total_bytes)?;

        let copy_result = if prepared.plan.root_is_file() {
            copy_and_verify_local_file_to_remote(prepared, &mut remote, &active, events).await
        } else if prepared.plan.root_is_symlink() {
            verify_local_symlink_to_remote(prepared, &remote, &active).await
        } else {
            copy_and_verify_local_tree_to_remote(prepared, &mut remote, &active, events).await
        };
        if let Err(error) = copy_result {
            return Err(abandon_remote_after_error(remote, error).await);
        }
        if prepared.plan.root_is_file() {
            let permissions = prepared
                .plan
                .entries()
                .first()
                .and_then(|entry| entry.remote_permissions());
            if let Err(error) = remote.set_entry_permissions("", permissions).await {
                return Err(abandon_remote_after_error(remote, error).await);
            }
        }

        let revalidate_local = local.clone();
        let revalidate_prepared = prepared.clone();
        let revalidate_active = active.clone();
        if let Err(error) = tokio::task::spawn_blocking(move || {
            revalidate_local.revalidate_prepared_file_transfer_source(
                &revalidate_prepared,
                &revalidate_active.cancelled,
            )
        })
        .await
        .map_err(|_| ExplorerError::StateUnavailable)?
        {
            return Err(abandon_remote_after_error(remote, error).await);
        }

        if let Err(error) = active.ensure_not_cancelled() {
            return Err(abandon_remote_after_error(remote, error).await);
        }
        let authoritative_destination = remote.destination.clone();
        let entry = remote.finalize().await?;

        // Remote finalization is irreversible and SFTP cannot prove ownership
        // of a later replacement path. From here cancellation must not produce
        // a misleading cancelled state; finish the source decision exactly once.
        let finish_local = local;
        let finish_prepared = prepared.clone();
        let invalidated_entry_ids = tokio::task::spawn_blocking(move || {
            finish_local.finish_prepared_file_transfer_source(
                &finish_prepared,
                &AtomicBool::new(false),
            )
        })
        .await
        .map_err(|_| ExplorerError::PartialCompletion(
            "The verified remote copy was kept, but Explora could not establish whether local source cleanup finished."
                .to_owned(),
        ))??;

        Ok(FileOperationOutcomeDto::Moved {
            entry: Box::new(entry),
            source_parent: prepared.source_parent.clone(),
            destination: authoritative_destination,
            rebased_entry_ids: Vec::new(),
            invalidated_entry_ids,
        })
    }

    async fn run_remote_to_remote(
        &self,
        ssh: &SshConnectionManager,
        source: &EntryRefDto,
        destination: &DirectoryRefDto,
        active: &ActiveOperation,
        events: &mut OperationEventEmitter,
    ) -> Result<FileOperationOutcomeDto, ExplorerError> {
        match self
            .transfer_remote_file_to_remote(
                ssh,
                source,
                destination,
                RemoteMoveConflictPolicy::Fail,
                active,
                events,
            )
            .await
        {
            Ok(outcome) => Ok(outcome),
            Err(ExplorerError::Conflict) => {
                let destination_name = ssh.describe_transfer_destination(destination).await?;
                let source_name = ssh.describe_operation_target(source).await?.0;
                let prompt_id = Uuid::new_v4().to_string();
                let decisions = vec![
                    FileOperationPromptResponseDto::KeepBoth,
                    FileOperationPromptResponseDto::Skip,
                    FileOperationPromptResponseDto::Cancel,
                ];
                let response = active.begin_prompt(prompt_id.clone(), decisions.clone())?;
                events.awaiting_conflict(FileOperationPromptDto::MoveConflict {
                    id: prompt_id,
                    title: format!("“{source_name}” already exists"),
                    message: format!(
                        "Choose how to handle the existing remote item in “{destination_name}”. Nothing will be replaced."
                    ),
                    target_name: source_name.clone(),
                    destination_name,
                    decisions,
                })?;
                match active.await_prompt_async(response).await? {
                    FileOperationPromptResponseDto::KeepBoth => {
                        events.running()?;
                        self.transfer_remote_file_to_remote(
                            ssh,
                            source,
                            destination,
                            RemoteMoveConflictPolicy::KeepBoth,
                            active,
                            events,
                        )
                        .await
                    }
                    FileOperationPromptResponseDto::Skip => {
                        events.running()?;
                        Ok(FileOperationOutcomeDto::MoveSkipped {
                            entry: source.clone(),
                            name: source_name,
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

    #[allow(clippy::too_many_arguments)]
    async fn transfer_remote_file_to_remote(
        &self,
        ssh: &SshConnectionManager,
        source: &EntryRefDto,
        destination: &DirectoryRefDto,
        conflict_policy: RemoteMoveConflictPolicy,
        active: &ActiveOperation,
        events: &mut OperationEventEmitter,
    ) -> Result<FileOperationOutcomeDto, ExplorerError> {
        let _transfer_guard = self.transfer_guard.lock().await;
        active.ensure_not_cancelled()?;
        let prepared_source = ssh
            .prepare_transfer_source(source, &active.cancelled)
            .await?;
        let destination_kind = if prepared_source.root_is_file() {
            RemoteTransferDestinationKind::File
        } else if prepared_source.root_is_symlink() {
            RemoteTransferDestinationKind::Symlink {
                target: prepared_source
                    .root_link_target()
                    .ok_or(ExplorerError::StateUnavailable)?
                    .to_owned(),
            }
        } else if prepared_source.root_is_directory() {
            RemoteTransferDestinationKind::Directory
        } else {
            return Err(ExplorerError::Unsupported(
                "This remote item type cannot be transferred.".to_owned(),
            ));
        };
        let mut prepared_destination = ssh
            .prepare_transfer_destination(
                destination,
                &prepared_source.name,
                &destination_kind,
                conflict_policy,
                &active.cancelled,
            )
            .await?;
        events.byte_progress(0, prepared_source.total_bytes)?;
        let copy_result = if prepared_source.root_is_file() {
            copy_and_verify_remote_file_to_remote(
                &prepared_source,
                &mut prepared_destination,
                active,
                events,
            )
            .await
        } else if prepared_source.root_is_symlink() {
            verify_remote_symlink_to_remote(&prepared_source, &prepared_destination, active).await
        } else {
            copy_and_verify_remote_tree_to_remote(
                &prepared_source,
                &mut prepared_destination,
                active,
                events,
            )
            .await
        };
        if let Err(error) = copy_result {
            return Err(abandon_remote_after_error(prepared_destination, error).await);
        }
        if prepared_source.root_is_file() {
            if let Err(error) = prepared_destination
                .set_entry_permissions("", prepared_source.permissions)
                .await
            {
                return Err(abandon_remote_after_error(prepared_destination, error).await);
            }
        }
        if let Err(error) = prepared_source.revalidate().await {
            return Err(abandon_remote_after_error(prepared_destination, error).await);
        }
        if let Err(error) = active.ensure_not_cancelled() {
            return Err(abandon_remote_after_error(prepared_destination, error).await);
        }
        let authoritative_destination = prepared_destination.destination.clone();
        let entry = prepared_destination.finalize().await?;
        let source_parent = prepared_source.source_parent.clone();
        let invalidated_entry_ids = prepared_source.remove_after_verified_transfer().await?;
        Ok(FileOperationOutcomeDto::Moved {
            entry: Box::new(entry),
            source_parent,
            destination: authoritative_destination,
            rebased_entry_ids: Vec::new(),
            invalidated_entry_ids,
        })
    }

    async fn run_remote_to_local(
        &self,
        local: Arc<LocalFilesystem>,
        ssh: &SshConnectionManager,
        source: &EntryRefDto,
        destination: &DirectoryRefDto,
        active: Arc<ActiveOperation>,
        events: &mut OperationEventEmitter,
    ) -> Result<FileOperationOutcomeDto, ExplorerError> {
        match self
            .transfer_remote_file_to_local(
                local.clone(),
                ssh,
                source,
                destination,
                LocalMoveConflictPolicy::Fail,
                active.clone(),
                events,
            )
            .await
        {
            Ok(outcome) => Ok(outcome),
            Err(ExplorerError::Conflict) => {
                let source_name = ssh.describe_operation_target(source).await?.0;
                let describe_local = local.clone();
                let describe_destination = destination.clone();
                let destination_name = tokio::task::spawn_blocking(move || {
                    describe_local.describe_transfer_destination(&describe_destination)
                })
                .await
                .map_err(|_| ExplorerError::StateUnavailable)??;
                let prompt_id = Uuid::new_v4().to_string();
                let decisions = vec![
                    FileOperationPromptResponseDto::KeepBoth,
                    FileOperationPromptResponseDto::Skip,
                    FileOperationPromptResponseDto::Cancel,
                ];
                let response = active.begin_prompt(prompt_id.clone(), decisions.clone())?;
                events.awaiting_conflict(FileOperationPromptDto::MoveConflict {
                    id: prompt_id,
                    title: format!("“{source_name}” already exists"),
                    message: format!(
                        "Choose how to handle the existing item in “{destination_name}”. Nothing will be replaced."
                    ),
                    target_name: source_name.clone(),
                    destination_name,
                    decisions,
                })?;
                match active.await_prompt_async(response).await? {
                    FileOperationPromptResponseDto::KeepBoth => {
                        events.running()?;
                        self.transfer_remote_file_to_local(
                            local,
                            ssh,
                            source,
                            destination,
                            LocalMoveConflictPolicy::KeepBoth,
                            active,
                            events,
                        )
                        .await
                    }
                    FileOperationPromptResponseDto::Skip => {
                        events.running()?;
                        Ok(FileOperationOutcomeDto::MoveSkipped {
                            entry: source.clone(),
                            name: source_name,
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

    #[allow(clippy::too_many_arguments)]
    async fn transfer_remote_file_to_local(
        &self,
        local: Arc<LocalFilesystem>,
        ssh: &SshConnectionManager,
        source: &EntryRefDto,
        destination: &DirectoryRefDto,
        conflict_policy: LocalMoveConflictPolicy,
        active: Arc<ActiveOperation>,
        events: &mut OperationEventEmitter,
    ) -> Result<FileOperationOutcomeDto, ExplorerError> {
        let _transfer_guard = self.transfer_guard.lock().await;
        active.ensure_not_cancelled()?;
        let prepared_source = ssh
            .prepare_transfer_source(source, &active.cancelled)
            .await?;
        if !prepared_source.root_is_file()
            && !prepared_source.root_is_symlink()
            && !prepared_source.root_is_directory()
        {
            return Err(ExplorerError::Unsupported(
                "This remote item type cannot be transferred locally.".to_owned(),
            ));
        }
        #[cfg(windows)]
        if prepared_source
            .entries()
            .iter()
            .any(|entry| entry.kind == RemoteTransferEntryKind::Symlink)
        {
            return Err(ExplorerError::Unsupported(
                "Remote symbolic links cannot be recreated safely on Windows without authoritative target-type metadata."
                    .to_owned(),
            ));
        }
        let prepare_local = local.clone();
        let prepare_destination = destination.clone();
        let prepare_name = prepared_source.name.clone();
        let prepare_active = active.clone();
        let link_target = prepared_source.root_link_target().map(str::to_owned);
        let source_is_directory = prepared_source.root_is_directory();
        let mut prepared_destination = tokio::task::spawn_blocking(move || {
            if source_is_directory {
                prepare_local.prepare_directory_transfer_destination(
                    &prepare_destination,
                    &prepare_name,
                    conflict_policy,
                    &prepare_active.cancelled,
                )
            } else if let Some(link_target) = link_target {
                prepare_local.prepare_symlink_transfer_destination(
                    &prepare_destination,
                    &prepare_name,
                    std::path::Path::new(&link_target),
                    false,
                    conflict_policy,
                    &prepare_active.cancelled,
                )
            } else {
                prepare_local.prepare_file_transfer_destination(
                    &prepare_destination,
                    &prepare_name,
                    conflict_policy,
                    &prepare_active.cancelled,
                )
            }
        })
        .await
        .map_err(|_| ExplorerError::StateUnavailable)??;
        events.byte_progress(0, prepared_source.total_bytes)?;
        if prepared_source.root_is_file() {
            copy_and_verify_remote_file_to_local(
                &prepared_source,
                &mut prepared_destination,
                &active,
                events,
            )
            .await?;
        } else if prepared_source.root_is_symlink() {
            verify_remote_symlink_to_local(&prepared_source, &prepared_destination, &active)
                .await?;
        } else {
            copy_and_verify_remote_tree_to_local(
                &prepared_source,
                &mut prepared_destination,
                &active,
                events,
            )
            .await?;
        }
        prepared_source.revalidate().await?;
        active.ensure_not_cancelled()?;

        let finalize_local = local;
        let source_permissions = prepared_source.permissions;
        let source_is_file = prepared_source.root_is_file();
        let source_is_directory = prepared_source.root_is_directory();
        let (entry, authoritative_destination) = tokio::task::spawn_blocking(move || {
            if source_is_file {
                finalize_local.finalize_received_file(prepared_destination, source_permissions)
            } else if source_is_directory {
                finalize_local.finalize_received_directory(prepared_destination)
            } else {
                finalize_local.finalize_received_symlink(prepared_destination)
            }
        })
        .await
        .map_err(|_| ExplorerError::StateUnavailable)??;
        let source_parent = prepared_source.source_parent.clone();
        let invalidated_entry_ids = prepared_source.remove_after_verified_transfer().await?;
        Ok(FileOperationOutcomeDto::Moved {
            entry: Box::new(entry),
            source_parent,
            destination: authoritative_destination,
            rebased_entry_ids: Vec::new(),
            invalidated_entry_ids,
        })
    }

    async fn run_remote_batch(
        &self,
        local: Arc<LocalFilesystem>,
        ssh: &SshConnectionManager,
        request: &FileOperationRequestDto,
        active: Arc<ActiveOperation>,
        events: &mut OperationEventEmitter,
    ) -> OperationTerminal {
        if request.sources.len() == 1 {
            return self
                .run_remote_single(local, ssh, request, active, events, false)
                .await
                .into();
        }
        if let Err(error) = ssh.validate_batch_sources(&request.sources) {
            return OperationTerminal::from(Err(error));
        }

        let delete_confirmed =
            if matches!(request.action, FileOperationActionDto::DeletePermanently {}) {
                match self
                    .confirm_remote_batch_delete(ssh, request, &active, events)
                    .await
                {
                    Ok(()) => true,
                    Err(ExplorerError::Cancelled) => {
                        return OperationTerminal::Cancelled(Some(cancelled_batch_outcome(
                            &request.sources,
                            Vec::new(),
                            0,
                        )));
                    }
                    Err(error) => return OperationTerminal::from(Err(error)),
                }
            } else {
                false
            };

        let mut items = Vec::with_capacity(request.sources.len());
        let mut successful = 0_usize;
        let mut failures = 0_usize;
        for (index, source) in request.sources.iter().enumerate() {
            events.begin_item();
            if active.ensure_not_cancelled().is_err() {
                return OperationTerminal::Cancelled(Some(cancelled_batch_outcome(
                    &request.sources,
                    items,
                    index,
                )));
            }
            let single = single_source_request(request, source.clone());
            match self
                .run_remote_single(
                    local.clone(),
                    ssh,
                    &single,
                    active.clone(),
                    events,
                    delete_confirmed,
                )
                .await
            {
                Ok(outcome) => {
                    successful += 1;
                    items.push(FileOperationBatchItemDto::Completed {
                        source: source.clone(),
                        outcome: Box::new(outcome),
                    });
                    if let Err(error) = events.settle_item() {
                        return failed_batch_terminal(
                            &request.sources,
                            items,
                            index + 1,
                            successful,
                            ExplorerErrorDto::from(error),
                        );
                    }
                }
                Err(ExplorerError::Cancelled) => {
                    items.push(FileOperationBatchItemDto::Cancelled {
                        source: source.clone(),
                    });
                    return OperationTerminal::Cancelled(Some(cancelled_batch_outcome(
                        &request.sources,
                        items,
                        index + 1,
                    )));
                }
                Err(error) => {
                    let stop = should_stop_batch(&error);
                    let error = ExplorerErrorDto::from(error);
                    failures += 1;
                    items.push(FileOperationBatchItemDto::Failed {
                        source: source.clone(),
                        error: error.clone(),
                    });
                    if let Err(progress_error) = events.settle_item() {
                        return failed_batch_terminal(
                            &request.sources,
                            items,
                            index + 1,
                            successful,
                            ExplorerErrorDto::from(progress_error),
                        );
                    }
                    if stop {
                        return failed_batch_terminal(
                            &request.sources,
                            items,
                            index + 1,
                            successful,
                            error,
                        );
                    }
                }
            }
        }

        completed_batch_terminal(items, successful, failures, request.sources.len())
    }

    async fn confirm_remote_batch_delete(
        &self,
        ssh: &SshConnectionManager,
        request: &FileOperationRequestDto,
        active: &ActiveOperation,
        events: &mut OperationEventEmitter,
    ) -> Result<(), ExplorerError> {
        let (_, location_name) = ssh.describe_operation_target(&request.sources[0]).await?;
        for source in request.sources.iter().skip(1) {
            ssh.describe_operation_target(source).await?;
        }
        let count = request.sources.len();
        let target_name = format!("{count} selected items");
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
            title: format!("Delete {count} items permanently?"),
            message: format!(
                "These remote items on {location_name} will be removed immediately. They cannot be recovered from Trash."
            ),
            target_name,
            location_name,
            confirm_label: "Delete Permanently",
        })?;
        match active.await_prompt_async(response).await? {
            FileOperationPromptResponseDto::Confirm => Ok(()),
            FileOperationPromptResponseDto::Cancel => Err(ExplorerError::Cancelled),
            FileOperationPromptResponseDto::KeepBoth | FileOperationPromptResponseDto::Skip => {
                Err(ExplorerError::InvalidConfiguration(
                    "That response is not valid for permanent deletion.".to_owned(),
                ))
            }
        }
    }

    async fn run_remote_single(
        &self,
        local: Arc<LocalFilesystem>,
        ssh: &SshConnectionManager,
        request: &FileOperationRequestDto,
        active: Arc<ActiveOperation>,
        events: &mut OperationEventEmitter,
        delete_confirmed: bool,
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
                if destination.location_id != source.location_id {
                    if destination.location_id.starts_with("ssh:") {
                        return self
                            .run_remote_to_remote(ssh, source, destination, &active, events)
                            .await;
                    }
                    return self
                        .run_remote_to_local(
                            local,
                            ssh,
                            source,
                            destination,
                            active.clone(),
                            events,
                        )
                        .await;
                }
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
                if !delete_confirmed {
                    let (target_name, location_name) =
                        ssh.describe_operation_target(source).await?;
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
                        FileOperationPromptResponseDto::Cancel => {
                            return Err(ExplorerError::Cancelled);
                        }
                        FileOperationPromptResponseDto::KeepBoth
                        | FileOperationPromptResponseDto::Skip => {
                            return Err(ExplorerError::InvalidConfiguration(
                                "That response is not valid for permanent deletion.".to_owned(),
                            ));
                        }
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

fn single_source_request(
    request: &FileOperationRequestDto,
    source: EntryRefDto,
) -> FileOperationRequestDto {
    FileOperationRequestDto {
        sources: vec![source],
        action: request.action.clone(),
    }
}

fn should_stop_batch(error: &ExplorerError) -> bool {
    !matches!(
        error,
        ExplorerError::InvalidReference
            | ExplorerError::InvalidName(_)
            | ExplorerError::Conflict
            | ExplorerError::SourceChanged
            | ExplorerError::Io { .. }
            | ExplorerError::Unsupported(_)
    )
}

fn partial_batch_error(successful: usize, total: usize) -> ExplorerErrorDto {
    ExplorerErrorDto {
        code: ExplorerErrorCode::PartialCompletion,
        message: format!(
            "Explora completed {successful} of {total} selected items. Review the item results before retrying."
        ),
    }
}

fn batch_outcome(
    status: FileOperationBatchStatusDto,
    items: Vec<FileOperationBatchItemDto>,
) -> FileOperationOutcomeDto {
    FileOperationOutcomeDto::Batch { status, items }
}

fn cancelled_batch_outcome(
    sources: &[EntryRefDto],
    mut items: Vec<FileOperationBatchItemDto>,
    next_index: usize,
) -> FileOperationOutcomeDto {
    items.extend(
        sources
            .iter()
            .skip(next_index)
            .cloned()
            .map(|source| FileOperationBatchItemDto::NotStarted { source }),
    );
    batch_outcome(FileOperationBatchStatusDto::Cancelled, items)
}

fn failed_batch_terminal(
    sources: &[EntryRefDto],
    mut items: Vec<FileOperationBatchItemDto>,
    next_index: usize,
    successful: usize,
    error: ExplorerErrorDto,
) -> OperationTerminal {
    items.extend(
        sources
            .iter()
            .skip(next_index)
            .cloned()
            .map(|source| FileOperationBatchItemDto::NotStarted { source }),
    );
    let total = sources.len();
    let terminal_error = if successful > 0 {
        partial_batch_error(successful, total)
    } else {
        error
    };
    OperationTerminal::Failed {
        error: terminal_error,
        outcome: Some(batch_outcome(FileOperationBatchStatusDto::Partial, items)),
    }
}

fn completed_batch_terminal(
    items: Vec<FileOperationBatchItemDto>,
    successful: usize,
    failures: usize,
    total: usize,
) -> OperationTerminal {
    if failures == 0 {
        OperationTerminal::Completed(batch_outcome(FileOperationBatchStatusDto::Completed, items))
    } else {
        OperationTerminal::Failed {
            error: partial_batch_error(successful, total),
            outcome: Some(batch_outcome(FileOperationBatchStatusDto::Partial, items)),
        }
    }
}

async fn verify_local_symlink_to_remote(
    source: &PreparedLocalFileTransfer,
    destination: &PreparedRemoteDestination,
    active: &ActiveOperation,
) -> Result<(), ExplorerError> {
    active.ensure_not_cancelled()?;
    let expected = source
        .plan
        .root_link_target()
        .ok_or(ExplorerError::StateUnavailable)?;
    let current = tokio::fs::read_link(source.plan.source_root())
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ExplorerError::SourceChanged
            } else {
                ExplorerError::Io {
                    message: "Explora could not inspect the local symbolic-link source.".to_owned(),
                    kind: error.kind(),
                }
            }
        })?;
    if current != expected {
        return Err(ExplorerError::SourceChanged);
    }
    let expected = expected.to_str().ok_or_else(|| {
        ExplorerError::Unsupported(
            "This symbolic-link target cannot be represented on the remote filesystem.".to_owned(),
        )
    })?;
    if destination.read_partial_link().await? != expected {
        return Err(ExplorerError::Unexpected(
            "The remote partial symbolic link does not preserve the source target.".to_owned(),
        ));
    }
    active.ensure_not_cancelled()
}

async fn verify_remote_symlink_to_remote(
    source: &PreparedRemoteTransfer,
    destination: &PreparedRemoteDestination,
    active: &ActiveOperation,
) -> Result<(), ExplorerError> {
    active.ensure_not_cancelled()?;
    let expected = source
        .root_link_target()
        .ok_or(ExplorerError::StateUnavailable)?;
    if destination.read_partial_link().await? != expected {
        return Err(ExplorerError::Unexpected(
            "The remote partial symbolic link does not preserve the source target.".to_owned(),
        ));
    }
    active.ensure_not_cancelled()
}

async fn verify_remote_symlink_to_local(
    source: &PreparedRemoteTransfer,
    destination: &PreparedLocalFileDestination,
    active: &ActiveOperation,
) -> Result<(), ExplorerError> {
    active.ensure_not_cancelled()?;
    let expected = source
        .root_link_target()
        .ok_or(ExplorerError::StateUnavailable)?;
    let actual = tokio::fs::read_link(destination.artifact.current_path())
        .await
        .map_err(|error| ExplorerError::Io {
            message: "Explora could not inspect the owned local partial symbolic link.".to_owned(),
            kind: error.kind(),
        })?;
    if actual != std::path::Path::new(expected) {
        return Err(ExplorerError::Unexpected(
            "The local partial symbolic link does not preserve the source target.".to_owned(),
        ));
    }
    active.ensure_not_cancelled()
}

async fn copy_and_verify_local_tree_to_remote(
    source: &PreparedLocalFileTransfer,
    destination: &mut PreparedRemoteDestination,
    active: &ActiveOperation,
    events: &mut OperationEventEmitter,
) -> Result<(), ExplorerError> {
    let total_bytes = source.plan.total_bytes();
    let mut completed = 0_u64;
    let mut last_emitted = 0_u64;
    for entry in source.plan.entries().iter().skip(1) {
        active.ensure_not_cancelled()?;
        let relative_path = entry.remote_relative_path()?;
        match entry.kind {
            LocalTransferEntryKind::Directory => {
                destination.create_directory_entry(&relative_path).await?;
            }
            LocalTransferEntryKind::Symlink { .. } => {
                let target = entry
                    .link_target
                    .as_deref()
                    .and_then(|target| target.to_str())
                    .ok_or_else(|| {
                        ExplorerError::Unsupported(
                            "A symbolic-link target cannot be represented on the remote filesystem."
                                .to_owned(),
                        )
                    })?;
                destination
                    .create_symlink_entry(&relative_path, target)
                    .await?;
            }
            LocalTransferEntryKind::File => {
                destination.begin_file_entry(&relative_path).await?;
                let mut source_file = tokio::fs::File::open(source.plan.source_entry_path(entry))
                    .await
                    .map_err(|error| ExplorerError::Io {
                        message: "Explora could not open a local transfer source file.".to_owned(),
                        kind: error.kind(),
                    })?;
                let mut remaining = entry.len;
                let mut buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
                while remaining > 0 {
                    active.ensure_not_cancelled()?;
                    let chunk = usize::try_from(remaining.min(TRANSFER_CHUNK_BYTES as u64))
                        .map_err(|_| ExplorerError::StateUnavailable)?;
                    let read = source_file
                        .read(&mut buffer[..chunk])
                        .await
                        .map_err(|error| ExplorerError::Io {
                            message: "Explora could not read a local transfer source file."
                                .to_owned(),
                            kind: error.kind(),
                        })?;
                    if read == 0 {
                        return Err(ExplorerError::SourceChanged);
                    }
                    let written = destination.write_chunk(&buffer[..read]).await?;
                    completed = completed
                        .checked_add(read as u64)
                        .ok_or(ExplorerError::StateUnavailable)?;
                    if written != completed {
                        return Err(ExplorerError::Unexpected(
                            "The remote partial tree has an unexpected byte count.".to_owned(),
                        ));
                    }
                    remaining -= read as u64;
                    if completed == total_bytes
                        || completed.saturating_sub(last_emitted) >= BYTE_PROGRESS_EVENT_INTERVAL
                    {
                        last_emitted = completed;
                        events.byte_progress(completed, total_bytes)?;
                    }
                }
                let mut extra = [0_u8; 1];
                if source_file
                    .read(&mut extra)
                    .await
                    .map_err(|error| ExplorerError::Io {
                        message: "Explora could not finish reading a local transfer source file."
                            .to_owned(),
                        kind: error.kind(),
                    })?
                    != 0
                {
                    return Err(ExplorerError::SourceChanged);
                }
                destination.close_for_verification().await?;
            }
        }
    }
    if destination.bytes_written()? != total_bytes {
        return Err(ExplorerError::Unexpected(
            "The remote partial tree has an unexpected byte count.".to_owned(),
        ));
    }

    for entry in source.plan.entries() {
        active.ensure_not_cancelled()?;
        let relative_path = entry.remote_relative_path()?;
        let metadata = destination.entry_metadata(&relative_path).await?;
        match entry.kind {
            LocalTransferEntryKind::Directory if metadata.is_dir() => {}
            LocalTransferEntryKind::Symlink { .. } if metadata.is_symlink() => {
                let expected = entry
                    .link_target
                    .as_deref()
                    .and_then(|target| target.to_str())
                    .ok_or(ExplorerError::StateUnavailable)?;
                if destination.read_link_entry(&relative_path).await? != expected {
                    return Err(ExplorerError::Unexpected(
                        "A remote partial symbolic link does not preserve its source target."
                            .to_owned(),
                    ));
                }
            }
            LocalTransferEntryKind::File
                if metadata.is_regular() && metadata.size.unwrap_or(0) == entry.len =>
            {
                verify_local_file_entry_to_remote(
                    source.plan.source_entry_path(entry),
                    &relative_path,
                    entry.len,
                    destination,
                    active,
                )
                .await?;
            }
            _ => {
                return Err(ExplorerError::Unexpected(
                    "The remote partial tree does not match the source structure.".to_owned(),
                ));
            }
        }
    }
    for entry in source.plan.entries().iter().rev() {
        if !matches!(entry.kind, LocalTransferEntryKind::Symlink { .. }) {
            destination
                .set_entry_permissions(&entry.remote_relative_path()?, entry.remote_permissions())
                .await?;
        }
    }
    active.ensure_not_cancelled()
}

async fn verify_local_file_entry_to_remote(
    source_path: std::path::PathBuf,
    relative_path: &str,
    expected_len: u64,
    destination: &PreparedRemoteDestination,
    active: &ActiveOperation,
) -> Result<(), ExplorerError> {
    let mut source =
        tokio::fs::File::open(source_path)
            .await
            .map_err(|error| ExplorerError::Io {
                message: "Explora could not reopen a local transfer source file.".to_owned(),
                kind: error.kind(),
            })?;
    let mut target = destination
        .open_entry_for_verification(relative_path)
        .await?;
    compare_local_and_remote_files(&mut source, &mut target, expected_len, active).await
}

async fn compare_local_and_remote_files(
    local: &mut tokio::fs::File,
    remote: &mut russh_sftp::client::fs::File,
    expected_len: u64,
    active: &ActiveOperation,
) -> Result<(), ExplorerError> {
    let mut local_buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
    let mut remote_buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
    let mut remaining = expected_len;
    while remaining > 0 {
        active.ensure_not_cancelled()?;
        let chunk = usize::try_from(remaining.min(TRANSFER_CHUNK_BYTES as u64))
            .map_err(|_| ExplorerError::StateUnavailable)?;
        local
            .read_exact(&mut local_buffer[..chunk])
            .await
            .map_err(|_| ExplorerError::SourceChanged)?;
        remote
            .read_exact(&mut remote_buffer[..chunk])
            .await
            .map_err(|_| {
                ExplorerError::Unexpected(
                    "A remote partial file ended before verification completed.".to_owned(),
                )
            })?;
        if local_buffer[..chunk] != remote_buffer[..chunk] {
            return Err(ExplorerError::SourceChanged);
        }
        remaining -= chunk as u64;
    }
    let mut extra = [0_u8; 1];
    if local
        .read(&mut extra)
        .await
        .map_err(|error| ExplorerError::Io {
            message: "Explora could not finish verifying a local source file.".to_owned(),
            kind: error.kind(),
        })?
        != 0
    {
        return Err(ExplorerError::SourceChanged);
    }
    if remote.read(&mut extra).await.map_err(|_| {
        ExplorerError::Offline(
            "The SSH connection was lost while verifying a remote partial file.".to_owned(),
        )
    })? != 0
    {
        return Err(ExplorerError::Unexpected(
            "A remote partial file grew during verification.".to_owned(),
        ));
    }
    Ok(())
}

async fn copy_and_verify_remote_tree_to_remote(
    source: &PreparedRemoteTransfer,
    destination: &mut PreparedRemoteDestination,
    active: &ActiveOperation,
    events: &mut OperationEventEmitter,
) -> Result<(), ExplorerError> {
    let mut completed = 0_u64;
    let mut last_emitted = 0_u64;
    for entry in source.entries().iter().skip(1) {
        active.ensure_not_cancelled()?;
        match entry.kind {
            RemoteTransferEntryKind::Directory => {
                destination
                    .create_directory_entry(&entry.relative_path)
                    .await?;
            }
            RemoteTransferEntryKind::Symlink => {
                destination
                    .create_symlink_entry(
                        &entry.relative_path,
                        entry
                            .link_target
                            .as_deref()
                            .ok_or(ExplorerError::StateUnavailable)?,
                    )
                    .await?;
            }
            RemoteTransferEntryKind::File => {
                destination.begin_file_entry(&entry.relative_path).await?;
                let mut source_file = source.open_entry_for_read(&entry.relative_path).await?;
                let mut remaining = entry.len;
                let mut buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
                while remaining > 0 {
                    active.ensure_not_cancelled()?;
                    let chunk = usize::try_from(remaining.min(TRANSFER_CHUNK_BYTES as u64))
                        .map_err(|_| ExplorerError::StateUnavailable)?;
                    let read = source_file.read(&mut buffer[..chunk]).await.map_err(|_| {
                        source.connection_error(
                            "The SSH connection was lost while reading a remote tree source.",
                        )
                    })?;
                    if read == 0 {
                        return Err(ExplorerError::SourceChanged);
                    }
                    let written = destination.write_chunk(&buffer[..read]).await?;
                    completed = completed
                        .checked_add(read as u64)
                        .ok_or(ExplorerError::StateUnavailable)?;
                    if written != completed {
                        return Err(ExplorerError::Unexpected(
                            "The remote partial tree has an unexpected byte count.".to_owned(),
                        ));
                    }
                    remaining -= read as u64;
                    if completed == source.total_bytes
                        || completed.saturating_sub(last_emitted) >= BYTE_PROGRESS_EVENT_INTERVAL
                    {
                        last_emitted = completed;
                        events.byte_progress(completed, source.total_bytes)?;
                    }
                }
                let mut extra = [0_u8; 1];
                if source_file.read(&mut extra).await.map_err(|_| {
                    source.connection_error(
                        "The SSH connection was lost while finishing a remote tree source read.",
                    )
                })? != 0
                {
                    return Err(ExplorerError::SourceChanged);
                }
                destination.close_for_verification().await?;
            }
        }
    }
    if destination.bytes_written()? != source.total_bytes {
        return Err(ExplorerError::Unexpected(
            "The remote partial tree has an unexpected byte count.".to_owned(),
        ));
    }

    for entry in source.entries() {
        active.ensure_not_cancelled()?;
        let metadata = destination.entry_metadata(&entry.relative_path).await?;
        match entry.kind {
            RemoteTransferEntryKind::Directory if metadata.is_dir() => {}
            RemoteTransferEntryKind::Symlink if metadata.is_symlink() => {
                if destination.read_link_entry(&entry.relative_path).await?
                    != entry
                        .link_target
                        .as_deref()
                        .ok_or(ExplorerError::StateUnavailable)?
                {
                    return Err(ExplorerError::Unexpected(
                        "A remote partial symbolic link does not preserve its source target."
                            .to_owned(),
                    ));
                }
            }
            RemoteTransferEntryKind::File
                if metadata.is_regular() && metadata.size.unwrap_or(0) == entry.len =>
            {
                verify_remote_file_entry_to_remote(
                    source,
                    &entry.relative_path,
                    entry.len,
                    destination,
                    active,
                )
                .await?;
            }
            _ => {
                return Err(ExplorerError::Unexpected(
                    "The remote partial tree does not match the source structure.".to_owned(),
                ));
            }
        }
    }
    for entry in source.entries().iter().rev() {
        if entry.kind != RemoteTransferEntryKind::Symlink {
            destination
                .set_entry_permissions(&entry.relative_path, entry.permissions)
                .await?;
        }
    }
    active.ensure_not_cancelled()
}

async fn verify_remote_file_entry_to_remote(
    source: &PreparedRemoteTransfer,
    relative_path: &str,
    expected_len: u64,
    destination: &PreparedRemoteDestination,
    active: &ActiveOperation,
) -> Result<(), ExplorerError> {
    let mut source_file = source.open_entry_for_read(relative_path).await?;
    let mut destination_file = destination
        .open_entry_for_verification(relative_path)
        .await?;
    let mut source_buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
    let mut destination_buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
    let mut remaining = expected_len;
    while remaining > 0 {
        active.ensure_not_cancelled()?;
        let chunk = usize::try_from(remaining.min(TRANSFER_CHUNK_BYTES as u64))
            .map_err(|_| ExplorerError::StateUnavailable)?;
        source_file
            .read_exact(&mut source_buffer[..chunk])
            .await
            .map_err(|_| ExplorerError::SourceChanged)?;
        destination_file
            .read_exact(&mut destination_buffer[..chunk])
            .await
            .map_err(|_| {
                destination.connection_error(
                    "The SSH connection was lost while verifying a remote tree destination.",
                )
            })?;
        if source_buffer[..chunk] != destination_buffer[..chunk] {
            return Err(ExplorerError::SourceChanged);
        }
        remaining -= chunk as u64;
    }
    let mut extra = [0_u8; 1];
    if source_file.read(&mut extra).await.map_err(|_| {
        source.connection_error("The SSH connection was lost while verifying a remote tree source.")
    })? != 0
    {
        return Err(ExplorerError::SourceChanged);
    }
    if destination_file.read(&mut extra).await.map_err(|_| {
        destination.connection_error(
            "The SSH connection was lost while finishing remote tree verification.",
        )
    })? != 0
    {
        return Err(ExplorerError::Unexpected(
            "A remote partial file grew during verification.".to_owned(),
        ));
    }
    Ok(())
}

async fn copy_and_verify_remote_tree_to_local(
    source: &PreparedRemoteTransfer,
    destination: &mut PreparedLocalFileDestination,
    active: &ActiveOperation,
    events: &mut OperationEventEmitter,
) -> Result<(), ExplorerError> {
    let mut completed = 0_u64;
    let mut last_emitted = 0_u64;
    for entry in source.entries().iter().skip(1) {
        active.ensure_not_cancelled()?;
        let relative_path = remote_relative_to_local(&entry.relative_path)?;
        match entry.kind {
            RemoteTransferEntryKind::Directory => {
                destination
                    .artifact
                    .create_directory_entry(&relative_path)?;
            }
            RemoteTransferEntryKind::Symlink => {
                destination.artifact.create_symlink_entry(
                    &relative_path,
                    std::path::Path::new(
                        entry
                            .link_target
                            .as_deref()
                            .ok_or(ExplorerError::StateUnavailable)?,
                    ),
                    false,
                )?;
            }
            RemoteTransferEntryKind::File => {
                let local_file = destination.artifact.create_file_entry(&relative_path)?;
                let mut local_file = tokio::fs::File::from_std(local_file);
                let mut source_file = source.open_entry_for_read(&entry.relative_path).await?;
                let mut remaining = entry.len;
                let mut buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
                while remaining > 0 {
                    active.ensure_not_cancelled()?;
                    let chunk = usize::try_from(remaining.min(TRANSFER_CHUNK_BYTES as u64))
                        .map_err(|_| ExplorerError::StateUnavailable)?;
                    let read = source_file.read(&mut buffer[..chunk]).await.map_err(|_| {
                        source.connection_error(
                            "The SSH connection was lost while reading a remote tree source.",
                        )
                    })?;
                    if read == 0 {
                        return Err(ExplorerError::SourceChanged);
                    }
                    local_file
                        .write_all(&buffer[..read])
                        .await
                        .map_err(|error| ExplorerError::Io {
                            message: "Explora could not write an owned local partial file."
                                .to_owned(),
                            kind: error.kind(),
                        })?;
                    completed = completed
                        .checked_add(read as u64)
                        .ok_or(ExplorerError::StateUnavailable)?;
                    remaining -= read as u64;
                    if completed == source.total_bytes
                        || completed.saturating_sub(last_emitted) >= BYTE_PROGRESS_EVENT_INTERVAL
                    {
                        last_emitted = completed;
                        events.byte_progress(completed, source.total_bytes)?;
                    }
                }
                let mut extra = [0_u8; 1];
                if source_file.read(&mut extra).await.map_err(|_| {
                    source.connection_error(
                        "The SSH connection was lost while finishing a remote tree source read.",
                    )
                })? != 0
                {
                    return Err(ExplorerError::SourceChanged);
                }
                local_file
                    .flush()
                    .await
                    .map_err(|error| ExplorerError::Io {
                        message: "Explora could not flush an owned local partial file.".to_owned(),
                        kind: error.kind(),
                    })?;
                local_file
                    .sync_all()
                    .await
                    .map_err(|error| ExplorerError::Io {
                        message: "Explora could not synchronize an owned local partial file."
                            .to_owned(),
                        kind: error.kind(),
                    })?;
            }
        }
    }
    if completed != source.total_bytes {
        return Err(ExplorerError::Unexpected(
            "The local partial tree has an unexpected byte count.".to_owned(),
        ));
    }

    for entry in source.entries() {
        active.ensure_not_cancelled()?;
        let relative_path = remote_relative_to_local(&entry.relative_path)?;
        let destination_path = destination.artifact.entry_path(&relative_path)?;
        let metadata = tokio::fs::symlink_metadata(&destination_path)
            .await
            .map_err(|error| ExplorerError::Io {
                message: "Explora could not inspect the local partial tree.".to_owned(),
                kind: error.kind(),
            })?;
        let file_type = metadata.file_type();
        match entry.kind {
            RemoteTransferEntryKind::Directory if file_type.is_dir() => {}
            RemoteTransferEntryKind::Symlink if file_type.is_symlink() => {
                let target = tokio::fs::read_link(&destination_path)
                    .await
                    .map_err(|error| ExplorerError::Io {
                        message: "Explora could not inspect a local partial symbolic link."
                            .to_owned(),
                        kind: error.kind(),
                    })?;
                if target
                    != std::path::Path::new(
                        entry
                            .link_target
                            .as_deref()
                            .ok_or(ExplorerError::StateUnavailable)?,
                    )
                {
                    return Err(ExplorerError::Unexpected(
                        "A local partial symbolic link does not preserve its source target."
                            .to_owned(),
                    ));
                }
            }
            RemoteTransferEntryKind::File if file_type.is_file() && metadata.len() == entry.len => {
                verify_remote_file_entry_to_local(
                    source,
                    &entry.relative_path,
                    &destination_path,
                    entry.len,
                    active,
                )
                .await?;
            }
            _ => {
                return Err(ExplorerError::Unexpected(
                    "The local partial tree does not match the source structure.".to_owned(),
                ));
            }
        }
    }
    for entry in source.entries().iter().rev() {
        if entry.kind != RemoteTransferEntryKind::Symlink {
            LocalFilesystem::apply_received_permissions(
                destination,
                &remote_relative_to_local(&entry.relative_path)?,
                entry.permissions,
            )?;
        }
    }
    active.ensure_not_cancelled()
}

async fn verify_remote_file_entry_to_local(
    source: &PreparedRemoteTransfer,
    relative_path: &str,
    destination_path: &std::path::Path,
    expected_len: u64,
    active: &ActiveOperation,
) -> Result<(), ExplorerError> {
    let mut source_file = source.open_entry_for_read(relative_path).await?;
    let mut destination_file = tokio::fs::File::open(destination_path)
        .await
        .map_err(|error| ExplorerError::Io {
            message: "Explora could not reopen a local partial file.".to_owned(),
            kind: error.kind(),
        })?;
    let mut source_buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
    let mut destination_buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
    let mut remaining = expected_len;
    while remaining > 0 {
        active.ensure_not_cancelled()?;
        let chunk = usize::try_from(remaining.min(TRANSFER_CHUNK_BYTES as u64))
            .map_err(|_| ExplorerError::StateUnavailable)?;
        source_file
            .read_exact(&mut source_buffer[..chunk])
            .await
            .map_err(|_| ExplorerError::SourceChanged)?;
        destination_file
            .read_exact(&mut destination_buffer[..chunk])
            .await
            .map_err(|_| {
                ExplorerError::Unexpected(
                    "A local partial file ended before verification completed.".to_owned(),
                )
            })?;
        if source_buffer[..chunk] != destination_buffer[..chunk] {
            return Err(ExplorerError::SourceChanged);
        }
        remaining -= chunk as u64;
    }
    let mut extra = [0_u8; 1];
    if source_file.read(&mut extra).await.map_err(|_| {
        source.connection_error("The SSH connection was lost while verifying a remote tree source.")
    })? != 0
    {
        return Err(ExplorerError::SourceChanged);
    }
    if destination_file
        .read(&mut extra)
        .await
        .map_err(|error| ExplorerError::Io {
            message: "Explora could not finish verifying a local partial file.".to_owned(),
            kind: error.kind(),
        })?
        != 0
    {
        return Err(ExplorerError::Unexpected(
            "A local partial file grew during verification.".to_owned(),
        ));
    }
    Ok(())
}

fn remote_relative_to_local(relative_path: &str) -> Result<std::path::PathBuf, ExplorerError> {
    if relative_path.is_empty() {
        return Ok(std::path::PathBuf::new());
    }
    let mut result = std::path::PathBuf::new();
    for component in relative_path.split('/') {
        if component.is_empty() || component == "." || component == ".." || component.contains('\0')
        {
            return Err(ExplorerError::InvalidReference);
        }
        result.push(component);
    }
    Ok(result)
}

async fn copy_and_verify_local_file_to_remote(
    prepared: &PreparedLocalFileTransfer,
    remote: &mut PreparedRemoteDestination,
    active: &ActiveOperation,
    events: &mut OperationEventEmitter,
) -> Result<(), ExplorerError> {
    let total_bytes = prepared.plan.total_bytes();
    let mut source = tokio::fs::File::open(prepared.plan.source_root())
        .await
        .map_err(|error| ExplorerError::Io {
            message: "Explora could not open the local transfer source.".to_owned(),
            kind: error.kind(),
        })?;
    let mut buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
    let mut completed = 0_u64;
    let mut last_emitted = 0_u64;
    while completed < total_bytes {
        active.ensure_not_cancelled()?;
        let remaining = usize::try_from((total_bytes - completed).min(TRANSFER_CHUNK_BYTES as u64))
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let read = source
            .read(&mut buffer[..remaining])
            .await
            .map_err(|error| ExplorerError::Io {
                message: "Explora could not read the local transfer source.".to_owned(),
                kind: error.kind(),
            })?;
        if read == 0 {
            return Err(ExplorerError::SourceChanged);
        }
        completed = remote.write_chunk(&buffer[..read]).await?;
        if completed == total_bytes
            || completed.saturating_sub(last_emitted) >= BYTE_PROGRESS_EVENT_INTERVAL
        {
            last_emitted = completed;
            events.byte_progress(completed, total_bytes)?;
        }
    }
    let mut extra = [0_u8; 1];
    if source
        .read(&mut extra)
        .await
        .map_err(|error| ExplorerError::Io {
            message: "Explora could not finish reading the local transfer source.".to_owned(),
            kind: error.kind(),
        })?
        != 0
    {
        return Err(ExplorerError::SourceChanged);
    }
    if remote.bytes_written()? != total_bytes {
        return Err(ExplorerError::Unexpected(
            "The remote partial file has an unexpected size.".to_owned(),
        ));
    }
    active.ensure_not_cancelled()?;
    remote.close_for_verification().await?;

    let mut source = tokio::fs::File::open(prepared.plan.source_root())
        .await
        .map_err(|error| ExplorerError::Io {
            message: "Explora could not reopen the local transfer source.".to_owned(),
            kind: error.kind(),
        })?;
    let mut destination = remote.open_partial_for_verification().await?;
    let mut source_buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
    let mut destination_buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
    let mut remaining = total_bytes;
    while remaining > 0 {
        active.ensure_not_cancelled()?;
        let chunk = usize::try_from(remaining.min(TRANSFER_CHUNK_BYTES as u64))
            .map_err(|_| ExplorerError::StateUnavailable)?;
        source
            .read_exact(&mut source_buffer[..chunk])
            .await
            .map_err(|_| ExplorerError::SourceChanged)?;
        destination
            .read_exact(&mut destination_buffer[..chunk])
            .await
            .map_err(|_| {
                ExplorerError::Unexpected(
                    "The remote partial file ended before verification completed.".to_owned(),
                )
            })?;
        if source_buffer[..chunk] != destination_buffer[..chunk] {
            return Err(ExplorerError::SourceChanged);
        }
        remaining -= chunk as u64;
    }
    let source_extra = source
        .read(&mut extra)
        .await
        .map_err(|error| ExplorerError::Io {
            message: "Explora could not finish verifying the local source.".to_owned(),
            kind: error.kind(),
        })?;
    let destination_extra = destination.read(&mut extra).await.map_err(|_| {
        ExplorerError::Offline(
            "The SSH connection was lost while verifying the remote partial file.".to_owned(),
        )
    })?;
    if source_extra != 0 {
        return Err(ExplorerError::SourceChanged);
    }
    if destination_extra != 0 {
        return Err(ExplorerError::Unexpected(
            "The remote partial file grew during verification.".to_owned(),
        ));
    }
    Ok(())
}

async fn copy_and_verify_remote_file_to_remote(
    source: &PreparedRemoteTransfer,
    destination: &mut PreparedRemoteDestination,
    active: &ActiveOperation,
    events: &mut OperationEventEmitter,
) -> Result<(), ExplorerError> {
    let total_bytes = source.total_bytes;
    let mut source_file = source.open_for_read().await?;
    let mut buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
    let mut completed = 0_u64;
    let mut last_emitted = 0_u64;
    while completed < total_bytes {
        active.ensure_not_cancelled()?;
        let chunk = usize::try_from((total_bytes - completed).min(TRANSFER_CHUNK_BYTES as u64))
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let read = source_file.read(&mut buffer[..chunk]).await.map_err(|_| {
            source.connection_error(
                "The SSH connection was lost while reading the remote transfer source.",
            )
        })?;
        if read == 0 {
            return Err(ExplorerError::SourceChanged);
        }
        completed = destination.write_chunk(&buffer[..read]).await?;
        if completed == total_bytes
            || completed.saturating_sub(last_emitted) >= BYTE_PROGRESS_EVENT_INTERVAL
        {
            last_emitted = completed;
            events.byte_progress(completed, total_bytes)?;
        }
    }
    let mut extra = [0_u8; 1];
    if source_file.read(&mut extra).await.map_err(|_| {
        source
            .connection_error("The SSH connection was lost while finishing the remote source read.")
    })? != 0
    {
        return Err(ExplorerError::SourceChanged);
    }
    if destination.bytes_written()? != total_bytes {
        return Err(ExplorerError::Unexpected(
            "The remote partial file has an unexpected size.".to_owned(),
        ));
    }
    active.ensure_not_cancelled()?;
    destination.close_for_verification().await?;

    let mut source_file = source.open_for_read().await?;
    let mut destination_file = destination.open_partial_for_verification().await?;
    let mut source_buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
    let mut destination_buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
    let mut remaining = total_bytes;
    while remaining > 0 {
        active.ensure_not_cancelled()?;
        let chunk = usize::try_from(remaining.min(TRANSFER_CHUNK_BYTES as u64))
            .map_err(|_| ExplorerError::StateUnavailable)?;
        source_file
            .read_exact(&mut source_buffer[..chunk])
            .await
            .map_err(|_| ExplorerError::SourceChanged)?;
        destination_file
            .read_exact(&mut destination_buffer[..chunk])
            .await
            .map_err(|_| {
                destination.connection_error(
                    "The SSH connection was lost while verifying the remote transfer destination.",
                )
            })?;
        if source_buffer[..chunk] != destination_buffer[..chunk] {
            return Err(ExplorerError::SourceChanged);
        }
        remaining -= chunk as u64;
    }
    let source_extra = source_file.read(&mut extra).await.map_err(|_| {
        source.connection_error("The SSH connection was lost while finishing source verification.")
    })?;
    let destination_extra = destination_file.read(&mut extra).await.map_err(|_| {
        destination.connection_error(
            "The SSH connection was lost while finishing destination verification.",
        )
    })?;
    if source_extra != 0 {
        return Err(ExplorerError::SourceChanged);
    }
    if destination_extra != 0 {
        return Err(ExplorerError::Unexpected(
            "The remote partial file grew during verification.".to_owned(),
        ));
    }
    Ok(())
}

async fn copy_and_verify_remote_file_to_local(
    source: &PreparedRemoteTransfer,
    destination: &mut PreparedLocalFileDestination,
    active: &ActiveOperation,
    events: &mut OperationEventEmitter,
) -> Result<(), ExplorerError> {
    let total_bytes = source.total_bytes;
    let mut source_file = source.open_for_read().await?;
    let local_file = destination.artifact.take_file()?;
    let mut local_file = tokio::fs::File::from_std(local_file);
    let mut buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
    let mut completed = 0_u64;
    let mut last_emitted = 0_u64;
    while completed < total_bytes {
        active.ensure_not_cancelled()?;
        let chunk = usize::try_from((total_bytes - completed).min(TRANSFER_CHUNK_BYTES as u64))
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let read = source_file.read(&mut buffer[..chunk]).await.map_err(|_| {
            source.connection_error(
                "The SSH connection was lost while reading the remote transfer source.",
            )
        })?;
        if read == 0 {
            return Err(ExplorerError::SourceChanged);
        }
        local_file
            .write_all(&buffer[..read])
            .await
            .map_err(|error| ExplorerError::Io {
                message: "Explora could not write the owned local partial file.".to_owned(),
                kind: error.kind(),
            })?;
        completed = completed.checked_add(read as u64).ok_or_else(|| {
            ExplorerError::InvalidConfiguration(
                "The transfer exceeded the supported size.".to_owned(),
            )
        })?;
        if completed == total_bytes
            || completed.saturating_sub(last_emitted) >= BYTE_PROGRESS_EVENT_INTERVAL
        {
            last_emitted = completed;
            events.byte_progress(completed, total_bytes)?;
        }
    }
    let mut extra = [0_u8; 1];
    if source_file.read(&mut extra).await.map_err(|_| {
        source
            .connection_error("The SSH connection was lost while finishing the remote source read.")
    })? != 0
    {
        return Err(ExplorerError::SourceChanged);
    }
    active.ensure_not_cancelled()?;
    local_file
        .flush()
        .await
        .map_err(|error| ExplorerError::Io {
            message: "Explora could not flush the local partial file.".to_owned(),
            kind: error.kind(),
        })?;
    local_file
        .sync_all()
        .await
        .map_err(|error| ExplorerError::Io {
            message: "Explora could not synchronize the local partial file.".to_owned(),
            kind: error.kind(),
        })?;
    let local_file = local_file.into_std().await;
    destination.artifact.restore_file(local_file, completed)?;

    let mut source_file = source.open_for_read().await?;
    let mut local_file = tokio::fs::File::open(destination.artifact.current_path())
        .await
        .map_err(|error| ExplorerError::Io {
            message: "Explora could not reopen the local partial file.".to_owned(),
            kind: error.kind(),
        })?;
    let mut source_buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
    let mut destination_buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
    let mut remaining = total_bytes;
    while remaining > 0 {
        active.ensure_not_cancelled()?;
        let chunk = usize::try_from(remaining.min(TRANSFER_CHUNK_BYTES as u64))
            .map_err(|_| ExplorerError::StateUnavailable)?;
        source_file
            .read_exact(&mut source_buffer[..chunk])
            .await
            .map_err(|_| ExplorerError::SourceChanged)?;
        local_file
            .read_exact(&mut destination_buffer[..chunk])
            .await
            .map_err(|_| {
                ExplorerError::Unexpected(
                    "The local partial file ended before verification completed.".to_owned(),
                )
            })?;
        if source_buffer[..chunk] != destination_buffer[..chunk] {
            return Err(ExplorerError::SourceChanged);
        }
        remaining -= chunk as u64;
    }
    let source_extra = source_file.read(&mut extra).await.map_err(|_| {
        source.connection_error("The SSH connection was lost while finishing source verification.")
    })?;
    let destination_extra =
        local_file
            .read(&mut extra)
            .await
            .map_err(|error| ExplorerError::Io {
                message: "Explora could not finish verifying the local partial file.".to_owned(),
                kind: error.kind(),
            })?;
    if source_extra != 0 {
        return Err(ExplorerError::SourceChanged);
    }
    if destination_extra != 0 {
        return Err(ExplorerError::Unexpected(
            "The local partial file grew during verification.".to_owned(),
        ));
    }
    Ok(())
}

async fn abandon_remote_after_error(
    remote: PreparedRemoteDestination,
    original: ExplorerError,
) -> ExplorerError {
    match remote.abandon().await {
        Ok(()) => original,
        Err(_) => ExplorerError::PartialCompletion(
            "The local source was preserved, but the remote partial file could not be cleaned up. Reconnect and remove the .explora-partial file before retrying."
                .to_owned(),
        ),
    }
}

fn validate_request(request: &FileOperationRequestDto) -> Result<(), ExplorerError> {
    if request.sources.is_empty() || request.sources.len() > MAX_OPERATION_SOURCES {
        return Err(ExplorerError::InvalidConfiguration(format!(
            "Filesystem actions require between 1 and {MAX_OPERATION_SOURCES} selected items."
        )));
    }
    if matches!(request.action, FileOperationActionDto::Rename { .. }) && request.sources.len() != 1
    {
        return Err(ExplorerError::InvalidConfiguration(
            "Rename requires exactly one selected item.".to_owned(),
        ));
    }

    let source_location = &request.sources[0].location_id;
    let mut unique_sources = HashSet::with_capacity(request.sources.len());
    for source in &request.sources {
        if source.id.is_empty()
            || source.id.len() > 256
            || source.location_id.is_empty()
            || source.location_id.len() > 256
        {
            return Err(ExplorerError::InvalidReference);
        }
        if source.location_id != *source_location {
            return Err(ExplorerError::InvalidConfiguration(
                "A batch filesystem action must use items from one location.".to_owned(),
            ));
        }
        if !unique_sources.insert((source.location_id.as_str(), source.id.as_str())) {
            return Err(ExplorerError::InvalidConfiguration(
                "A selected item can appear only once in a filesystem action.".to_owned(),
            ));
        }
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
        invalidated_entry_ids: Vec::new(),
    }
}

fn moved_remote_outcome(entry: MovedRemoteEntry) -> FileOperationOutcomeDto {
    FileOperationOutcomeDto::Moved {
        entry: Box::new(entry.entry),
        source_parent: entry.source_parent,
        destination: entry.destination,
        rebased_entry_ids: entry.rebased_entry_ids,
        invalidated_entry_ids: Vec::new(),
    }
}

fn transferred_local_outcome(entry: TransferredLocalEntry) -> FileOperationOutcomeDto {
    FileOperationOutcomeDto::Moved {
        entry: Box::new(entry.entry),
        source_parent: entry.source_parent,
        destination: entry.destination,
        rebased_entry_ids: Vec::new(),
        invalidated_entry_ids: entry.invalidated_entry_ids,
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

    struct PausingTrash {
        destination: PathBuf,
        first_moved: StdMutex<Option<std::sync::mpsc::Sender<()>>>,
        release_first: StdMutex<std::sync::mpsc::Receiver<()>>,
    }

    impl PlatformTrash for PausingTrash {
        fn is_available(&self) -> bool {
            true
        }

        fn move_to_trash(&self, path: &Path) -> Result<(), ExplorerError> {
            let name = path.file_name().ok_or(ExplorerError::InvalidReference)?;
            fs::rename(path, self.destination.join(name))
                .map_err(|error| ExplorerError::io("trash", path, error))?;
            let notify = self
                .first_moved
                .lock()
                .map_err(|_| ExplorerError::StateUnavailable)?
                .take();
            if let Some(notify) = notify {
                notify
                    .send(())
                    .map_err(|_| ExplorerError::StateUnavailable)?;
                self.release_first
                    .lock()
                    .map_err(|_| ExplorerError::StateUnavailable)?
                    .recv_timeout(Duration::from_secs(2))
                    .map_err(|_| ExplorerError::StateUnavailable)?;
            }
            Ok(())
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

    fn transfer_fixture() -> (TempDir, Arc<LocalFilesystem>, EntryRefDto, DirectoryRefDto) {
        let temp = TempDir::new().expect("temporary directory");
        let source_path = temp.path().join("source");
        let destination_path = temp.path().join("destination");
        fs::create_dir(&source_path).expect("source root");
        fs::create_dir(&destination_path).expect("destination root");
        fs::write(source_path.join("notes.md"), vec![0x33; 600_000]).expect("fixture file");
        let local = Arc::new(
            LocalFilesystem::new(vec![
                LocalRoot {
                    id: "operation-source",
                    name: "Source",
                    role: LocationRole::Home,
                    path: source_path,
                },
                LocalRoot {
                    id: "operation-destination",
                    name: "Destination",
                    role: LocationRole::Volume,
                    path: destination_path,
                },
            ])
            .expect("local filesystem"),
        );
        let locations = local.locations().expect("locations");
        let source_root = locations
            .iter()
            .find(|location| location.id == "operation-source")
            .expect("source location")
            .root
            .clone();
        let destination_root = locations
            .iter()
            .find(|location| location.id == "operation-destination")
            .expect("destination location")
            .root
            .clone();
        let mut source = None;
        local
            .list_directory(
                &source_root.id,
                &source_root.location_id,
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
            .expect("source listing");
        (temp, local, source.expect("source entry"), destination_root)
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
        connect_remote_fixture_as(server, manager, "operation-test-target").await
    }

    async fn connect_remote_fixture_as(
        server: &TestSshServer,
        manager: Arc<SshConnectionManager>,
        target_id: &str,
    ) -> LocationSummaryDto {
        let request_id = format!("operation-remote-connect-{target_id}");
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
                    id: target_id.to_owned(),
                    name: format!("Operation test server {target_id}"),
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
                let captured = events.lock().expect("events").clone();
                if captured
                    .iter()
                    .any(|event| event.get("event").and_then(Value::as_str) == Some(expected))
                {
                    break;
                }
                if let Some(terminal) = captured.iter().find(|event| {
                    matches!(
                        event.get("event").and_then(Value::as_str),
                        Some("completed" | "failed" | "cancelled")
                    )
                }) {
                    panic!("operation reached unexpected terminal event: {terminal}");
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

        let duplicate = FileOperationRequestDto {
            sources: vec![
                EntryRefDto {
                    id: "entry".to_owned(),
                    location_id: "home".to_owned(),
                },
                EntryRefDto {
                    id: "entry".to_owned(),
                    location_id: "home".to_owned(),
                },
            ],
            action: FileOperationActionDto::Trash {},
        };
        assert!(matches!(
            validate_request(&duplicate),
            Err(ExplorerError::InvalidConfiguration(_))
        ));

        let mixed_locations = FileOperationRequestDto {
            sources: vec![
                EntryRefDto {
                    id: "first".to_owned(),
                    location_id: "home".to_owned(),
                },
                EntryRefDto {
                    id: "second".to_owned(),
                    location_id: "ssh:server".to_owned(),
                },
            ],
            action: FileOperationActionDto::Trash {},
        };
        assert!(matches!(
            validate_request(&mixed_locations),
            Err(ExplorerError::InvalidConfiguration(_))
        ));

        let batch_rename = FileOperationRequestDto {
            sources: vec![
                EntryRefDto {
                    id: "first".to_owned(),
                    location_id: "home".to_owned(),
                },
                EntryRefDto {
                    id: "second".to_owned(),
                    location_id: "home".to_owned(),
                },
            ],
            action: FileOperationActionDto::Rename {
                new_name: "renamed".to_owned(),
            },
        };
        assert!(matches!(
            validate_request(&batch_rename),
            Err(ExplorerError::InvalidConfiguration(_))
        ));
    }

    #[test]
    fn terminal_events_keep_identity_action_progress_and_sequence() {
        let event = FileOperationEventDto::Cancelled {
            operation_id: "operation-1".to_owned(),
            sequence: 2,
            action: FileOperationKindDto::Trash,
            completed_items: 0,
            total_items: 1,
            completed_bytes: None,
            total_bytes: None,
            current_item_completed: None,
            current_item_total: None,
            outcome: None,
        };
        assert_eq!(
            serde_json::to_value(event).expect("serializable event"),
            json!({
                "event": "cancelled",
                "operationId": "operation-1",
                "sequence": 2,
                "action": "trash",
                "completedItems": 0,
                "totalItems": 1,
                "completedBytes": null,
                "totalBytes": null,
                "currentItemCompleted": null,
                "currentItemTotal": null,
                "outcome": null
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn batch_trash_returns_structured_partial_results_and_continues_safely() {
        let (temp, local, source) = fixture();
        fs::write(temp.path().join("second.txt"), b"second").expect("second source");
        let changed_source = listed_entry_ref(&local, "second.txt");
        let trash_destination = temp.path().join("native-trash");
        fs::create_dir(&trash_destination).expect("trash fixture");
        let (first_moved_tx, first_moved_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let coordinator = Arc::new(FileOperationCoordinator::with_platform_trash(Arc::new(
            PausingTrash {
                destination: trash_destination.clone(),
                first_moved: StdMutex::new(Some(first_moved_tx)),
                release_first: StdMutex::new(release_rx),
            },
        )));
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));

        coordinator
            .start(
                local,
                FileOperationRequestDto {
                    sources: vec![source, changed_source],
                    action: FileOperationActionDto::Trash {},
                },
                channel(events.clone()),
            )
            .expect("start batch Trash");
        first_moved_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("first item moved");
        fs::remove_file(temp.path().join("second.txt")).expect("change second source");
        release_tx.send(()).expect("release first item");
        wait_for_event(&events, "failed").await;

        let terminal = events
            .lock()
            .expect("events")
            .last()
            .expect("terminal")
            .clone();
        assert_eq!(terminal["error"]["code"], "partialCompletion");
        assert_eq!(terminal["outcome"]["status"], "partial");
        assert_eq!(terminal["outcome"]["items"][0]["status"], "completed");
        assert_eq!(terminal["outcome"]["items"][1]["status"], "failed");
        assert_eq!(
            terminal["outcome"]["items"][1]["error"]["code"],
            "sourceChanged"
        );
        assert!(trash_destination.join("notes.md").is_file());
        assert!(!temp.path().join("notes.md").exists());
    }

    #[tokio::test]
    async fn batch_actions_reject_overlapping_source_subtrees_before_mutation() {
        let (temp, local, _) = fixture();
        fs::create_dir(temp.path().join("Folder")).expect("folder source");
        fs::write(temp.path().join("Folder/child.txt"), b"child").expect("child source");
        let folder = listed_entry_ref(&local, "Folder");
        let root = local.locations().expect("locations")[0].root.clone();
        let mut folder_directory = None;
        local
            .list_directory(
                &root.id,
                &root.location_id,
                &AtomicBool::new(false),
                |event| {
                    if let DirectoryListingEvent::Entries { entries, .. } = event {
                        folder_directory = entries
                            .into_iter()
                            .find(|entry| entry.name == "Folder")
                            .and_then(|entry| entry.directory);
                    }
                    Ok(())
                },
            )
            .expect("root listing");
        let folder_directory = folder_directory.expect("folder directory");
        let mut child = None;
        local
            .list_directory(
                &folder_directory.id,
                &folder_directory.location_id,
                &AtomicBool::new(false),
                |event| {
                    if let DirectoryListingEvent::Entries { entries, .. } = event {
                        child = entries
                            .into_iter()
                            .find(|entry| entry.name == "child.txt")
                            .map(|entry| entry.reference);
                    }
                    Ok(())
                },
            )
            .expect("folder listing");
        let trash_destination = temp.path().join("native-trash");
        fs::create_dir(&trash_destination).expect("trash fixture");
        let coordinator = Arc::new(FileOperationCoordinator::with_platform_trash(Arc::new(
            FakeTrash {
                destination: trash_destination,
            },
        )));
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));

        coordinator
            .start(
                local,
                FileOperationRequestDto {
                    sources: vec![folder, child.expect("child reference")],
                    action: FileOperationActionDto::Trash {},
                },
                channel(events.clone()),
            )
            .expect("start overlapping batch");
        wait_for_event(&events, "failed").await;

        let terminal = events
            .lock()
            .expect("events")
            .last()
            .expect("terminal")
            .clone();
        assert_eq!(terminal["error"]["code"], "invalidConfiguration");
        assert!(terminal["outcome"].is_null());
        assert!(temp.path().join("Folder/child.txt").is_file());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_between_batch_items_preserves_unstarted_sources() {
        let (temp, local, first) = fixture();
        fs::write(temp.path().join("second.txt"), b"second").expect("second source");
        let second = listed_entry_ref(&local, "second.txt");
        let trash_destination = temp.path().join("native-trash");
        fs::create_dir(&trash_destination).expect("trash fixture");
        let (first_moved_tx, first_moved_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let coordinator = Arc::new(FileOperationCoordinator::with_platform_trash(Arc::new(
            PausingTrash {
                destination: trash_destination.clone(),
                first_moved: StdMutex::new(Some(first_moved_tx)),
                release_first: StdMutex::new(release_rx),
            },
        )));
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));
        let operation_id = coordinator
            .start(
                local,
                FileOperationRequestDto {
                    sources: vec![first, second],
                    action: FileOperationActionDto::Trash {},
                },
                channel(events.clone()),
            )
            .expect("start cancellable batch");

        first_moved_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("first item moved");
        coordinator.cancel(&operation_id).expect("cancel batch");
        release_tx.send(()).expect("release first item");
        wait_for_event(&events, "cancelled").await;

        let terminal = events
            .lock()
            .expect("events")
            .last()
            .expect("terminal")
            .clone();
        assert_eq!(terminal["completedItems"], 1);
        assert_eq!(terminal["outcome"]["status"], "cancelled");
        assert_eq!(terminal["outcome"]["items"][0]["status"], "completed");
        assert_eq!(terminal["outcome"]["items"][1]["status"], "notStarted");
        assert!(trash_destination.join("notes.md").is_file());
        assert!(temp.path().join("second.txt").is_file());
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
    async fn batch_move_reports_conflict_skips_and_mixed_entry_results() {
        let (temp, local, notes) = fixture();
        fs::create_dir(temp.path().join("Projects")).expect("directory source");
        fs::write(temp.path().join("Projects/readme.md"), b"project").expect("directory child");
        let projects = listed_entry_ref(&local, "Projects");
        let destination = destination_fixture(&temp, &local, "destination");
        fs::write(temp.path().join("destination/notes.md"), b"existing")
            .expect("conflicting destination");
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));

        let operation_id = coordinator
            .start(
                local,
                FileOperationRequestDto {
                    sources: vec![notes.clone(), projects.clone()],
                    action: FileOperationActionDto::Move {
                        destination: destination.clone(),
                    },
                },
                channel(events.clone()),
            )
            .expect("start batch move");
        wait_for_event(&events, "awaitingConflict").await;
        let prompt_id = events
            .lock()
            .expect("events")
            .iter()
            .find(|event| event["event"] == "awaitingConflict")
            .expect("batch conflict")["prompt"]["id"]
            .as_str()
            .expect("prompt id")
            .to_owned();
        coordinator
            .respond(
                &operation_id,
                &prompt_id,
                FileOperationPromptResponseDto::Skip,
            )
            .expect("skip conflict");
        wait_for_event(&events, "completed").await;

        let terminal = events
            .lock()
            .expect("events")
            .last()
            .expect("terminal")
            .clone();
        assert_eq!(terminal["completedItems"], 2);
        assert_eq!(terminal["totalItems"], 2);
        assert_eq!(terminal["outcome"]["kind"], "batch");
        assert_eq!(terminal["outcome"]["status"], "completed");
        assert_eq!(
            terminal["outcome"]["items"][0]["outcome"]["kind"],
            "moveSkipped"
        );
        assert_eq!(terminal["outcome"]["items"][1]["outcome"]["kind"], "moved");
        assert_eq!(
            fs::read(temp.path().join("destination/notes.md")).expect("existing target"),
            b"existing"
        );
        assert!(temp.path().join("notes.md").is_file());
        assert!(temp.path().join("destination/Projects/readme.md").is_file());
        assert!(!temp.path().join("Projects").exists());
    }

    #[tokio::test]
    async fn coordinator_transfers_and_verifies_a_file_between_local_locations() {
        let (temp, local, source, destination) = transfer_fixture();
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));

        coordinator
            .start(
                local,
                FileOperationRequestDto {
                    sources: vec![source.clone()],
                    action: FileOperationActionDto::Move {
                        destination: destination.clone(),
                    },
                },
                channel(events.clone()),
            )
            .expect("start transfer");
        wait_for_event(&events, "completed").await;

        let events = events.lock().expect("events");
        let terminal = events.last().expect("terminal event");
        assert_eq!(terminal["outcome"]["kind"], "moved");
        assert_eq!(
            terminal["outcome"]["entry"]["reference"]["locationId"],
            destination.location_id
        );
        assert!(terminal["outcome"]["invalidatedEntryIds"]
            .as_array()
            .expect("invalidated source identities")
            .iter()
            .any(|id| id == &source.id));
        assert_eq!(terminal["completedBytes"], "600000");
        assert_eq!(terminal["totalBytes"], "600000");
        assert!(events.iter().any(|event| {
            event["event"] == "running"
                && event["completedBytes"] == "0"
                && event["totalBytes"] == "600000"
        }));
        assert!(!temp.path().join("source/notes.md").exists());
        assert_eq!(
            fs::read(temp.path().join("destination/notes.md")).expect("destination bytes"),
            vec![0x33; 600_000]
        );
    }

    #[tokio::test]
    async fn coordinator_transfers_a_local_directory_tree_with_aggregate_byte_progress() {
        let (temp, local, _, destination) = transfer_fixture();
        fs::create_dir_all(temp.path().join("source/bundle/nested")).expect("source tree");
        fs::write(
            temp.path().join("source/bundle/first.bin"),
            vec![0x21; 300_000],
        )
        .expect("first source");
        fs::write(
            temp.path().join("source/bundle/nested/second.bin"),
            vec![0x42; 400_000],
        )
        .expect("second source");
        let source_root = local
            .locations()
            .expect("locations")
            .into_iter()
            .find(|location| location.id == "operation-source")
            .expect("source location")
            .root;
        let mut source = None;
        local
            .list_directory(
                &source_root.id,
                &source_root.location_id,
                &AtomicBool::new(false),
                |event| {
                    if let DirectoryListingEvent::Entries { entries, .. } = event {
                        source = entries
                            .into_iter()
                            .find(|entry| entry.name == "bundle")
                            .map(|entry| entry.reference);
                    }
                    Ok(())
                },
            )
            .expect("source listing");
        let source = source.expect("bundle entry");
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));

        coordinator
            .start(
                local,
                FileOperationRequestDto {
                    sources: vec![source.clone()],
                    action: FileOperationActionDto::Move {
                        destination: destination.clone(),
                    },
                },
                channel(events.clone()),
            )
            .expect("start transfer");
        wait_for_event(&events, "completed").await;

        let events = events.lock().expect("events");
        let terminal = events.last().expect("terminal event");
        assert_eq!(terminal["outcome"]["kind"], "moved");
        assert_eq!(terminal["completedBytes"], "700000");
        assert_eq!(terminal["totalBytes"], "700000");
        assert!(terminal["outcome"]["invalidatedEntryIds"]
            .as_array()
            .expect("invalidated source identities")
            .iter()
            .any(|id| id == &source.id));
        assert!(!temp.path().join("source/bundle").exists());
        assert_eq!(
            fs::read(temp.path().join("destination/bundle/nested/second.bin"))
                .expect("destination bytes"),
            vec![0x42; 400_000]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn coordinator_uploads_verifies_and_removes_a_local_file_over_sftp() {
        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let ssh = Arc::new(SshConnectionManager::default());
        let remote = connect_remote_fixture(&server, ssh.clone()).await;
        let (temp, local, source, _) = transfer_fixture();
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));

        coordinator
            .start_with_backends(
                local,
                ssh.clone(),
                FileOperationRequestDto {
                    sources: vec![source.clone()],
                    action: FileOperationActionDto::Move {
                        destination: remote.root.clone(),
                    },
                },
                channel(events.clone()),
            )
            .expect("start local to SFTP transfer");
        wait_for_event(&events, "completed").await;

        {
            let events = events.lock().expect("events");
            let terminal = events.last().expect("terminal event");
            assert_eq!(terminal["outcome"]["kind"], "moved");
            assert_eq!(
                terminal["outcome"]["entry"]["reference"]["locationId"],
                remote.id
            );
            assert_eq!(terminal["completedBytes"], "600000");
            assert_eq!(terminal["totalBytes"], "600000");
            assert!(terminal["outcome"]["invalidatedEntryIds"]
                .as_array()
                .expect("invalidated local identities")
                .iter()
                .any(|id| id == &source.id));
        }
        assert!(!temp.path().join("source/notes.md").exists());
        assert_eq!(
            server.read_file("/notes.md").await.expect("uploaded file"),
            vec![0x33; 600_000]
        );
        assert!(!remote_root_entries(&ssh, &remote)
            .await
            .iter()
            .any(|entry| entry.name.starts_with(".explora-partial-")));
        ssh.disconnect("operation-test-target")
            .await
            .expect("disconnect");
        server.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn coordinator_moves_a_local_symlink_to_sftp_without_following_it() {
        use std::os::unix::fs::symlink;

        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let ssh = Arc::new(SshConnectionManager::default());
        let remote = connect_remote_fixture(&server, ssh.clone()).await;
        let (temp, local, _, _) = transfer_fixture();
        symlink("target.txt", temp.path().join("source/shortcut"))
            .expect("create local source symlink");
        let source_root = local
            .locations()
            .expect("locations")
            .into_iter()
            .find(|location| location.id == "operation-source")
            .expect("source location")
            .root;
        let mut source = None;
        local
            .list_directory(
                &source_root.id,
                &source_root.location_id,
                &AtomicBool::new(false),
                |event| {
                    if let DirectoryListingEvent::Entries { entries, .. } = event {
                        source = entries
                            .into_iter()
                            .find(|entry| entry.name == "shortcut")
                            .map(|entry| entry.reference);
                    }
                    Ok(())
                },
            )
            .expect("source listing");
        let source = source.expect("symlink source");
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));

        coordinator
            .start_with_backends(
                local,
                ssh.clone(),
                FileOperationRequestDto {
                    sources: vec![source],
                    action: FileOperationActionDto::Move {
                        destination: remote.root.clone(),
                    },
                },
                channel(events.clone()),
            )
            .expect("start local symlink transfer");
        wait_for_event(&events, "completed").await;

        assert_eq!(
            server.read_link("/shortcut").await.as_deref(),
            Some("target.txt")
        );
        assert!(fs::symlink_metadata(temp.path().join("source/shortcut")).is_err());
        assert!(temp.path().join("source/notes.md").is_file());
        assert_eq!(
            events.lock().expect("events").last().expect("terminal")["totalBytes"],
            "0"
        );
        ssh.disconnect("operation-test-target")
            .await
            .expect("disconnect");
        server.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn coordinator_uploads_and_verifies_a_local_directory_tree_over_sftp() {
        use std::os::unix::fs::symlink;

        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let ssh = Arc::new(SshConnectionManager::default());
        let remote = connect_remote_fixture(&server, ssh.clone()).await;
        let (temp, local, _, _) = transfer_fixture();
        fs::create_dir_all(temp.path().join("source/bundle/nested")).expect("nested source");
        fs::create_dir(temp.path().join("source/bundle/empty")).expect("empty source");
        fs::write(temp.path().join("source/bundle/first.bin"), b"first")
            .expect("first source file");
        fs::write(
            temp.path().join("source/bundle/nested/second.bin"),
            b"second!",
        )
        .expect("second source file");
        symlink(
            "nested/second.bin",
            temp.path().join("source/bundle/shortcut"),
        )
        .expect("source tree symlink");
        let source_root = local
            .locations()
            .expect("locations")
            .into_iter()
            .find(|location| location.id == "operation-source")
            .expect("source location")
            .root;
        let mut source = None;
        local
            .list_directory(
                &source_root.id,
                &source_root.location_id,
                &AtomicBool::new(false),
                |event| {
                    if let DirectoryListingEvent::Entries { entries, .. } = event {
                        source = entries
                            .into_iter()
                            .find(|entry| entry.name == "bundle")
                            .map(|entry| entry.reference);
                    }
                    Ok(())
                },
            )
            .expect("source listing");
        let source = source.expect("directory source");
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));

        coordinator
            .start_with_backends(
                local,
                ssh.clone(),
                FileOperationRequestDto {
                    sources: vec![source],
                    action: FileOperationActionDto::Move {
                        destination: remote.root.clone(),
                    },
                },
                channel(events.clone()),
            )
            .expect("start local directory upload");
        wait_for_event(&events, "completed").await;

        assert!(!temp.path().join("source/bundle").exists());
        assert_eq!(
            server.read_file("/bundle/first.bin").await.as_deref(),
            Some(b"first".as_slice())
        );
        assert_eq!(
            server
                .read_file("/bundle/nested/second.bin")
                .await
                .as_deref(),
            Some(b"second!".as_slice())
        );
        assert!(server.path_exists("/bundle/empty").await);
        assert_eq!(
            server.read_link("/bundle/shortcut").await.as_deref(),
            Some("nested/second.bin")
        );
        assert!(temp.path().join("source/notes.md").is_file());
        assert_eq!(
            events.lock().expect("events").last().expect("terminal")["totalBytes"],
            "12"
        );
        assert!(!remote_root_entries(&ssh, &remote)
            .await
            .iter()
            .any(|entry| entry.name.starts_with(".explora-partial-")));
        ssh.disconnect("operation-test-target")
            .await
            .expect("disconnect");
        server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn local_directory_to_sftp_conflict_keeps_both_complete_trees() {
        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        server.create_dir("/bundle").await;
        server
            .write_file("/bundle/existing.txt", b"existing".to_vec())
            .await;
        let ssh = Arc::new(SshConnectionManager::default());
        let remote = connect_remote_fixture(&server, ssh.clone()).await;
        let (temp, local, _, _) = transfer_fixture();
        fs::create_dir(temp.path().join("source/bundle")).expect("source bundle");
        fs::write(temp.path().join("source/bundle/new.txt"), b"new").expect("source file");
        let source_root = local
            .locations()
            .expect("locations")
            .into_iter()
            .find(|location| location.id == "operation-source")
            .expect("source location")
            .root;
        let mut source = None;
        local
            .list_directory(
                &source_root.id,
                &source_root.location_id,
                &AtomicBool::new(false),
                |event| {
                    if let DirectoryListingEvent::Entries { entries, .. } = event {
                        source = entries
                            .into_iter()
                            .find(|entry| entry.name == "bundle")
                            .map(|entry| entry.reference);
                    }
                    Ok(())
                },
            )
            .expect("source listing");
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));
        let operation_id = coordinator
            .start_with_backends(
                local,
                ssh.clone(),
                FileOperationRequestDto {
                    sources: vec![source.expect("directory source")],
                    action: FileOperationActionDto::Move {
                        destination: remote.root.clone(),
                    },
                },
                channel(events.clone()),
            )
            .expect("start conflicting directory transfer");
        wait_for_event(&events, "awaitingConflict").await;
        let prompt_id = events
            .lock()
            .expect("events")
            .iter()
            .find(|event| event["event"] == "awaitingConflict")
            .expect("conflict event")["prompt"]["id"]
            .as_str()
            .expect("prompt id")
            .to_owned();
        coordinator
            .respond(
                &operation_id,
                &prompt_id,
                FileOperationPromptResponseDto::KeepBoth,
            )
            .expect("keep both");
        wait_for_event(&events, "completed").await;

        assert_eq!(
            server.read_file("/bundle/existing.txt").await.as_deref(),
            Some(b"existing".as_slice())
        );
        assert_eq!(
            server.read_file("/bundle copy/new.txt").await.as_deref(),
            Some(b"new".as_slice())
        );
        assert!(!temp.path().join("source/bundle").exists());
        ssh.disconnect("operation-test-target")
            .await
            .expect("disconnect");
        server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_remote_tree_write_cleans_partial_and_preserves_local_source() {
        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let ssh = Arc::new(SshConnectionManager::default());
        let remote = connect_remote_fixture(&server, ssh.clone()).await;
        let (temp, local, _, _) = transfer_fixture();
        fs::create_dir(temp.path().join("source/bundle")).expect("source bundle");
        fs::write(
            temp.path().join("source/bundle/locked.txt"),
            b"cannot write",
        )
        .expect("locked source file");
        let source_root = local
            .locations()
            .expect("locations")
            .into_iter()
            .find(|location| location.id == "operation-source")
            .expect("source location")
            .root;
        let mut source = None;
        local
            .list_directory(
                &source_root.id,
                &source_root.location_id,
                &AtomicBool::new(false),
                |event| {
                    if let DirectoryListingEvent::Entries { entries, .. } = event {
                        source = entries
                            .into_iter()
                            .find(|entry| entry.name == "bundle")
                            .map(|entry| entry.reference);
                    }
                    Ok(())
                },
            )
            .expect("source listing");
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));
        coordinator
            .start_with_backends(
                local,
                ssh.clone(),
                FileOperationRequestDto {
                    sources: vec![source.expect("directory source")],
                    action: FileOperationActionDto::Move {
                        destination: remote.root.clone(),
                    },
                },
                channel(events.clone()),
            )
            .expect("start failing directory transfer");
        wait_for_event(&events, "failed").await;

        assert_eq!(
            events.lock().expect("events").last().expect("terminal")["error"]["code"],
            "permissionDenied"
        );
        assert!(temp.path().join("source/bundle/locked.txt").is_file());
        assert!(!server.path_exists("/bundle").await);
        assert!(!remote_root_entries(&ssh, &remote)
            .await
            .iter()
            .any(|entry| entry.name.starts_with(".explora-partial-")));
        ssh.disconnect("operation-test-target")
            .await
            .expect("disconnect");
        server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_remote_tree_creation_cleans_partial_and_preserves_local_source() {
        let server = TestSshServer::start_with_delays(
            TestAuthMode::PublicKey,
            true,
            Duration::ZERO,
            Duration::from_millis(100),
        )
        .await;
        let ssh = Arc::new(SshConnectionManager::default());
        let remote = connect_remote_fixture(&server, ssh.clone()).await;
        let (temp, local, _, _) = transfer_fixture();
        fs::create_dir(temp.path().join("source/bundle")).expect("source bundle");
        fs::write(
            temp.path().join("source/bundle/data.bin"),
            vec![0x41; 600_000],
        )
        .expect("source file");
        let source_root = local
            .locations()
            .expect("locations")
            .into_iter()
            .find(|location| location.id == "operation-source")
            .expect("source location")
            .root;
        let mut source = None;
        local
            .list_directory(
                &source_root.id,
                &source_root.location_id,
                &AtomicBool::new(false),
                |event| {
                    if let DirectoryListingEvent::Entries { entries, .. } = event {
                        source = entries
                            .into_iter()
                            .find(|entry| entry.name == "bundle")
                            .map(|entry| entry.reference);
                    }
                    Ok(())
                },
            )
            .expect("source listing");
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));
        let operation_id = coordinator
            .start_with_backends(
                local,
                ssh.clone(),
                FileOperationRequestDto {
                    sources: vec![source.expect("directory source")],
                    action: FileOperationActionDto::Move {
                        destination: remote.root.clone(),
                    },
                },
                channel(events.clone()),
            )
            .expect("start cancellable directory transfer");
        server.wait_for_mutation().await;
        coordinator.cancel(&operation_id).expect("cancel transfer");
        wait_for_event(&events, "cancelled").await;

        assert!(temp.path().join("source/bundle/data.bin").is_file());
        assert!(!server.path_exists("/bundle").await);
        assert!(!remote_root_entries(&ssh, &remote)
            .await
            .iter()
            .any(|entry| entry.name.starts_with(".explora-partial-")));
        ssh.disconnect("operation-test-target")
            .await
            .expect("disconnect");
        server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn local_to_sftp_conflict_preserves_both_until_keep_both_is_selected() {
        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let ssh = Arc::new(SshConnectionManager::default());
        let remote = connect_remote_fixture(&server, ssh.clone()).await;
        let (temp, local, _) = fixture();
        fs::write(temp.path().join("README.md"), b"uploaded readme").expect("local source");
        let source = listed_entry_ref(&local, "README.md");
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));
        let operation_id = coordinator
            .start_with_backends(
                local,
                ssh.clone(),
                FileOperationRequestDto {
                    sources: vec![source],
                    action: FileOperationActionDto::Move {
                        destination: remote.root.clone(),
                    },
                },
                channel(events.clone()),
            )
            .expect("start conflicting transfer");
        wait_for_event(&events, "awaitingConflict").await;

        assert!(temp.path().join("README.md").exists());
        assert_eq!(
            server
                .read_file("/README.md")
                .await
                .expect("existing remote"),
            vec![0; 128]
        );
        let prompt_id = events
            .lock()
            .expect("events")
            .iter()
            .find(|event| event["event"] == "awaitingConflict")
            .expect("conflict event")["prompt"]["id"]
            .as_str()
            .expect("prompt id")
            .to_owned();
        coordinator
            .respond(
                &operation_id,
                &prompt_id,
                FileOperationPromptResponseDto::KeepBoth,
            )
            .expect("keep both response");
        wait_for_event(&events, "completed").await;

        assert!(!temp.path().join("README.md").exists());
        assert_eq!(
            server
                .read_file("/README copy.md")
                .await
                .expect("kept-both upload"),
            b"uploaded readme"
        );
        assert_eq!(
            server
                .read_file("/README.md")
                .await
                .expect("existing remote"),
            vec![0; 128]
        );
        assert!(!remote_root_entries(&ssh, &remote)
            .await
            .iter()
            .any(|entry| entry.name.starts_with(".explora-partial-")));
        ssh.disconnect("operation-test-target")
            .await
            .expect("disconnect");
        server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_local_to_sftp_copy_removes_the_partial_and_preserves_the_source() {
        let server = TestSshServer::start_with_delays(
            TestAuthMode::PublicKey,
            true,
            Duration::ZERO,
            Duration::from_millis(100),
        )
        .await;
        let ssh = Arc::new(SshConnectionManager::default());
        let remote = connect_remote_fixture(&server, ssh.clone()).await;
        let (temp, local, source, _) = transfer_fixture();
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));
        let operation_id = coordinator
            .start_with_backends(
                local,
                ssh.clone(),
                FileOperationRequestDto {
                    sources: vec![source],
                    action: FileOperationActionDto::Move {
                        destination: remote.root.clone(),
                    },
                },
                channel(events.clone()),
            )
            .expect("start cancellable transfer");
        server.wait_for_mutation().await;
        coordinator.cancel(&operation_id).expect("cancel transfer");
        wait_for_event(&events, "cancelled").await;

        assert!(temp.path().join("source/notes.md").exists());
        assert!(!server.path_exists("/notes.md").await);
        assert!(!remote_root_entries(&ssh, &remote)
            .await
            .iter()
            .any(|entry| entry.name.starts_with(".explora-partial-")));
        ssh.disconnect("operation-test-target")
            .await
            .expect("disconnect");
        server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn local_source_change_during_sftp_copy_removes_the_partial_and_reports_source_changed() {
        let server = TestSshServer::start_with_delays(
            TestAuthMode::PublicKey,
            true,
            Duration::ZERO,
            Duration::from_millis(100),
        )
        .await;
        let ssh = Arc::new(SshConnectionManager::default());
        let remote = connect_remote_fixture(&server, ssh.clone()).await;
        let (temp, local, source, _) = transfer_fixture();
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));
        coordinator
            .start_with_backends(
                local,
                ssh.clone(),
                FileOperationRequestDto {
                    sources: vec![source],
                    action: FileOperationActionDto::Move {
                        destination: remote.root.clone(),
                    },
                },
                channel(events.clone()),
            )
            .expect("start mutable transfer");
        server.wait_for_mutation().await;
        fs::write(temp.path().join("source/notes.md"), b"changed").expect("mutate local source");
        wait_for_event(&events, "failed").await;

        let terminal = events
            .lock()
            .expect("events")
            .last()
            .expect("terminal event")
            .clone();
        assert_eq!(terminal["error"]["code"], "sourceChanged");
        assert_eq!(
            fs::read(temp.path().join("source/notes.md")).expect("changed source"),
            b"changed"
        );
        assert!(!server.path_exists("/notes.md").await);
        assert!(!remote_root_entries(&ssh, &remote)
            .await
            .iter()
            .any(|entry| entry.name.starts_with(".explora-partial-")));
        ssh.disconnect("operation-test-target")
            .await
            .expect("disconnect");
        server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn coordinator_streams_and_verifies_a_file_between_two_sftp_locations() {
        let source_server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let destination_server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let payload = vec![0x58; 600_000];
        source_server
            .write_file("/transfer.bin", payload.clone())
            .await;
        let ssh = Arc::new(SshConnectionManager::default());
        let source_location =
            connect_remote_fixture_as(&source_server, ssh.clone(), "operation-source-target").await;
        let destination_location = connect_remote_fixture_as(
            &destination_server,
            ssh.clone(),
            "operation-destination-target",
        )
        .await;
        let source = remote_root_entries(&ssh, &source_location)
            .await
            .into_iter()
            .find(|entry| entry.name == "transfer.bin")
            .expect("remote transfer source")
            .reference;
        let (_temp, local, _) = fixture();
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));

        coordinator
            .start_with_backends(
                local,
                ssh.clone(),
                FileOperationRequestDto {
                    sources: vec![source.clone()],
                    action: FileOperationActionDto::Move {
                        destination: destination_location.root.clone(),
                    },
                },
                channel(events.clone()),
            )
            .expect("start SFTP to SFTP transfer");
        wait_for_event(&events, "completed").await;

        {
            let events = events.lock().expect("events");
            let terminal = events.last().expect("terminal event");
            assert_eq!(terminal["outcome"]["kind"], "moved");
            assert_eq!(
                terminal["outcome"]["entry"]["reference"]["locationId"],
                destination_location.id
            );
            assert_eq!(terminal["completedBytes"], "600000");
            assert_eq!(terminal["totalBytes"], "600000");
            assert!(terminal["outcome"]["invalidatedEntryIds"]
                .as_array()
                .expect("invalidated remote identities")
                .iter()
                .any(|id| id == &source.id));
        }
        assert!(!source_server.path_exists("/transfer.bin").await);
        assert_eq!(
            destination_server
                .read_file("/transfer.bin")
                .await
                .expect("destination payload"),
            payload
        );
        assert!(!remote_root_entries(&ssh, &destination_location)
            .await
            .iter()
            .any(|entry| entry.name.starts_with(".explora-partial-")));
        ssh.disconnect("operation-source-target")
            .await
            .expect("disconnect source");
        ssh.disconnect("operation-destination-target")
            .await
            .expect("disconnect destination");
        source_server.shutdown().await;
        destination_server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn coordinator_moves_a_symlink_between_sftp_locations_without_following_it() {
        let source_server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let destination_server = TestSshServer::start(TestAuthMode::PublicKey).await;
        source_server
            .create_symlink("/move-link", "../shared/target")
            .await;
        let ssh = Arc::new(SshConnectionManager::default());
        let source_location =
            connect_remote_fixture_as(&source_server, ssh.clone(), "symlink-source-target").await;
        let destination_location = connect_remote_fixture_as(
            &destination_server,
            ssh.clone(),
            "symlink-destination-target",
        )
        .await;
        let source = remote_root_entries(&ssh, &source_location)
            .await
            .into_iter()
            .find(|entry| entry.name == "move-link")
            .expect("remote symlink source")
            .reference;
        let (_temp, local, _) = fixture();
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));

        coordinator
            .start_with_backends(
                local,
                ssh.clone(),
                FileOperationRequestDto {
                    sources: vec![source],
                    action: FileOperationActionDto::Move {
                        destination: destination_location.root.clone(),
                    },
                },
                channel(events.clone()),
            )
            .expect("start remote symlink transfer");
        wait_for_event(&events, "completed").await;

        assert!(!source_server.path_exists("/move-link").await);
        assert_eq!(
            destination_server.read_link("/move-link").await.as_deref(),
            Some("../shared/target")
        );
        assert!(source_server.path_exists("/projects/notes.txt").await);
        ssh.disconnect("symlink-source-target")
            .await
            .expect("disconnect source");
        ssh.disconnect("symlink-destination-target")
            .await
            .expect("disconnect destination");
        source_server.shutdown().await;
        destination_server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn coordinator_streams_and_verifies_a_directory_between_sftp_locations() {
        let source_server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let destination_server = TestSshServer::start(TestAuthMode::PublicKey).await;
        source_server.create_dir("/bundle").await;
        source_server.create_dir("/bundle/nested").await;
        source_server.create_dir("/bundle/empty").await;
        source_server
            .write_file("/bundle/first.bin", vec![0x31; 300_000])
            .await;
        source_server
            .write_file("/bundle/nested/second.bin", vec![0x52; 400_000])
            .await;
        source_server
            .create_symlink("/bundle/shortcut", "nested/second.bin")
            .await;
        let ssh = Arc::new(SshConnectionManager::default());
        let source_location =
            connect_remote_fixture_as(&source_server, ssh.clone(), "tree-source-target").await;
        let destination_location =
            connect_remote_fixture_as(&destination_server, ssh.clone(), "tree-destination-target")
                .await;
        let source = remote_root_entries(&ssh, &source_location)
            .await
            .into_iter()
            .find(|entry| entry.name == "bundle")
            .expect("remote directory source")
            .reference;
        let (_temp, local, _) = fixture();
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));

        coordinator
            .start_with_backends(
                local,
                ssh.clone(),
                FileOperationRequestDto {
                    sources: vec![source],
                    action: FileOperationActionDto::Move {
                        destination: destination_location.root.clone(),
                    },
                },
                channel(events.clone()),
            )
            .expect("start remote directory transfer");
        wait_for_event(&events, "completed").await;

        assert!(!source_server.path_exists("/bundle").await);
        assert_eq!(
            destination_server
                .read_file("/bundle/first.bin")
                .await
                .expect("first destination file"),
            vec![0x31; 300_000]
        );
        assert_eq!(
            destination_server
                .read_file("/bundle/nested/second.bin")
                .await
                .expect("second destination file"),
            vec![0x52; 400_000]
        );
        assert!(destination_server.path_exists("/bundle/empty").await);
        assert_eq!(
            destination_server
                .read_link("/bundle/shortcut")
                .await
                .as_deref(),
            Some("nested/second.bin")
        );
        assert_eq!(
            events.lock().expect("events").last().expect("terminal")["totalBytes"],
            "700000"
        );
        assert!(!remote_root_entries(&ssh, &destination_location)
            .await
            .iter()
            .any(|entry| entry.name.starts_with(".explora-partial-")));
        ssh.disconnect("tree-source-target")
            .await
            .expect("disconnect source");
        ssh.disconnect("tree-destination-target")
            .await
            .expect("disconnect destination");
        source_server.shutdown().await;
        destination_server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn changed_remote_directory_source_cleans_destination_partial() {
        let source_server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let destination_server = TestSshServer::start_with_delays(
            TestAuthMode::PublicKey,
            true,
            Duration::ZERO,
            Duration::from_millis(100),
        )
        .await;
        source_server.create_dir("/bundle").await;
        source_server
            .write_file("/bundle/data.bin", vec![0x55; 600_000])
            .await;
        let ssh = Arc::new(SshConnectionManager::default());
        let source_location =
            connect_remote_fixture_as(&source_server, ssh.clone(), "changed-tree-source").await;
        let destination_location =
            connect_remote_fixture_as(&destination_server, ssh.clone(), "changed-tree-destination")
                .await;
        let source = remote_root_entries(&ssh, &source_location)
            .await
            .into_iter()
            .find(|entry| entry.name == "bundle")
            .expect("remote directory source")
            .reference;
        let (_temp, local, _) = fixture();
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));
        coordinator
            .start_with_backends(
                local,
                ssh.clone(),
                FileOperationRequestDto {
                    sources: vec![source],
                    action: FileOperationActionDto::Move {
                        destination: destination_location.root.clone(),
                    },
                },
                channel(events.clone()),
            )
            .expect("start mutable remote directory transfer");
        destination_server.wait_for_mutation().await;
        source_server
            .write_file("/bundle/late.bin", b"late".to_vec())
            .await;
        wait_for_event(&events, "failed").await;

        assert_eq!(
            events.lock().expect("events").last().expect("terminal")["error"]["code"],
            "sourceChanged"
        );
        assert!(source_server.path_exists("/bundle/data.bin").await);
        assert!(source_server.path_exists("/bundle/late.bin").await);
        assert!(!destination_server.path_exists("/bundle").await);
        assert!(!remote_root_entries(&ssh, &destination_location)
            .await
            .iter()
            .any(|entry| entry.name.starts_with(".explora-partial-")));
        ssh.disconnect("changed-tree-source")
            .await
            .expect("disconnect source");
        ssh.disconnect("changed-tree-destination")
            .await
            .expect("disconnect destination");
        source_server.shutdown().await;
        destination_server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn coordinator_downloads_verifies_and_removes_an_sftp_file_to_local_storage() {
        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let payload = vec![0x71; 600_000];
        server.write_file("/download.bin", payload.clone()).await;
        let ssh = Arc::new(SshConnectionManager::default());
        let remote = connect_remote_fixture(&server, ssh.clone()).await;
        let source = remote_root_entries(&ssh, &remote)
            .await
            .into_iter()
            .find(|entry| entry.name == "download.bin")
            .expect("remote download source")
            .reference;
        let (temp, local, _) = fixture();
        let destination = destination_fixture(&temp, &local, "downloads");
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));

        coordinator
            .start_with_backends(
                local,
                ssh.clone(),
                FileOperationRequestDto {
                    sources: vec![source.clone()],
                    action: FileOperationActionDto::Move {
                        destination: destination.clone(),
                    },
                },
                channel(events.clone()),
            )
            .expect("start SFTP to local transfer");
        wait_for_event(&events, "completed").await;

        {
            let events = events.lock().expect("events");
            let terminal = events.last().expect("terminal event");
            assert_eq!(terminal["outcome"]["kind"], "moved");
            assert_eq!(
                terminal["outcome"]["entry"]["reference"]["locationId"],
                destination.location_id
            );
            assert_eq!(terminal["completedBytes"], "600000");
            assert_eq!(terminal["totalBytes"], "600000");
            assert!(terminal["outcome"]["invalidatedEntryIds"]
                .as_array()
                .expect("invalidated remote identities")
                .iter()
                .any(|id| id == &source.id));
        }
        assert!(!server.path_exists("/download.bin").await);
        assert_eq!(
            fs::read(temp.path().join("downloads/download.bin")).expect("downloaded payload"),
            payload
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::symlink_metadata(temp.path().join("downloads/download.bin"))
                    .expect("download metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o644
            );
        }
        assert!(fs::read_dir(temp.path().join("downloads"))
            .expect("download listing")
            .all(|entry| !entry
                .expect("download entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".explora-partial-")));
        ssh.disconnect("operation-test-target")
            .await
            .expect("disconnect");
        server.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn coordinator_moves_an_sftp_symlink_to_local_storage_without_following_it() {
        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        server
            .create_symlink("/download-link", "/projects/notes.txt")
            .await;
        let ssh = Arc::new(SshConnectionManager::default());
        let remote = connect_remote_fixture(&server, ssh.clone()).await;
        let source = remote_root_entries(&ssh, &remote)
            .await
            .into_iter()
            .find(|entry| entry.name == "download-link")
            .expect("remote symlink source")
            .reference;
        let (temp, local, _) = fixture();
        let destination = destination_fixture(&temp, &local, "downloads");
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));

        coordinator
            .start_with_backends(
                local,
                ssh.clone(),
                FileOperationRequestDto {
                    sources: vec![source],
                    action: FileOperationActionDto::Move { destination },
                },
                channel(events.clone()),
            )
            .expect("start remote symlink download");
        wait_for_event(&events, "completed").await;

        assert!(!server.path_exists("/download-link").await);
        assert_eq!(
            fs::read_link(temp.path().join("downloads/download-link"))
                .expect("local destination symlink"),
            PathBuf::from("/projects/notes.txt")
        );
        assert!(server.path_exists("/projects/notes.txt").await);
        ssh.disconnect("operation-test-target")
            .await
            .expect("disconnect");
        server.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn coordinator_downloads_and_verifies_an_sftp_directory_tree() {
        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        server.create_dir("/download-tree").await;
        server.create_dir("/download-tree/nested").await;
        server.create_dir("/download-tree/empty").await;
        server
            .write_file("/download-tree/first.bin", vec![0x17; 300_000])
            .await;
        server
            .write_file("/download-tree/nested/second.bin", vec![0x29; 400_000])
            .await;
        server
            .create_symlink("/download-tree/shortcut", "nested/second.bin")
            .await;
        let ssh = Arc::new(SshConnectionManager::default());
        let remote = connect_remote_fixture(&server, ssh.clone()).await;
        let source = remote_root_entries(&ssh, &remote)
            .await
            .into_iter()
            .find(|entry| entry.name == "download-tree")
            .expect("remote directory source")
            .reference;
        let (temp, local, _) = fixture();
        let destination = destination_fixture(&temp, &local, "downloads");
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));

        coordinator
            .start_with_backends(
                local,
                ssh.clone(),
                FileOperationRequestDto {
                    sources: vec![source],
                    action: FileOperationActionDto::Move {
                        destination: destination.clone(),
                    },
                },
                channel(events.clone()),
            )
            .expect("start remote directory download");
        wait_for_event(&events, "completed").await;

        assert!(!server.path_exists("/download-tree").await);
        assert_eq!(
            fs::read(temp.path().join("downloads/download-tree/first.bin"))
                .expect("first local file"),
            vec![0x17; 300_000]
        );
        assert_eq!(
            fs::read(
                temp.path()
                    .join("downloads/download-tree/nested/second.bin")
            )
            .expect("second local file"),
            vec![0x29; 400_000]
        );
        assert!(temp.path().join("downloads/download-tree/empty").is_dir());
        assert_eq!(
            fs::read_link(temp.path().join("downloads/download-tree/shortcut"))
                .expect("local tree symlink"),
            PathBuf::from("nested/second.bin")
        );
        assert_eq!(
            events.lock().expect("events").last().expect("terminal")["totalBytes"],
            "700000"
        );
        assert!(fs::read_dir(temp.path().join("downloads"))
            .expect("downloads")
            .all(|entry| !entry
                .expect("download entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".explora-partial-")));
        ssh.disconnect("operation-test-target")
            .await
            .expect("disconnect");
        server.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_remote_source_removal_keeps_the_verified_local_destination() {
        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        let ssh = Arc::new(SshConnectionManager::default());
        let remote = connect_remote_fixture(&server, ssh.clone()).await;
        let source = remote_root_entries(&ssh, &remote)
            .await
            .into_iter()
            .find(|entry| entry.name == "locked.txt")
            .expect("locked remote source")
            .reference;
        let (temp, local, _) = fixture();
        let destination = destination_fixture(&temp, &local, "downloads");
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));

        coordinator
            .start_with_backends(
                local,
                ssh.clone(),
                FileOperationRequestDto {
                    sources: vec![source],
                    action: FileOperationActionDto::Move { destination },
                },
                channel(events.clone()),
            )
            .expect("start partial transfer");
        wait_for_event(&events, "failed").await;

        let terminal = events
            .lock()
            .expect("events")
            .last()
            .expect("terminal event")
            .clone();
        assert_eq!(terminal["error"]["code"], "partialCompletion");
        assert!(server.path_exists("/locked.txt").await);
        assert_eq!(
            fs::read(temp.path().join("downloads/locked.txt")).expect("verified destination"),
            vec![0; 8]
        );
        ssh.disconnect("operation-test-target")
            .await
            .expect("disconnect");
        server.shutdown().await;
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn coordinator_batch_deletes_mixed_remote_entries_after_one_confirmation() {
        let server = TestSshServer::start(TestAuthMode::PublicKey).await;
        server.create_symlink("/readme-link", "/README.md").await;
        let ssh = Arc::new(SshConnectionManager::default());
        let location = connect_remote_fixture(&server, ssh.clone()).await;
        let remote_entries = remote_root_entries(&ssh, &location).await;
        let readme = remote_entries
            .iter()
            .find(|entry| entry.name == "README.md")
            .expect("remote file")
            .reference
            .clone();
        let link = remote_entries
            .iter()
            .find(|entry| entry.name == "readme-link")
            .expect("remote link")
            .reference
            .clone();
        let (_temp, local, _) = fixture();
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));
        let operation_id = coordinator
            .start_with_backends(
                local,
                ssh.clone(),
                FileOperationRequestDto {
                    sources: vec![readme, link],
                    action: FileOperationActionDto::DeletePermanently {},
                },
                channel(events.clone()),
            )
            .expect("start remote batch delete");
        wait_for_event(&events, "awaitingConfirmation").await;
        let prompt = events
            .lock()
            .expect("events")
            .iter()
            .find(|event| event["event"] == "awaitingConfirmation")
            .expect("confirmation")["prompt"]
            .clone();
        assert_eq!(prompt["targetName"], "2 selected items");
        coordinator
            .respond(
                &operation_id,
                prompt["id"].as_str().expect("prompt id"),
                FileOperationPromptResponseDto::Confirm,
            )
            .expect("confirm remote batch delete");
        wait_for_event(&events, "completed").await;

        let terminal = events
            .lock()
            .expect("events")
            .last()
            .expect("terminal")
            .clone();
        assert_eq!(terminal["outcome"]["status"], "completed");
        assert_eq!(terminal["outcome"]["items"][0]["status"], "completed");
        assert_eq!(terminal["outcome"]["items"][1]["status"], "completed");
        assert!(!server.path_exists("/README.md").await);
        assert!(!server.path_exists("/readme-link").await);
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
    async fn batch_permanent_delete_uses_one_authoritative_confirmation() {
        let (temp, local, first) = fixture();
        fs::write(temp.path().join("second.txt"), b"second").expect("second source");
        let second = listed_entry_ref(&local, "second.txt");
        let coordinator = Arc::new(FileOperationCoordinator::default());
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));
        let operation_id = coordinator
            .start(
                local,
                FileOperationRequestDto {
                    sources: vec![first, second],
                    action: FileOperationActionDto::DeletePermanently {},
                },
                channel(events.clone()),
            )
            .expect("start batch delete");
        wait_for_event(&events, "awaitingConfirmation").await;

        let captured = events.lock().expect("events").clone();
        let prompts = captured
            .iter()
            .filter(|event| event["event"] == "awaitingConfirmation")
            .collect::<Vec<_>>();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0]["prompt"]["targetName"], "2 selected items");
        let prompt_id = prompts[0]["prompt"]["id"]
            .as_str()
            .expect("prompt id")
            .to_owned();
        coordinator
            .respond(
                &operation_id,
                &prompt_id,
                FileOperationPromptResponseDto::Confirm,
            )
            .expect("confirm batch delete");
        wait_for_event(&events, "completed").await;

        let captured = events.lock().expect("events");
        assert_eq!(
            captured
                .iter()
                .filter(|event| event["event"] == "awaitingConfirmation")
                .count(),
            1
        );
        assert_eq!(
            captured.last().expect("terminal")["outcome"]["status"],
            "completed"
        );
        assert!(!temp.path().join("notes.md").exists());
        assert!(!temp.path().join("second.txt").exists());
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
