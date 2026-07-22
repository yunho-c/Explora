use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use uuid::Uuid;

use crate::{
    filesystem::{EntryRefDto, ExplorerError, ExplorerErrorDto, FileEntrySummaryDto},
    local_filesystem::LocalFilesystem,
};

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
    },
    Running {
        operation_id: String,
        sequence: u64,
    },
    Completed {
        operation_id: String,
        sequence: u64,
        entry: Box<FileEntrySummaryDto>,
    },
    Cancelled {
        operation_id: String,
        sequence: u64,
    },
    Failed {
        operation_id: String,
        sequence: u64,
        error: ExplorerErrorDto,
    },
}

#[derive(Default)]
pub struct FileOperationCoordinator {
    active: Mutex<HashMap<String, Arc<AtomicBool>>>,
    // The first slice serializes mutations. Later phases can replace this with
    // subtree-aware guards without changing the operation or IPC contracts.
    execution_guard: Mutex<()>,
}

impl FileOperationCoordinator {
    pub fn start(
        self: &Arc<Self>,
        local: Arc<LocalFilesystem>,
        request: FileOperationRequestDto,
        on_event: Channel<FileOperationEventDto>,
    ) -> Result<String, ExplorerError> {
        if request.sources.len() != 1 {
            return Err(ExplorerError::InvalidConfiguration(
                "Filesystem actions currently require exactly one selected item.".to_owned(),
            ));
        }
        if request.sources[0].location_id.starts_with("ssh:") {
            return Err(ExplorerError::Unsupported(
                "Remote filesystem actions are not available yet.".to_owned(),
            ));
        }

        let operation_id = Uuid::new_v4().to_string();
        let cancelled = Arc::new(AtomicBool::new(false));
        self.active
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .insert(operation_id.clone(), cancelled.clone());
        if on_event
            .send(FileOperationEventDto::Queued {
                operation_id: operation_id.clone(),
                sequence: 0,
            })
            .is_err()
        {
            self.finish(&operation_id);
            return Err(ExplorerError::ChannelClosed);
        }

        let coordinator = self.clone();
        let task_operation_id = operation_id.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let result =
                coordinator.run_local(&local, &request, &task_operation_id, &cancelled, &on_event);
            if let Err(error) = result {
                let event = if matches!(error, ExplorerError::Cancelled) {
                    FileOperationEventDto::Cancelled {
                        operation_id: task_operation_id.clone(),
                        sequence: 2,
                    }
                } else {
                    FileOperationEventDto::Failed {
                        operation_id: task_operation_id.clone(),
                        sequence: 2,
                        error: ExplorerErrorDto::from(error),
                    }
                };
                let _ = on_event.send(event);
            }
            coordinator.finish(&task_operation_id);
        });

        Ok(operation_id)
    }

    pub fn cancel(&self, operation_id: &str) -> Result<(), ExplorerError> {
        if let Some(cancelled) = self
            .active
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .get(operation_id)
        {
            cancelled.store(true, Ordering::Relaxed);
        }
        Ok(())
    }

    fn run_local(
        &self,
        local: &LocalFilesystem,
        request: &FileOperationRequestDto,
        operation_id: &str,
        cancelled: &AtomicBool,
        on_event: &Channel<FileOperationEventDto>,
    ) -> Result<(), ExplorerError> {
        let _guard = self
            .execution_guard
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        if cancelled.load(Ordering::Relaxed) {
            return Err(ExplorerError::Cancelled);
        }
        on_event
            .send(FileOperationEventDto::Running {
                operation_id: operation_id.to_owned(),
                sequence: 1,
            })
            .map_err(|_| ExplorerError::ChannelClosed)?;

        let source = &request.sources[0];
        let entry = match &request.action {
            FileOperationActionDto::Rename { new_name } => {
                local.rename_entry(source, new_name, cancelled)?
            }
        };
        on_event
            .send(FileOperationEventDto::Completed {
                operation_id: operation_id.to_owned(),
                sequence: 2,
                entry: Box::new(entry),
            })
            .map_err(|_| ExplorerError::ChannelClosed)
    }

    fn finish(&self, operation_id: &str) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(operation_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Mutex as StdMutex, time::Duration};

    use serde_json::{json, Value};
    use tauri::ipc::InvokeResponseBody;
    use tempfile::TempDir;

    use crate::{
        filesystem::{DirectoryListingEvent, LocationRole},
        local_filesystem::LocalRoot,
    };

    use super::*;

    #[test]
    fn request_rejects_unknown_fields_and_preserves_a_typed_action() {
        let request: FileOperationRequestDto = serde_json::from_value(json!({
            "sources": [{ "id": "entry", "locationId": "home" }],
            "action": { "kind": "rename", "newName": "renamed.txt" }
        }))
        .expect("valid request");
        assert_eq!(
            request.action,
            FileOperationActionDto::Rename {
                new_name: "renamed.txt".to_owned()
            }
        );

        assert!(serde_json::from_value::<FileOperationRequestDto>(json!({
            "sources": [{ "id": "entry", "locationId": "home" }],
            "action": { "kind": "rename", "newName": "renamed.txt", "replace": true }
        }))
        .is_err());
    }

    #[test]
    fn terminal_events_keep_the_operation_identity_and_sequence() {
        let event = FileOperationEventDto::Cancelled {
            operation_id: "operation-1".to_owned(),
            sequence: 2,
        };
        assert_eq!(
            serde_json::to_value(event).expect("serializable event"),
            json!({
                "event": "cancelled",
                "operationId": "operation-1",
                "sequence": 2
            })
        );
    }

    #[tokio::test]
    async fn coordinator_streams_a_complete_local_rename_lifecycle() {
        let temp = TempDir::new().expect("temporary directory");
        fs::write(temp.path().join("notes.md"), b"hello").expect("fixture file");
        let local = Arc::new(
            LocalFilesystem::new(vec![LocalRoot {
                id: "home",
                name: "Home",
                role: LocationRole::Home,
                path: temp.path().to_path_buf(),
            }])
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
                        source = entries.into_iter().next().map(|entry| entry.reference);
                    }
                    Ok(())
                },
            )
            .expect("directory listing");
        let source = source.expect("source entry");
        let events = Arc::new(StdMutex::new(Vec::<Value>::new()));
        let captured_events = events.clone();
        let channel = Channel::new(move |body| {
            let InvokeResponseBody::Json(json) = body else {
                panic!("operation events must be JSON");
            };
            captured_events
                .lock()
                .expect("captured events")
                .push(serde_json::from_str(&json).expect("valid event JSON"));
            Ok(())
        });
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
                channel,
            )
            .expect("start operation");

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if events
                    .lock()
                    .expect("events")
                    .iter()
                    .any(|event| event.get("event").and_then(Value::as_str) == Some("completed"))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("completed operation");
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
        assert_eq!(
            fs::read(temp.path().join("renamed.md")).expect("renamed file"),
            b"hello"
        );
    }
}
