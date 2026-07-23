use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use tauri::{ipc::Channel, AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::{oneshot, Notify, Semaphore};

use crate::{
    content_request::{ContentRequestManager, ContentRequestPolicy},
    filesystem::{
        ContentAvailability, ContentRequestCapabilityDto, ContentRequestEventDto,
        ContentRequestIntent, DirectoryListingEvent, ExplorerError, ExplorerErrorDto,
        ImagePreviewMode, LocationBackend, LocationSummaryDto, PreviewResultDto,
        PreviewUnavailableReason, SyncedFolderStatus,
    },
    gio_filesystem::GioFilesystem,
    local_filesystem::LocalFilesystem,
    preferences::{
        PreferencesSnapshotDto, PreferencesStore, UserPreferencesDto, UserPreferencesPatchDto,
    },
    preview::{
        metadata_result, metadata_result_with_content_request, ExternalPreviewSource,
        PreviewManager,
    },
    ssh::{SshConnectionEventDto, SshConnectionManager, SshPromptResponseDto},
    ssh_targets::{ManualSshTargetInputDto, SshTargetStore, SshTargetSummaryDto},
    synced_folders::{SyncedFolderManager, SyncedFolderSnapshotEventDto},
    volumes::{VolumeManager, VolumeSnapshotEventDto},
};

struct ListingControl {
    cancelled: AtomicBool,
    notification: Notify,
}

impl ListingControl {
    fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            notification: Notify::new(),
        }
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        // There is one command waiter. `notify_one` also retains a permit if
        // cancellation wins the race before that waiter starts polling.
        self.notification.notify_one();
    }

    async fn cancelled(&self) {
        if self.cancelled.load(Ordering::Acquire) {
            return;
        }
        self.notification.notified().await;
    }
}

pub struct AppState {
    local: Arc<LocalFilesystem>,
    gio: Arc<GioFilesystem>,
    preferences: Arc<PreferencesStore>,
    ssh_targets: Arc<SshTargetStore>,
    ssh: Arc<SshConnectionManager>,
    preview: Arc<PreviewManager>,
    content_requests: Arc<ContentRequestManager>,
    volumes: Arc<VolumeManager>,
    synced_folders: Arc<SyncedFolderManager>,
    listings: Mutex<HashMap<String, Arc<ListingControl>>>,
    synced_listing_limiter: Arc<Semaphore>,
}

impl AppState {
    pub fn new(
        local: Arc<LocalFilesystem>,
        gio: Arc<GioFilesystem>,
        preferences: PreferencesStore,
        ssh_targets: SshTargetStore,
        volumes: Arc<VolumeManager>,
        synced_folders: Arc<SyncedFolderManager>,
    ) -> Self {
        Self {
            local,
            gio,
            preferences: Arc::new(preferences),
            ssh_targets: Arc::new(ssh_targets),
            ssh: Arc::new(SshConnectionManager::default()),
            preview: Arc::new(PreviewManager::default()),
            content_requests: Arc::new(ContentRequestManager::default()),
            volumes,
            synced_folders,
            listings: Mutex::new(HashMap::new()),
            synced_listing_limiter: Arc::new(Semaphore::new(MAX_CONCURRENT_SYNCED_LISTINGS)),
        }
    }

    fn begin_listing(&self, request_id: &str) -> Result<Arc<ListingControl>, ExplorerError> {
        validate_request_id(request_id)?;
        let cancellation = Arc::new(ListingControl::new());
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
            cancellation.cancel();
        }
        Ok(())
    }

    fn resolve_location_backend(
        &self,
        location_id: &str,
    ) -> Result<LocationBackend, ExplorerError> {
        let local = self.local.contains_location(location_id)?;
        let gio = self.gio.contains_location(location_id)?;
        let ssh = self.ssh.contains_location(location_id)?;
        match (local, gio, ssh) {
            (true, false, false) => Ok(LocationBackend::Local),
            (false, true, false) => Ok(LocationBackend::Gio),
            (false, false, true) => Ok(LocationBackend::Ssh),
            (false, false, false)
                if self.local.is_unavailable_location(location_id)?
                    || self.gio.is_unavailable_location(location_id)? =>
            {
                Err(ExplorerError::Unavailable(
                    "This location is no longer available.".to_owned(),
                ))
            }
            _ => Err(ExplorerError::InvalidReference),
        }
    }
}

const CONTENT_REQUEST_POLL_INTERVAL: Duration = Duration::from_millis(250);
const CONTENT_REQUEST_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const SYNCED_LISTING_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CONCURRENT_SYNCED_LISTINGS: usize = 4;

async fn await_bounded_synced_listing(
    result: oneshot::Receiver<Result<(), ExplorerError>>,
    control: Arc<ListingControl>,
    timeout: Duration,
) -> Result<(), ExplorerError> {
    tokio::select! {
        biased;
        _ = control.cancelled() => Err(ExplorerError::Cancelled),
        result = result => result.unwrap_or_else(|_| {
            Err(ExplorerError::Unexpected(
                "The synced-folder listing task stopped unexpectedly.".to_owned(),
            ))
        }),
        _ = tokio::time::sleep(timeout) => {
            control.cancel();
            Err(ExplorerError::TimedOut(
                "The sync provider took too long to open this folder.".to_owned(),
            ))
        },
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
    locations.extend(state.gio.locations().map_err(ExplorerErrorDto::from)?);
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
    let backend = state
        .resolve_location_backend(&location_id)
        .map_err(ExplorerErrorDto::from)?;
    let synced_location = backend == LocationBackend::Gio
        || (backend == LocationBackend::Local
            && state
                .local
                .is_synced_folder(&location_id)
                .map_err(ExplorerErrorDto::from)?);
    let synced_listing_permit = if synced_location {
        Some(
            state
                .synced_listing_limiter
                .clone()
                .try_acquire_owned()
                .map_err(|_| {
                    ExplorerErrorDto::from(ExplorerError::Unexpected(
                        "Too many synced folders are still waiting on their providers.".to_owned(),
                    ))
                })?,
        )
    } else {
        None
    };

    let cancellation = state
        .begin_listing(&request_id)
        .map_err(ExplorerErrorDto::from)?;
    let listing_request_id = request_id.clone();

    let result = match backend {
        LocationBackend::Ssh => {
            state
                .ssh
                .list_directory(
                    &location_id,
                    &directory_id,
                    &cancellation.cancelled,
                    |event| {
                        on_event
                            .send(event)
                            .map_err(|_| ExplorerError::ChannelClosed)
                    },
                )
                .await
        }
        LocationBackend::Gio => {
            let gio = state.gio.clone();
            let worker_cancellation = cancellation.clone();
            let (result_sender, result_receiver) = oneshot::channel();
            tauri::async_runtime::spawn_blocking(move || {
                // A provider-blocked worker retains this permit even after the
                // command returns a timeout, bounding abandoned GIO calls.
                let _permit = synced_listing_permit;
                let result = gio.list_directory(
                    &directory_id,
                    &location_id,
                    &worker_cancellation.cancelled,
                    |event| {
                        on_event
                            .send(event)
                            .map_err(|_| ExplorerError::ChannelClosed)
                    },
                );
                let _ = result_sender.send(result);
            });

            await_bounded_synced_listing(
                result_receiver,
                cancellation.clone(),
                SYNCED_LISTING_TIMEOUT,
            )
            .await
        }
        LocationBackend::Local => {
            let local = state.local.clone();
            let worker_cancellation = cancellation.clone();
            let (result_sender, result_receiver) = oneshot::channel();
            tauri::async_runtime::spawn_blocking(move || {
                // A provider-blocked worker retains this permit even after the
                // command returns a timeout, bounding abandoned native calls.
                let _permit = synced_listing_permit;
                let result = local.list_directory(
                    &directory_id,
                    &location_id,
                    &worker_cancellation.cancelled,
                    |event| {
                        on_event
                            .send(event)
                            .map_err(|_| ExplorerError::ChannelClosed)
                    },
                );
                let _ = result_sender.send(result);
            });

            if synced_location {
                await_bounded_synced_listing(
                    result_receiver,
                    cancellation.clone(),
                    SYNCED_LISTING_TIMEOUT,
                )
                .await
            } else {
                result_receiver.await.unwrap_or_else(|_| {
                    Err(ExplorerError::Unexpected(
                        "The directory listing task stopped unexpectedly.".to_owned(),
                    ))
                })
            }
        }
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

    let backend = state
        .resolve_location_backend(&location_id)
        .map_err(ExplorerErrorDto::from)?;

    if backend == LocationBackend::Ssh {
        return Ok(metadata_result(
            entry_id,
            None,
            None,
            PreviewUnavailableReason::Remote,
            "Remote content preview is not available yet.",
        ));
    }

    if backend == LocationBackend::Gio {
        let access = state
            .gio
            .resolve_preview_access(&entry_id, &location_id)
            .map_err(|error| local_preview_error(error, true))?;
        if let Some(reason) = access.reason {
            let message = match reason {
                PreviewUnavailableReason::Directory => {
                    "Folders provide metadata rather than a content preview."
                }
                PreviewUnavailableReason::Symlink => {
                    "Explora does not follow symbolic links while previewing."
                }
                _ => "This cloud entry cannot be previewed.",
            };
            return Ok(metadata_result(
                entry_id,
                access.size.map(|size| size.to_string()),
                access.modified_at,
                reason,
                message,
            ));
        }

        let uri = access.uri;
        return state
            .preview
            .prepare_external(
                request_id,
                entry_id,
                ExternalPreviewSource {
                    name: access.name,
                    size: access.size,
                    modified_at: access.modified_at,
                },
                image_mode,
                move |output, read_limit, cancelled| {
                    GioFilesystem::materialize_preview(uri, output, read_limit, cancelled)
                },
            )
            .await
            .map_err(|error| local_preview_error(error, true));
    }

    let synced_location = state
        .local
        .is_synced_folder(&location_id)
        .map_err(ExplorerErrorDto::from)?;

    debug_assert_eq!(backend, LocationBackend::Local);

    // Resolve and revalidate the opaque entry before deciding whether content
    // access is allowed. Filesystem metadata is sufficient for this decision
    // and does not open the file or request provider hydration.
    let access = state
        .local
        .resolve_preview_access(&entry_id, &location_id)
        .map_err(|error| local_preview_error(error, synced_location))?;
    if access.availability != ContentAvailability::Local {
        let request_content = access
            .content_request_policy
            .filter(|policy| {
                content_request_provider_status_error(*policy, access.provider_status).is_none()
            })
            .map(content_request_capability);
        return Ok(metadata_result_with_content_request(
            entry_id,
            access.size,
            access.modified_at,
            PreviewUnavailableReason::DownloadRequired,
            synced_preview_message(
                access.availability,
                access.provider_status,
                access.content_request_policy,
            ),
            request_content,
        ));
    }

    state
        .preview
        .prepare_local(request_id, entry_id, access.path, image_mode)
        .await
        .map_err(|error| local_preview_error(error, synced_location))
}

fn content_request_capability(_policy: ContentRequestPolicy) -> ContentRequestCapabilityDto {
    ContentRequestCapabilityDto {
        intent: ContentRequestIntent::DownloadToPreview,
        // The current adapters hand work to the operating system. Cancelling
        // this task stops Explora from waiting but does not prove the provider
        // stopped its request.
        provider_work_cancellable: false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentRequestPollAction {
    Continue,
    Complete,
    StaleReference,
    AvailabilityError,
    ProviderError,
    TimedOut,
}

fn content_request_poll_action(
    expected_policy: ContentRequestPolicy,
    current_policy: Option<ContentRequestPolicy>,
    availability: ContentAvailability,
    has_provider_error: bool,
    timed_out: bool,
) -> ContentRequestPollAction {
    if current_policy != Some(expected_policy) {
        return ContentRequestPollAction::StaleReference;
    }
    if availability == ContentAvailability::Local {
        return ContentRequestPollAction::Complete;
    }
    if availability == ContentAvailability::Error {
        return ContentRequestPollAction::AvailabilityError;
    }
    if has_provider_error
        && !matches!(
            availability,
            ContentAvailability::Downloading | ContentAvailability::Syncing
        )
    {
        return ContentRequestPollAction::ProviderError;
    }
    if timed_out {
        return ContentRequestPollAction::TimedOut;
    }
    ContentRequestPollAction::Continue
}

fn local_preview_error(error: ExplorerError, synced_location: bool) -> ExplorerErrorDto {
    ExplorerErrorDto::from(if synced_location {
        redact_synced_folder_error(error)
    } else {
        error
    })
}

fn synced_preview_message(
    availability: ContentAvailability,
    provider_status: Option<SyncedFolderStatus>,
    content_request_policy: Option<ContentRequestPolicy>,
) -> &'static str {
    match provider_status {
        Some(SyncedFolderStatus::Offline) => {
            return "The sync provider is offline. This file is not available locally."
        }
        Some(SyncedFolderStatus::Paused) => {
            return "Syncing is paused. This file is not available locally."
        }
        Some(SyncedFolderStatus::Error) => {
            return "The sync provider reported an error. This file is not available locally."
        }
        Some(SyncedFolderStatus::Unknown)
            if content_request_policy != Some(ContentRequestPolicy::ICloud) =>
        {
            return "Explora cannot verify the sync provider's status or access this file locally."
        }
        Some(SyncedFolderStatus::Available | SyncedFolderStatus::Unknown) | None => {}
    }
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

fn content_request_provider_status_error(
    policy: ContentRequestPolicy,
    status: Option<SyncedFolderStatus>,
) -> Option<ExplorerError> {
    match status {
        Some(SyncedFolderStatus::Available) => None,
        Some(SyncedFolderStatus::Offline) => Some(ExplorerError::Offline(
            "The sync provider is offline.".to_owned(),
        )),
        Some(SyncedFolderStatus::Paused) => Some(ExplorerError::Unsupported(
            "Syncing is paused for this provider.".to_owned(),
        )),
        Some(SyncedFolderStatus::Error) => Some(ExplorerError::Unexpected(
            "The sync provider reported an error.".to_owned(),
        )),
        Some(SyncedFolderStatus::Unknown) | None => match policy {
            // macOS has a documented item request but no provider-neutral root
            // status. The request itself is the authoritative operation and
            // returns a structured error if iCloud cannot accept it.
            ContentRequestPolicy::ICloud => None,
            // Windows does expose authoritative CFAPI provider status, so an
            // unknown result must not start placeholder hydration.
            ContentRequestPolicy::WindowsCloudFiles => Some(ExplorerError::Unsupported(
                "Explora cannot verify that this sync provider is available.".to_owned(),
            )),
        },
    }
}

#[tauri::command]
pub async fn request_content(
    state: State<'_, AppState>,
    request_id: String,
    entry_id: String,
    location_id: String,
    on_event: Channel<ContentRequestEventDto>,
) -> Result<(), ExplorerErrorDto> {
    validate_request_id(&request_id).map_err(ExplorerErrorDto::from)?;
    validate_reference_id(&entry_id).map_err(ExplorerErrorDto::from)?;
    validate_reference_id(&location_id).map_err(ExplorerErrorDto::from)?;

    match state
        .resolve_location_backend(&location_id)
        .map_err(ExplorerErrorDto::from)?
    {
        LocationBackend::Local
            if state
                .local
                .is_synced_folder(&location_id)
                .map_err(ExplorerErrorDto::from)? => {}
        LocationBackend::Gio => {
            return Err(ExplorerErrorDto::from(ExplorerError::Unsupported(
                "This synced folder does not support explicit content requests.".to_owned(),
            )))
        }
        LocationBackend::Local | LocationBackend::Ssh => {
            return Err(ExplorerErrorDto::from(ExplorerError::InvalidReference))
        }
    }

    let initial = state
        .local
        .resolve_preview_access(&entry_id, &location_id)
        .map_err(|error| local_preview_error(error, true))?;

    // A stale UI action may race with provider completion. Treat an already
    // local item as an idempotent success without starting more provider work.
    if initial.availability == ContentAvailability::Local {
        send_content_request_event(
            &on_event,
            ContentRequestEventDto::Started {
                provider_work_cancellable: false,
            },
        )?;
        return send_content_request_event(
            &on_event,
            ContentRequestEventDto::Complete {
                availability: ContentAvailability::Local,
            },
        );
    }

    let policy = initial.content_request_policy.ok_or_else(|| {
        ExplorerErrorDto::from(ExplorerError::Unsupported(
            "This synced folder does not support explicit content requests.".to_owned(),
        ))
    })?;
    if let Some(error) = content_request_provider_status_error(policy, initial.provider_status) {
        return Err(ExplorerErrorDto::from(error));
    }

    // Register cancellation before publishing Started so a fast UI stop
    // cannot race ahead of the task registry.
    let mut request = state
        .content_requests
        .begin(request_id, policy, initial.path)
        .map_err(ExplorerErrorDto::from)?;

    send_content_request_event(
        &on_event,
        ContentRequestEventDto::Started {
            provider_work_cancellable: false,
        },
    )?;

    send_content_request_event(
        &on_event,
        ContentRequestEventDto::Progress {
            availability: initial.availability,
        },
    )?;

    let started_at = Instant::now();
    let mut last_availability = initial.availability;
    let mut provider_error = None;

    loop {
        if request.is_cancelled() {
            return Err(ExplorerErrorDto::from(ExplorerError::Cancelled));
        }
        if let Some(Err(error)) = request.take_provider_result() {
            provider_error = Some(error);
        }
        tokio::time::sleep(CONTENT_REQUEST_POLL_INTERVAL).await;

        // Re-resolving the opaque entry catches removal, replacement by a
        // non-file, an offline root, and policy changes before preview reads.
        let current = state
            .local
            .resolve_preview_access(&entry_id, &location_id)
            .map_err(|error| local_preview_error(error, true))?;
        if current.content_request_policy != Some(policy) {
            return Err(ExplorerErrorDto::from(ExplorerError::StaleReference));
        }
        if current.availability != ContentAvailability::Local {
            if let Some(error) =
                content_request_provider_status_error(policy, current.provider_status)
            {
                return Err(local_preview_error(error, true));
            }
        }
        match content_request_poll_action(
            policy,
            current.content_request_policy,
            current.availability,
            provider_error.is_some(),
            started_at.elapsed() >= CONTENT_REQUEST_TIMEOUT,
        ) {
            ContentRequestPollAction::Continue => {
                // A provider request can race with an already active system
                // download. Once authoritative state says work is underway,
                // its redundant start error no longer controls the wait.
                provider_error = None;
            }
            ContentRequestPollAction::Complete => {
                return send_content_request_event(
                    &on_event,
                    ContentRequestEventDto::Complete {
                        availability: ContentAvailability::Local,
                    },
                );
            }
            ContentRequestPollAction::StaleReference => {
                return Err(ExplorerErrorDto::from(ExplorerError::StaleReference));
            }
            ContentRequestPollAction::AvailabilityError => {
                return Err(ExplorerErrorDto::from(ExplorerError::Unexpected(
                    "The operating system reported a download error for this file.".to_owned(),
                )));
            }
            ContentRequestPollAction::ProviderError => {
                let error = match provider_error.take() {
                    Some(error) => error,
                    None => ExplorerError::Unexpected(
                        "The operating system content request failed.".to_owned(),
                    ),
                };
                return Err(local_preview_error(error, true));
            }
            ContentRequestPollAction::TimedOut => {
                return Err(ExplorerErrorDto::from(ExplorerError::TimedOut(
                    "Explora stopped waiting because this download took too long. The operating system may continue downloading it."
                        .to_owned(),
                )));
            }
        }
        if current.availability != last_availability {
            last_availability = current.availability;
            send_content_request_event(
                &on_event,
                ContentRequestEventDto::Progress {
                    availability: current.availability,
                },
            )?;
        }
    }
}

fn send_content_request_event(
    channel: &Channel<ContentRequestEventDto>,
    event: ContentRequestEventDto,
) -> Result<(), ExplorerErrorDto> {
    channel
        .send(event)
        .map_err(|_| ExplorerErrorDto::from(ExplorerError::ChannelClosed))
}

#[tauri::command]
pub fn cancel_content_request(
    state: State<'_, AppState>,
    request_id: String,
) -> Result<(), ExplorerErrorDto> {
    validate_request_id(&request_id).map_err(ExplorerErrorDto::from)?;
    state
        .content_requests
        .cancel(&request_id)
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

    #[tokio::test]
    async fn bounded_synced_listing_returns_promptly_on_cancellation() {
        let (sender, receiver) = oneshot::channel();
        let control = Arc::new(ListingControl::new());
        control.cancel();

        let result = await_bounded_synced_listing(receiver, control, Duration::from_secs(1)).await;

        assert!(matches!(result, Err(ExplorerError::Cancelled)));
        drop(sender);
    }

    #[tokio::test]
    async fn bounded_synced_listing_times_out_and_cancels_late_worker_events() {
        let (_sender, receiver) = oneshot::channel();
        let control = Arc::new(ListingControl::new());

        let result =
            await_bounded_synced_listing(receiver, control.clone(), Duration::from_millis(1)).await;

        assert!(matches!(result, Err(ExplorerError::TimedOut(_))));
        assert!(control.cancelled.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn bounded_synced_listing_preserves_completed_results() {
        let (sender, receiver) = oneshot::channel();
        sender.send(Ok(())).expect("listing result receiver");

        assert!(await_bounded_synced_listing(
            receiver,
            Arc::new(ListingControl::new()),
            Duration::from_secs(1),
        )
        .await
        .is_ok());
    }

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
    fn content_request_contract_is_explicit_and_never_claims_provider_cancellation() {
        let capability = content_request_capability(ContentRequestPolicy::ICloud);
        let capability_json = serde_json::to_value(capability).expect("serialize capability");
        assert_eq!(capability_json["intent"], "downloadToPreview");
        assert_eq!(capability_json["providerWorkCancellable"], false);

        let progress = serde_json::to_value(ContentRequestEventDto::Progress {
            availability: ContentAvailability::Downloading,
        })
        .expect("serialize progress");
        assert_eq!(progress["event"], "progress");
        assert_eq!(progress["availability"], "downloading");

        let timeout = ExplorerErrorDto::from(ExplorerError::TimedOut("slow".to_owned()));
        assert_eq!(timeout.code, crate::filesystem::ExplorerErrorCode::TimedOut);
    }

    #[test]
    fn content_request_polling_prioritizes_revalidation_over_timeout() {
        let policy = ContentRequestPolicy::ICloud;
        assert_eq!(
            content_request_poll_action(
                policy,
                Some(policy),
                ContentAvailability::Local,
                false,
                true,
            ),
            ContentRequestPollAction::Complete
        );
        assert_eq!(
            content_request_poll_action(
                policy,
                Some(ContentRequestPolicy::WindowsCloudFiles),
                ContentAvailability::Local,
                false,
                false,
            ),
            ContentRequestPollAction::StaleReference
        );
        assert_eq!(
            content_request_poll_action(
                policy,
                Some(policy),
                ContentAvailability::OnlineOnly,
                false,
                true,
            ),
            ContentRequestPollAction::TimedOut
        );
    }

    #[test]
    fn content_request_polling_handles_provider_and_availability_errors_honestly() {
        let policy = ContentRequestPolicy::WindowsCloudFiles;
        assert_eq!(
            content_request_poll_action(
                policy,
                Some(policy),
                ContentAvailability::OnlineOnly,
                true,
                false,
            ),
            ContentRequestPollAction::ProviderError
        );
        assert_eq!(
            content_request_poll_action(
                policy,
                Some(policy),
                ContentAvailability::Downloading,
                true,
                false,
            ),
            ContentRequestPollAction::Continue
        );
        assert_eq!(
            content_request_poll_action(
                policy,
                Some(policy),
                ContentAvailability::Error,
                false,
                false,
            ),
            ContentRequestPollAction::AvailabilityError
        );
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
        let available = Some(SyncedFolderStatus::Available);
        assert!(
            synced_preview_message(ContentAvailability::OnlineOnly, available, None)
                .contains("online-only")
        );
        assert!(
            synced_preview_message(ContentAvailability::Downloading, available, None)
                .contains("downloading")
        );
        assert!(
            synced_preview_message(ContentAvailability::Syncing, available, None)
                .contains("not current")
        );
        assert!(
            synced_preview_message(ContentAvailability::Error, available, None)
                .contains("download error")
        );
        assert!(
            synced_preview_message(ContentAvailability::Unknown, available, None)
                .contains("cannot verify")
        );
        assert!(synced_preview_message(
            ContentAvailability::OnlineOnly,
            Some(SyncedFolderStatus::Offline),
            Some(ContentRequestPolicy::ICloud)
        )
        .contains("provider is offline"));
        assert!(synced_preview_message(
            ContentAvailability::OnlineOnly,
            Some(SyncedFolderStatus::Unknown),
            Some(ContentRequestPolicy::ICloud)
        )
        .contains("online-only"));
        assert!(synced_preview_message(
            ContentAvailability::OnlineOnly,
            Some(SyncedFolderStatus::Unknown),
            Some(ContentRequestPolicy::WindowsCloudFiles)
        )
        .contains("cannot verify"));
        assert!(matches!(
            content_request_provider_status_error(
                ContentRequestPolicy::ICloud,
                Some(SyncedFolderStatus::Offline)
            ),
            Some(ExplorerError::Offline(_))
        ));
        assert!(content_request_provider_status_error(
            ContentRequestPolicy::WindowsCloudFiles,
            Some(SyncedFolderStatus::Available)
        )
        .is_none());
        assert!(content_request_provider_status_error(
            ContentRequestPolicy::ICloud,
            Some(SyncedFolderStatus::Unknown)
        )
        .is_none());
        assert!(matches!(
            content_request_provider_status_error(
                ContentRequestPolicy::WindowsCloudFiles,
                Some(SyncedFolderStatus::Unknown)
            ),
            Some(ExplorerError::Unsupported(_))
        ));
        assert!(
            content_request_provider_status_error(ContentRequestPolicy::ICloud, None).is_none()
        );
    }
}
