use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use tauri::{ipc::Channel, State};

use crate::local_filesystem::{
    DirectoryListingEvent, ExplorerError, ExplorerErrorDto, LocalFilesystem, LocationSummaryDto,
};

pub struct AppState {
    filesystem: Arc<LocalFilesystem>,
    listings: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl AppState {
    pub fn new(filesystem: LocalFilesystem) -> Self {
        Self {
            filesystem: Arc::new(filesystem),
            listings: Mutex::new(HashMap::new()),
        }
    }

    fn begin_listing(&self, request_id: &str) -> Result<Arc<AtomicBool>, ExplorerError> {
        if request_id.is_empty() || request_id.len() > 128 {
            return Err(ExplorerError::InvalidReference);
        }

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

#[tauri::command]
pub fn list_local_locations(state: State<'_, AppState>) -> Vec<LocationSummaryDto> {
    state.filesystem.locations()
}

#[tauri::command]
pub async fn list_local_directory(
    state: State<'_, AppState>,
    request_id: String,
    directory_id: String,
    location_id: String,
    on_event: Channel<DirectoryListingEvent>,
) -> Result<(), ExplorerErrorDto> {
    let cancellation = state
        .begin_listing(&request_id)
        .map_err(ExplorerErrorDto::from)?;
    let filesystem = state.filesystem.clone();
    let listing_request_id = request_id.clone();

    let result = tauri::async_runtime::spawn_blocking(move || {
        filesystem.list_directory(&directory_id, &location_id, &cancellation, |event| {
            on_event
                .send(event)
                .map_err(|_| ExplorerError::ChannelClosed)
        })
    })
    .await;

    state.finish_listing(&listing_request_id);

    match result {
        Ok(result) => result.map_err(ExplorerErrorDto::from),
        Err(error) => Err(ExplorerErrorDto {
            code: crate::local_filesystem::ExplorerErrorCode::Unexpected,
            message: format!("The directory listing task failed: {error}"),
        }),
    }
}

#[tauri::command]
pub fn cancel_local_listing(
    state: State<'_, AppState>,
    request_id: String,
) -> Result<(), ExplorerErrorDto> {
    state
        .cancel_listing(&request_id)
        .map_err(ExplorerErrorDto::from)
}
