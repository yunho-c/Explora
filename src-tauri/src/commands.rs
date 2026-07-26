use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use tauri::{
    ipc::{Channel, Response},
    State, WebviewWindow,
};

use crate::{
    file_operations::{
        FileOperationCoordinator, FileOperationEventDto, FileOperationPromptResponseDto,
        FileOperationRequestDto,
    },
    filesystem::{
        DirectoryListingEvent, ExplorerError, ExplorerErrorDto, ImagePreviewMode,
        LocationSummaryDto, PreviewResultDto, PreviewUnavailableReason,
    },
    local_filesystem::LocalFilesystem,
    native_open::{
        NativeOpenEventDto, NativeOpenManager, NativeOpenOutcomeDto, LARGE_REMOTE_OPEN_BYTES,
        MAX_REMOTE_OPEN_BYTES,
    },
    preferences::{
        PreferencesSnapshotDto, PreferencesStore, UserPreferencesDto, UserPreferencesPatchDto,
    },
    preview::{metadata_result, PreviewManager},
    ssh::{SshConnectionEventDto, SshConnectionManager, SshPromptResponseDto},
    ssh_targets::{ManualSshTargetInputDto, SshTargetStore, SshTargetSummaryDto},
    terminal::{
        types::{TerminalCloseReason, TerminalSessionSummaryDto, TerminalSizeDto},
        LocalTerminalLaunch, SshTerminalLaunch, TerminalCoordinator,
    },
    volumes::{VolumeManager, VolumeSnapshotEventDto},
};

pub struct AppState {
    local: Arc<LocalFilesystem>,
    preferences: Arc<PreferencesStore>,
    ssh_targets: Arc<SshTargetStore>,
    ssh: Arc<SshConnectionManager>,
    preview: Arc<PreviewManager>,
    terminal: Arc<TerminalCoordinator>,
    volumes: Arc<VolumeManager>,
    native_open: Arc<NativeOpenManager>,
    listings: Mutex<HashMap<String, Arc<AtomicBool>>>,
    operations: Arc<FileOperationCoordinator>,
}

impl AppState {
    pub fn new(
        local: Arc<LocalFilesystem>,
        preferences: PreferencesStore,
        ssh_targets: SshTargetStore,
        volumes: Arc<VolumeManager>,
        native_open: NativeOpenManager,
    ) -> Self {
        Self {
            local,
            preferences: Arc::new(preferences),
            ssh_targets: Arc::new(ssh_targets),
            ssh: Arc::new(SshConnectionManager::default()),
            preview: Arc::new(PreviewManager::default()),
            terminal: Arc::new(TerminalCoordinator::default()),
            volumes,
            native_open: Arc::new(native_open),
            listings: Mutex::new(HashMap::new()),
            operations: Arc::new(FileOperationCoordinator::default()),
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

    pub fn close_terminals_for_window(&self, window_label: &str) {
        self.terminal
            .close_window(window_label, TerminalCloseReason::WindowClosed);
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
pub fn get_native_open_status(state: State<'_, AppState>) -> Option<String> {
    state.native_open.startup_warning()
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
pub async fn create_terminal(
    window: WebviewWindow,
    state: State<'_, AppState>,
    request_id: String,
    location_id: String,
    directory_id: Option<String>,
    size: TerminalSizeDto,
    on_event: Channel<Response>,
) -> Result<TerminalSessionSummaryDto, ExplorerErrorDto> {
    validate_request_id(&request_id).map_err(ExplorerErrorDto::from)?;
    validate_reference_id(&location_id).map_err(ExplorerErrorDto::from)?;
    if location_id.starts_with("ssh:") {
        size.validate().map_err(ExplorerErrorDto::from)?;
        let opened = state
            .ssh
            .open_terminal(&location_id, size)
            .await
            .map_err(ExplorerErrorDto::from)?;
        return state
            .terminal
            .create_ssh(SshTerminalLaunch {
                window_label: window.label(),
                location_id: &opened.location_id,
                title: &opened.title,
                context_label: &opened.context_label,
                channel: opened.channel,
                on_event,
            })
            .map_err(ExplorerErrorDto::from);
    }
    let directory_id = directory_id.ok_or(ExplorerError::InvalidReference)?;
    validate_reference_id(&directory_id).map_err(ExplorerErrorDto::from)?;
    size.validate().map_err(ExplorerErrorDto::from)?;

    let local = state.local.clone();
    let terminal = state.terminal.clone();
    let window_label = window.label().to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        let directory = local.resolve_terminal_directory(&directory_id, &location_id)?;
        terminal.create_local(LocalTerminalLaunch {
            window_label: &window_label,
            location_id: &location_id,
            working_directory: &directory.path,
            title: &directory.title,
            context_label: &directory.context_label,
            size,
            on_event,
        })
    })
    .await
    .map_err(|error| {
        ExplorerErrorDto::from(ExplorerError::Unexpected(format!(
            "The terminal startup task failed: {error}"
        )))
    })?
    .map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub async fn write_terminal(
    window: WebviewWindow,
    state: State<'_, AppState>,
    session_id: String,
    input_sequence: u64,
    bytes: Vec<u8>,
) -> Result<(), ExplorerErrorDto> {
    validate_reference_id(&session_id).map_err(ExplorerErrorDto::from)?;
    if bytes.is_empty() || bytes.len() > crate::terminal::types::TerminalPolicy::MAX_INPUT_BYTES {
        return Err(ExplorerErrorDto::from(ExplorerError::InvalidReference));
    }
    let terminal = state.terminal.clone();
    let window_label = window.label().to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        terminal.write(&window_label, &session_id, input_sequence, &bytes)
    })
    .await
    .map_err(|error| {
        ExplorerErrorDto::from(ExplorerError::Unexpected(format!(
            "The terminal input task failed: {error}"
        )))
    })?
    .map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub fn resize_terminal(
    window: WebviewWindow,
    state: State<'_, AppState>,
    session_id: String,
    size: TerminalSizeDto,
) -> Result<(), ExplorerErrorDto> {
    validate_reference_id(&session_id).map_err(ExplorerErrorDto::from)?;
    state
        .terminal
        .resize(window.label(), &session_id, size)
        .map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub fn acknowledge_terminal_output(
    window: WebviewWindow,
    state: State<'_, AppState>,
    session_id: String,
    output_sequence: u64,
) -> Result<(), ExplorerErrorDto> {
    validate_reference_id(&session_id).map_err(ExplorerErrorDto::from)?;
    state
        .terminal
        .acknowledge(window.label(), &session_id, output_sequence)
        .map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub fn close_terminal(
    window: WebviewWindow,
    state: State<'_, AppState>,
    session_id: String,
    reason: TerminalCloseReason,
) -> Result<(), ExplorerErrorDto> {
    validate_reference_id(&session_id).map_err(ExplorerErrorDto::from)?;
    state
        .terminal
        .close(window.label(), &session_id, reason)
        .map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub fn start_file_operation(
    state: State<'_, AppState>,
    request: FileOperationRequestDto,
    on_event: Channel<FileOperationEventDto>,
) -> Result<String, ExplorerErrorDto> {
    state
        .operations
        .start_with_backends(state.local.clone(), state.ssh.clone(), request, on_event)
        .map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub fn cancel_file_operation(
    state: State<'_, AppState>,
    operation_id: String,
) -> Result<(), ExplorerErrorDto> {
    validate_request_id(&operation_id).map_err(ExplorerErrorDto::from)?;
    state
        .operations
        .cancel(&operation_id)
        .map_err(ExplorerErrorDto::from)
}

#[tauri::command]
pub fn respond_file_operation(
    state: State<'_, AppState>,
    operation_id: String,
    prompt_id: String,
    response: FileOperationPromptResponseDto,
) -> Result<(), ExplorerErrorDto> {
    validate_request_id(&operation_id).map_err(ExplorerErrorDto::from)?;
    validate_reference_id(&prompt_id).map_err(ExplorerErrorDto::from)?;
    state
        .operations
        .respond(&operation_id, &prompt_id, response)
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

#[tauri::command]
pub async fn open_entry(
    state: State<'_, AppState>,
    request_id: String,
    entry_id: String,
    location_id: String,
    allow_large_remote_download: bool,
    on_event: Channel<NativeOpenEventDto>,
) -> Result<NativeOpenOutcomeDto, ExplorerErrorDto> {
    validate_request_id(&request_id).map_err(ExplorerErrorDto::from)?;
    validate_reference_id(&entry_id).map_err(ExplorerErrorDto::from)?;
    validate_reference_id(&location_id).map_err(ExplorerErrorDto::from)?;
    let cancellation = state
        .native_open
        .begin(&request_id)
        .map_err(ExplorerErrorDto::from)?;

    let result = open_entry_inner(
        &state,
        &entry_id,
        &location_id,
        allow_large_remote_download,
        &on_event,
        &cancellation,
    )
    .await;
    state.native_open.finish(&request_id);
    result.map_err(ExplorerErrorDto::from)
}

async fn open_entry_inner(
    state: &State<'_, AppState>,
    entry_id: &str,
    location_id: &str,
    allow_large_remote_download: bool,
    on_event: &Channel<NativeOpenEventDto>,
    cancellation: &AtomicBool,
) -> Result<NativeOpenOutcomeDto, ExplorerError> {
    if cancellation.load(Ordering::Relaxed) {
        return Err(ExplorerError::Cancelled);
    }

    if !location_id.starts_with("ssh:") {
        let local = state.local.clone();
        let native_open = state.native_open.clone();
        let entry_id = entry_id.to_owned();
        let location_id = location_id.to_owned();
        on_event
            .send(NativeOpenEventDto::Launching)
            .map_err(|_| ExplorerError::ChannelClosed)?;
        return tauri::async_runtime::spawn_blocking(move || {
            let path = local.resolve_native_open_path(&entry_id, &location_id)?;
            native_open.open(&path)?;
            Ok(NativeOpenOutcomeDto::Opened)
        })
        .await
        .map_err(|error| {
            ExplorerError::Unexpected(format!("The native-open task failed: {error}"))
        })?;
    }

    let metadata = state
        .ssh
        .native_open_metadata(location_id, entry_id, cancellation)
        .await?;
    if metadata
        .size
        .is_some_and(|size| size > MAX_REMOTE_OPEN_BYTES)
    {
        return Err(ExplorerError::Unsupported(
            "Remote files larger than 2 GiB cannot be opened yet.".to_owned(),
        ));
    }
    if !allow_large_remote_download
        && metadata
            .size
            .is_none_or(|size| size > LARGE_REMOTE_OPEN_BYTES)
    {
        return Ok(NativeOpenOutcomeDto::ConfirmationRequired {
            size: metadata.size.map(|size| size.to_string()),
        });
    }

    on_event
        .send(NativeOpenEventDto::Queued)
        .map_err(|_| ExplorerError::ChannelClosed)?;
    let _permit = state
        .native_open
        .acquire_download_slot(cancellation)
        .await?;
    if cancellation.load(Ordering::Relaxed) {
        return Err(ExplorerError::Cancelled);
    }
    let name = state.ssh.native_open_name(location_id, entry_id)?;
    let (partial_path, final_path) = state.native_open.destination(&name)?;
    let download = state
        .ssh
        .download_for_native_open(
            location_id,
            entry_id,
            &partial_path,
            cancellation,
            MAX_REMOTE_OPEN_BYTES,
            |transferred, total| {
                on_event
                    .send(NativeOpenEventDto::Downloading {
                        transferred_bytes: transferred.to_string(),
                        total_bytes: total.map(|value| value.to_string()),
                    })
                    .map_err(|_| ExplorerError::ChannelClosed)
            },
        )
        .await;
    let download = match download {
        Ok(download) => download,
        Err(error) => {
            state.native_open.discard_snapshot(&partial_path);
            return Err(error);
        }
    };
    if cancellation.load(Ordering::Relaxed) {
        state.native_open.discard_snapshot(&partial_path);
        return Err(ExplorerError::Cancelled);
    }
    if let Err(error) =
        state
            .native_open
            .finalize_remote_snapshot(&partial_path, &final_path, download.executable)
    {
        state.native_open.discard_snapshot(&partial_path);
        return Err(error);
    }
    if cancellation.load(Ordering::Relaxed) {
        state.native_open.discard_snapshot(&final_path);
        return Err(ExplorerError::Cancelled);
    }
    if on_event.send(NativeOpenEventDto::Launching).is_err() {
        state.native_open.discard_snapshot(&final_path);
        return Err(ExplorerError::ChannelClosed);
    }
    if let Err(error) = state.native_open.open(&final_path) {
        state.native_open.discard_snapshot(&final_path);
        return Err(error);
    }
    Ok(NativeOpenOutcomeDto::Opened)
}

#[tauri::command]
pub fn cancel_open_entry(
    state: State<'_, AppState>,
    request_id: String,
) -> Result<(), ExplorerErrorDto> {
    validate_request_id(&request_id).map_err(ExplorerErrorDto::from)?;
    state
        .native_open
        .cancel(&request_id)
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
