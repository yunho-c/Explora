use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use tauri::{ipc::Channel, State};

use crate::{
    filesystem::{
        DirectoryListingEvent, ExplorerError, ExplorerErrorDto, ImagePreviewMode,
        LocationSummaryDto, PreviewResultDto, PreviewUnavailableReason,
    },
    local_filesystem::LocalFilesystem,
    preferences::{
        PreferencesSnapshotDto, PreferencesStore, UserPreferencesDto, UserPreferencesPatchDto,
    },
    preview::{metadata_result, PreviewManager},
    ssh::{SshConnectionEventDto, SshConnectionManager, SshPromptResponseDto},
    ssh_targets::{ManualSshTargetInputDto, SshTargetStore, SshTargetSummaryDto},
    volumes::{VolumeManager, VolumeSnapshotEventDto},
};

pub struct AppState {
    local: Arc<LocalFilesystem>,
    preferences: Arc<PreferencesStore>,
    ssh_targets: Arc<SshTargetStore>,
    ssh: Arc<SshConnectionManager>,
    preview: Arc<PreviewManager>,
    volumes: Arc<VolumeManager>,
    listings: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl AppState {
    pub fn new(
        local: Arc<LocalFilesystem>,
        preferences: PreferencesStore,
        ssh_targets: SshTargetStore,
        volumes: Arc<VolumeManager>,
    ) -> Self {
        Self {
            local,
            preferences: Arc::new(preferences),
            ssh_targets: Arc::new(ssh_targets),
            ssh: Arc::new(SshConnectionManager::default()),
            preview: Arc::new(PreviewManager::default()),
            volumes,
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
    let cancellation = state
        .begin_listing(&request_id)
        .map_err(ExplorerErrorDto::from)?;
    let listing_request_id = request_id.clone();

    let result = if location_id.starts_with("ssh:") {
        state
            .ssh
            .list_directory(&location_id, &directory_id, &cancellation, |event| {
                on_event
                    .send(event)
                    .map_err(|_| ExplorerError::ChannelClosed)
            })
            .await
    } else {
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
    result.map_err(ExplorerErrorDto::from)
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

    if location_id.starts_with("ssh:") {
        return Ok(metadata_result(
            entry_id,
            None,
            None,
            PreviewUnavailableReason::Remote,
            "Remote content preview is not available yet.",
        ));
    }

    let path = state
        .local
        .resolve_preview_path(&entry_id, &location_id)
        .map_err(ExplorerErrorDto::from)?;
    state
        .preview
        .prepare_local(request_id, entry_id, path, image_mode)
        .await
        .map_err(ExplorerErrorDto::from)
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
}
