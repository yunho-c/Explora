use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use tauri::{ipc::Channel, AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::{
    filesystem::{
        ContentAvailability, DirectoryListingEvent, ExplorerError, ExplorerErrorDto,
        ImagePreviewMode, LocationSummaryDto, PreviewResultDto, PreviewUnavailableReason,
    },
    local_filesystem::LocalFilesystem,
    preferences::{
        PreferencesSnapshotDto, PreferencesStore, UserPreferencesDto, UserPreferencesPatchDto,
    },
    preview::{metadata_result, PreviewManager},
    ssh::{SshConnectionEventDto, SshConnectionManager, SshPromptResponseDto},
    ssh_targets::{ManualSshTargetInputDto, SshTargetStore, SshTargetSummaryDto},
    synced_folders::{SyncedFolderManager, SyncedFolderSnapshotEventDto},
    volumes::{VolumeManager, VolumeSnapshotEventDto},
};

pub struct AppState {
    local: Arc<LocalFilesystem>,
    preferences: Arc<PreferencesStore>,
    ssh_targets: Arc<SshTargetStore>,
    ssh: Arc<SshConnectionManager>,
    preview: Arc<PreviewManager>,
    volumes: Arc<VolumeManager>,
    synced_folders: Arc<SyncedFolderManager>,
    listings: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl AppState {
    pub fn new(
        local: Arc<LocalFilesystem>,
        preferences: PreferencesStore,
        ssh_targets: SshTargetStore,
        volumes: Arc<VolumeManager>,
        synced_folders: Arc<SyncedFolderManager>,
    ) -> Self {
        Self {
            local,
            preferences: Arc::new(preferences),
            ssh_targets: Arc::new(ssh_targets),
            ssh: Arc::new(SshConnectionManager::default()),
            preview: Arc::new(PreviewManager::default()),
            volumes,
            synced_folders,
            listings: Mutex::new(HashMap::new()),
        }
    }

    fn begin_listing(&self, request_id: &str) -> Result<Arc<AtomicBool>, ExplorerError> {
        validate_request_id(request_id)?;
        let cancellation = Arc::new(AtomicBool::new(false));
        let mut listings = self
            .listings
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        if listings.contains_key(request_id) {
            return Err(ExplorerError::InvalidReference);
        }
        listings.insert(request_id.to_owned(), cancellation.clone());
        Ok(cancellation)
    }

    fn finish_listing(&self, request_id: &str) {
        if let Ok(mut listings) = self.listings.lock() {
            listings.remove(request_id);
        }
    }

    fn cancel_listing(&self, request_id: &str) -> Result<(), ExplorerError> {
        if let Some(cancellation) = self
            .listings
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .get(request_id)
        {
            cancellation.store(true, Ordering::Relaxed);
        }
        Ok(())
    }
}

fn validate_request_id(request_id: &str) -> Result<(), ExplorerError> {
    if request_id.is_empty() || request_id.len() > 128 {
        Err(ExplorerError::InvalidReference)
    } else {
        Ok(())
    }
}

fn redact_synced_folder_error(error: ExplorerError) -> ExplorerError {
    match error {
        ExplorerError::Io { kind, .. } => ExplorerError::Io {
            message: "Explora could not read this synced folder.".to_owned(),
            kind,
        },
        error => error,
    }
}

#[tauri::command]
pub fn get_user_preferences(
    state: State<'_, AppState>,
) -> Result<PreferencesSnapshotDto, ExplorerErrorDto> {
    state.preferences.snapshot().map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub async fn update_user_preferences(
    state: State<'_, AppState>,
    patch: UserPreferencesPatchDto,
) -> Result<UserPreferencesDto, ExplorerErrorDto> {
    let preferences = state.preferences.clone();
    tauri::async_runtime::spawn_blocking(move || preferences.update(patch))
        .await
        .map_err(|error| {
            ExplorerError::Unexpected(format!("The preference update task failed: {error}"))
        })?
        .map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub fn list_locations(
    state: State<'_, AppState>,
) -> Result<Vec<LocationSummaryDto>, ExplorerErrorDto> {
    let mut locations = state.local.locations().map_err(ExplorerErrorDto::from)?;
    locations.extend(state.ssh.locations());
    Ok(locations)
}

#[tauri::command]
pub fn watch_volumes(
    state: State<'_, AppState>,
    request_id: String,
    on_event: Channel<VolumeSnapshotEventDto>,
) -> Result<(), ExplorerErrorDto> {
    validate_request_id(&request_id).map_err(ExplorerErrorDto::from)?;
    state
        .volumes
        .subscribe(request_id, on_event)
        .map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub fn cancel_volume_watch(
    state: State<'_, AppState>,
    request_id: String,
) -> Result<(), ExplorerErrorDto> {
    state
        .volumes
        .unsubscribe(&request_id)
        .map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub fn watch_synced_folders(
    state: State<'_, AppState>,
    request_id: String,
    on_event: Channel<SyncedFolderSnapshotEventDto>,
) -> Result<(), ExplorerErrorDto> {
    validate_request_id(&request_id).map_err(ExplorerErrorDto::from)?;
    state
        .synced_folders
        .subscribe(request_id, on_event)
        .map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub fn cancel_synced_folder_watch(
    state: State<'_, AppState>,
    request_id: String,
) -> Result<(), ExplorerErrorDto> {
    state
        .synced_folders
        .unsubscribe(&request_id)
        .map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub async fn add_synced_folder(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>, ExplorerErrorDto> {
    if !state.synced_folders.can_add_folder() {
        return Err(ExplorerErrorDto::from(ExplorerError::Unsupported(
            "Adding synced folders manually is not supported on this platform.".to_owned(),
        )));
    }
    let Some(selection) = app.dialog().file().blocking_pick_folder() else {
        return Ok(None);
    };
    let path = selection.into_path().map_err(|_| {
        ExplorerErrorDto::from(ExplorerError::InvalidConfiguration(
            "The selected synced folder is not a local filesystem directory.".to_owned(),
        ))
    })?;
    state
        .synced_folders
        .add_manual_folder(path)
        .map(Some)
        .map_err(|error| synced_folder_configuration_error(error).into())
}

#[tauri::command]
pub async fn remove_synced_folder(
    state: State<'_, AppState>,
    folder_id: String,
) -> Result<(), ExplorerErrorDto> {
    state
        .synced_folders
        .remove_manual_folder(&folder_id)
        .map_err(|error| synced_folder_configuration_error(error).into())
}

fn synced_folder_configuration_error(error: ExplorerError) -> ExplorerError {
    match error {
        ExplorerError::Io { kind, .. } => ExplorerError::Io {
            message: "Explora could not update its saved synced folders.".to_owned(),
            kind,
        },
        error => error,
    }
}

#[tauri::command]
pub fn list_ssh_targets(
    state: State<'_, AppState>,
) -> Result<Vec<SshTargetSummaryDto>, ExplorerErrorDto> {
    let mut targets = state.ssh_targets.list().map_err(ExplorerErrorDto::from)?;
    state.ssh.apply_statuses(&mut targets);
    Ok(targets)
}

#[tauri::command]
pub fn create_ssh_target(
    state: State<'_, AppState>,
    input: ManualSshTargetInputDto,
) -> Result<SshTargetSummaryDto, ExplorerErrorDto> {
    state
        .ssh_targets
        .create(input)
        .map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub async fn update_ssh_target(
    state: State<'_, AppState>,
    target_id: String,
    input: ManualSshTargetInputDto,
) -> Result<SshTargetSummaryDto, ExplorerErrorDto> {
    let summary = state
        .ssh_targets
        .update(&target_id, input)
        .map_err(ExplorerErrorDto::from)?;
    state
        .ssh
        .forget_target(&target_id)
        .await
        .map_err(ExplorerErrorDto::from)?;
    Ok(summary)
}

#[tauri::command]
pub async fn delete_ssh_target(
    state: State<'_, AppState>,
    target_id: String,
) -> Result<(), ExplorerErrorDto> {
    state
        .ssh
        .forget_target(&target_id)
        .await
        .map_err(ExplorerErrorDto::from)?;
    state
        .ssh_targets
        .delete(&target_id)
        .map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub async fn connect_ssh_target(
    state: State<'_, AppState>,
    request_id: String,
    target_id: String,
    on_event: Channel<SshConnectionEventDto>,
) -> Result<LocationSummaryDto, ExplorerErrorDto> {
    validate_request_id(&request_id).map_err(ExplorerErrorDto::from)?;
    let target = state
        .ssh_targets
        .resolve(&target_id)
        .map_err(ExplorerErrorDto::from)?;
    state
        .ssh
        .connect(target, request_id, on_event)
        .await
        .map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub fn respond_ssh_prompt(
    state: State<'_, AppState>,
    request_id: String,
    prompt_id: String,
    response: SshPromptResponseDto,
) -> Result<(), ExplorerErrorDto> {
    state
        .ssh
        .respond(&request_id, &prompt_id, response)
        .map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub fn cancel_ssh_connection(
    state: State<'_, AppState>,
    request_id: String,
) -> Result<(), ExplorerErrorDto> {
    state
        .ssh
        .cancel_connection(&request_id)
        .map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub async fn disconnect_ssh_target(
    state: State<'_, AppState>,
    target_id: String,
) -> Result<(), ExplorerErrorDto> {
    state
        .ssh
        .disconnect(&target_id)
        .await
        .map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub async fn list_directory(
    state: State<'_, AppState>,
    request_id: String,
    directory_id: String,
    location_id: String,
    on_event: Channel<DirectoryListingEvent>,
) -> Result<(), ExplorerErrorDto> {
    let local_location = state
        .local
        .contains_location(&location_id)
        .map_err(ExplorerErrorDto::from)?;
    let ssh_location = state
        .ssh
        .contains_location(&location_id)
        .map_err(ExplorerErrorDto::from)?;
    if local_location == ssh_location {
        return Err(ExplorerErrorDto::from(ExplorerError::InvalidReference));
    }
    let synced_location = local_location
        && state
            .local
            .is_synced_folder(&location_id)
            .map_err(ExplorerErrorDto::from)?;

    let cancellation = state
        .begin_listing(&request_id)
        .map_err(ExplorerErrorDto::from)?;
    let listing_request_id = request_id.clone();

    let result = if ssh_location {
        state
            .ssh
            .list_directory(&location_id, &directory_id, &cancellation, |event| {
                on_event
                    .send(event)
                    .map_err(|_| ExplorerError::ChannelClosed)
            })
            .await
    } else {
        debug_assert!(local_location);
        let local = state.local.clone();
        tauri::async_runtime::spawn_blocking(move || {
            local.list_directory(&directory_id, &location_id, &cancellation, |event| {
                on_event
                    .send(event)
                    .map_err(|_| ExplorerError::ChannelClosed)
            })
        })
        .await
        .map_err(|error| {
            ExplorerError::Unexpected(format!("The directory listing task failed: {error}"))
        })?
    };

    state.finish_listing(&listing_request_id);
    result
        .map_err(|error| {
            if synced_location {
                redact_synced_folder_error(error)
            } else {
                error
            }
        })
        .map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub fn cancel_listing(
    state: State<'_, AppState>,
    request_id: String,
) -> Result<(), ExplorerErrorDto> {
    state
        .cancel_listing(&request_id)
        .map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub async fn prepare_preview(
    state: State<'_, AppState>,
    request_id: String,
    entry_id: String,
    location_id: String,
    image_mode: ImagePreviewMode,
) -> Result<PreviewResultDto, ExplorerErrorDto> {
    validate_request_id(&request_id).map_err(ExplorerErrorDto::from)?;
    validate_reference_id(&entry_id).map_err(ExplorerErrorDto::from)?;
    validate_reference_id(&location_id).map_err(ExplorerErrorDto::from)?;

    let ssh_location = state
        .ssh
        .contains_location(&location_id)
        .map_err(ExplorerErrorDto::from)?;
    let local_location = state
        .local
        .contains_location(&location_id)
        .map_err(ExplorerErrorDto::from)?;
    if local_location == ssh_location {
        return Err(ExplorerErrorDto::from(ExplorerError::InvalidReference));
    }

    if ssh_location {
        return Ok(metadata_result(
            entry_id,
            None,
            None,
            PreviewUnavailableReason::Remote,
            "Remote content preview is not available yet.",
        ));
    }

    let synced_location = state
        .local
        .is_synced_folder(&location_id)
        .map_err(ExplorerErrorDto::from)?;

    debug_assert!(local_location);

    // Resolve and revalidate the opaque entry before deciding whether content
    // access is allowed. Filesystem metadata is sufficient for this decision
    // and does not open the file or request provider hydration.
    let access = state
        .local
        .resolve_preview_access(&entry_id, &location_id)
        .map_err(|error| local_preview_error(error, synced_location))?;

    if access.availability != ContentAvailability::Local {
        return Ok(metadata_result(
            entry_id,
            access.size,
            access.modified_at,
            PreviewUnavailableReason::DownloadRequired,
            synced_preview_message(access.availability),
        ));
    }

    state
        .preview
        .prepare_local(request_id, entry_id, access.path, image_mode)
        .await
        .map_err(|error| local_preview_error(error, synced_location))
}

fn local_preview_error(error: ExplorerError, synced_location: bool) -> ExplorerErrorDto {
    ExplorerErrorDto::from(if synced_location {
        redact_synced_folder_error(error)
    } else {
        error
    })
}

fn synced_preview_message(availability: ContentAvailability) -> &'static str {
    match availability {
        ContentAvailability::OnlineOnly => {
            "This file is online-only. Download it explicitly before previewing."
        }
        ContentAvailability::Partial => {
            "Only part of this file is available locally. Download it explicitly before previewing."
        }
        ContentAvailability::Downloading => {
            "This file is still downloading. Try the preview again when the download finishes."
        }
        ContentAvailability::Syncing => {
            "This file's local copy is not current yet. Try the preview again after it finishes syncing."
        }
        ContentAvailability::Error => {
            "The operating system reported a download error for this file."
        }
        ContentAvailability::Unknown => {
            "Explora cannot verify that this file is available locally. Download it explicitly before previewing."
        }
        ContentAvailability::Local => "This file is available locally.",
    }
}

#[tauri::command]
pub fn cancel_preview(
    state: State<'_, AppState>,
    request_id: String,
) -> Result<(), ExplorerErrorDto> {
    validate_request_id(&request_id).map_err(ExplorerErrorDto::from)?;
    state
        .preview
        .cancel(&request_id)
        .map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub fn read_preview_resource(
    state: State<'_, AppState>,
    resource_id: String,
) -> Result<tauri::ipc::Response, ExplorerErrorDto> {
    validate_reference_id(&resource_id).map_err(ExplorerErrorDto::from)?;
    state
        .preview
        .take_resource(&resource_id)
        .map(tauri::ipc::Response::new)
        .map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub fn discard_preview_resource(
    state: State<'_, AppState>,
    resource_id: String,
) -> Result<(), ExplorerErrorDto> {
    validate_reference_id(&resource_id).map_err(ExplorerErrorDto::from)?;
    state
        .preview
        .discard_resource(&resource_id)
        .map_err(ExplorerErrorDto::from)
}

fn validate_reference_id(value: &str) -> Result<(), ExplorerError> {
    if value.is_empty() || value.len() > 256 {
        Err(ExplorerError::InvalidReference)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_ids_are_bounded() {
        assert!(validate_request_id("request-1").is_ok());
        assert!(validate_request_id("").is_err());
        assert!(validate_request_id(&"x".repeat(129)).is_err());
    }

    #[test]
    fn reference_ids_are_bounded() {
        assert!(validate_reference_id("entry-1").is_ok());
        assert!(validate_reference_id("").is_err());
        assert!(validate_reference_id(&"x".repeat(257)).is_err());
    }

    #[test]
    fn unexpected_error_code_remains_serializable() {
        let error = ExplorerErrorDto {
            code: crate::filesystem::ExplorerErrorCode::Unexpected,
            message: "failed".to_owned(),
        };
        assert_eq!(error.code, crate::filesystem::ExplorerErrorCode::Unexpected);
    }

    #[test]
    fn synced_folder_errors_do_not_expose_provider_paths() {
        let error = ExplorerError::Io {
            message:
                "could not open /Users/person/Library/CloudStorage/Provider-account@example.com"
                    .to_owned(),
            kind: std::io::ErrorKind::PermissionDenied,
        };

        let redacted = redact_synced_folder_error(error).to_string();
        assert_eq!(redacted, "Explora could not read this synced folder.");
        assert!(!redacted.contains("account@example.com"));
    }

    #[test]
    fn synced_folder_configuration_errors_do_not_expose_selected_paths() {
        let error = ExplorerError::Io {
            message: "could not save /home/person/private-cloud".to_owned(),
            kind: std::io::ErrorKind::PermissionDenied,
        };

        let redacted = synced_folder_configuration_error(error).to_string();
        assert_eq!(
            redacted,
            "Explora could not update its saved synced folders."
        );
        assert!(!redacted.contains("private-cloud"));
    }

    #[test]
    fn synced_preview_errors_do_not_expose_provider_paths() {
        let error = ExplorerError::Io {
            message: "could not preview /Users/person/Library/Mobile Documents/private.txt"
                .to_owned(),
            kind: std::io::ErrorKind::PermissionDenied,
        };

        let mapped = local_preview_error(error, true);
        assert_eq!(mapped.message, "Explora could not read this synced folder.");
        assert!(!mapped.message.contains("private.txt"));
    }

    #[test]
    fn synced_preview_messages_distinguish_authoritative_availability() {
        assert!(synced_preview_message(ContentAvailability::OnlineOnly).contains("online-only"));
        assert!(synced_preview_message(ContentAvailability::Downloading).contains("downloading"));
        assert!(synced_preview_message(ContentAvailability::Syncing).contains("not current"));
        assert!(synced_preview_message(ContentAvailability::Error).contains("download error"));
        assert!(synced_preview_message(ContentAvailability::Unknown).contains("cannot verify"));
    }
}
