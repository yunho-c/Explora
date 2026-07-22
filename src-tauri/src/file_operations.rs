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
        EntryRefDto, ExplorerError, ExplorerErrorDto, FileEntrySummaryDto, PROMPT_TIMEOUT,
    },
    local_filesystem::{LocalFilesystem, RemovedLocalEntry},
    platform_trash::{PlatformTrash, SystemPlatformTrash},
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
    Trash {},
    DeletePermanently {},
}

impl FileOperationActionDto {
    fn kind(&self) -> FileOperationKindDto {
        match self {
            Self::Rename { .. } => FileOperationKindDto::Rename,
            Self::Trash {} => FileOperationKindDto::Trash,
            Self::DeletePermanently {} => FileOperationKindDto::DeletePermanently,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FileOperationKindDto {
    Rename,
    Trash,
    DeletePermanently,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FileOperationPromptResponseDto {
    Confirm,
    Cancel,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileOperationPromptDto {
    pub id: String,
    pub kind: FileOperationPromptKindDto,
    pub title: String,
    pub message: String,
    pub target_name: String,
    pub location_name: String,
    pub confirm_label: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FileOperationPromptKindDto {
    PermanentDelete,
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

    fn begin_confirmation(
        &self,
        prompt_id: String,
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
                response: sender,
            });
        }
        Ok(receiver)
    }

    fn await_confirmation(
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
                completed_items: 0,
                total_items: 1,
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
                completed_items: 0,
                total_items: 1,
                prompt,
            })
            .map_err(|_| ExplorerError::ChannelClosed)
    }

    fn terminal(&mut self, result: Result<FileOperationOutcomeDto, ExplorerError>) {
        let sequence = self.next_sequence();
        let event = match result {
            Ok(outcome) => FileOperationEventDto::Completed {
                operation_id: self.operation_id.clone(),
                sequence,
                action: self.action,
                completed_items: 1,
                total_items: 1,
                outcome,
            },
            Err(ExplorerError::Cancelled) => FileOperationEventDto::Cancelled {
                operation_id: self.operation_id.clone(),
                sequence,
                action: self.action,
                completed_items: 0,
                total_items: 1,
            },
            Err(error) => FileOperationEventDto::Failed {
                operation_id: self.operation_id.clone(),
                sequence,
                action: self.action,
                completed_items: 0,
                total_items: 1,
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

    pub fn start(
        self: &Arc<Self>,
        local: Arc<LocalFilesystem>,
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
        tauri::async_runtime::spawn_blocking(move || {
            let mut events = OperationEventEmitter {
                operation_id: task_operation_id.clone(),
                action,
                sequence: 1,
                channel: on_event,
            };
            let result = coordinator.run_local(&local, &request, &active, &mut events);
            active.clear_prompt();
            events.terminal(result);
            coordinator.finish(&task_operation_id);
        });

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
        let _guard = self
            .execution_guard
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        active.ensure_not_cancelled()?;
        events.running()?;

        let source = &request.sources[0];
        match &request.action {
            FileOperationActionDto::Rename { new_name } => local
                .rename_entry(source, new_name, &active.cancelled)
                .map(|entry| FileOperationOutcomeDto::Renamed {
                    entry: Box::new(entry),
                }),
            FileOperationActionDto::Trash {} => local
                .trash_entry(source, &active.cancelled, self.platform_trash.as_ref())
                .map(trashed_outcome),
            FileOperationActionDto::DeletePermanently {} => {
                let (target_name, location_name) = local.describe_operation_target(source)?;
                let prompt_id = Uuid::new_v4().to_string();
                let response = active.begin_confirmation(prompt_id.clone())?;
                events.awaiting_confirmation(FileOperationPromptDto {
                    id: prompt_id.clone(),
                    kind: FileOperationPromptKindDto::PermanentDelete,
                    title: format!("Delete “{target_name}” permanently?"),
                    message:
                        "This item will be removed immediately and cannot be recovered from Trash."
                            .to_owned(),
                    target_name,
                    location_name,
                    confirm_label: "Delete Permanently",
                })?;
                match active.await_confirmation(response)? {
                    FileOperationPromptResponseDto::Confirm => {}
                    FileOperationPromptResponseDto::Cancel => return Err(ExplorerError::Cancelled),
                }
                active.ensure_not_cancelled()?;
                local
                    .permanently_delete_entry(source, &active.cancelled)
                    .map(deleted_outcome)
            }
        }
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
    if source.location_id.starts_with("ssh:") {
        return Err(ExplorerError::Unsupported(
            "Remote filesystem actions are not available yet.".to_owned(),
        ));
    }
    Ok(())
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
        filesystem::{DirectoryListingEvent, LocationRole},
        local_filesystem::LocalRoot,
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
