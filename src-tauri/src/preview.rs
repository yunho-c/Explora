use std::{
    collections::{HashMap, VecDeque},
    ffi::OsStr,
    fs::{self, File},
    io::{BufReader, Cursor, Read},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, UNIX_EPOCH},
};

use encoding_rs::{UTF_16BE, UTF_16LE, WINDOWS_1252};
use image::{
    metadata::Orientation, DynamicImage, GenericImageView, ImageDecoder, ImageFormat, ImageReader,
    Limits,
};
use tokio::sync::{Notify, Semaphore};
use uuid::Uuid;

use crate::filesystem::{
    ExplorerError, ImagePreviewMode, PreviewContentDto, PreviewResultDto, PreviewUnavailableReason,
};

pub const MAX_TEXT_BYTES: usize = 256 * 1024;
pub const MAX_IMAGE_FILE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_IMAGE_DIMENSION: u32 = 16_384;
pub const MAX_IMAGE_PIXELS: u64 = 40_000_000;
pub const MAX_IMAGE_ALLOCATION_BYTES: u64 = 192 * 1024 * 1024;
pub const MAX_PREVIEW_IMAGE_DIMENSION: u32 = 1_920;
pub const MAX_IMAGE_PREVIEW_RESOURCE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PDF_FILE_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_PREVIEW_RESOURCE_BYTES: usize = MAX_PDF_FILE_BYTES as usize;
pub const MAX_DIRECT_IMAGE_FILE_BYTES: u64 = MAX_IMAGE_PREVIEW_RESOURCE_BYTES as u64;
pub const MAX_PREVIEW_RESOURCE_COUNT: usize = 4;
pub const MAX_PREVIEW_RESOURCE_TOTAL_BYTES: usize = 64 * 1024 * 1024;
pub const PREVIEW_RESOURCE_TTL: Duration = Duration::from_secs(5 * 60);
pub const PREVIEW_TIMEOUT: Duration = Duration::from_secs(5);
pub const MAX_CONCURRENT_PREVIEWS: usize = 2;

#[derive(Debug)]
struct PreviewCancellation {
    cancelled: AtomicBool,
    notify: Notify,
}

impl PreviewCancellation {
    fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            notify: Notify::new(),
        }
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.notify.notify_waiters();
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    async fn cancelled(&self) {
        loop {
            let notified = self.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

#[derive(Debug)]
struct PreviewResource {
    id: String,
    bytes: Vec<u8>,
    created_at: Instant,
}

#[derive(Debug, Default)]
struct PreviewResourceStore {
    resources: VecDeque<PreviewResource>,
    total_bytes: usize,
}

impl PreviewResourceStore {
    fn insert(&mut self, bytes: Vec<u8>) -> Result<String, ExplorerError> {
        self.insert_at(bytes, Instant::now())
    }

    fn insert_at(&mut self, bytes: Vec<u8>, now: Instant) -> Result<String, ExplorerError> {
        if bytes.len() > MAX_PREVIEW_RESOURCE_BYTES {
            return Err(ExplorerError::Unsupported(
                "The prepared file is too large to preview.".to_owned(),
            ));
        }

        self.prune_expired(now);
        while self.resources.len() >= MAX_PREVIEW_RESOURCE_COUNT
            || self.total_bytes.saturating_add(bytes.len()) > MAX_PREVIEW_RESOURCE_TOTAL_BYTES
        {
            if let Some(resource) = self.resources.pop_front() {
                self.total_bytes = self.total_bytes.saturating_sub(resource.bytes.len());
            } else {
                break;
            }
        }

        let id = Uuid::new_v4().to_string();
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());
        self.resources.push_back(PreviewResource {
            id: id.clone(),
            bytes,
            created_at: now,
        });
        Ok(id)
    }

    fn take(&mut self, resource_id: &str) -> Option<Vec<u8>> {
        self.take_at(resource_id, Instant::now())
    }

    fn take_at(&mut self, resource_id: &str, now: Instant) -> Option<Vec<u8>> {
        self.prune_expired(now);
        let index = self
            .resources
            .iter()
            .position(|resource| resource.id == resource_id)?;
        let resource = self.resources.remove(index)?;
        self.total_bytes = self.total_bytes.saturating_sub(resource.bytes.len());
        Some(resource.bytes)
    }

    fn discard(&mut self, resource_id: &str) {
        if let Some(index) = self
            .resources
            .iter()
            .position(|resource| resource.id == resource_id)
        {
            if let Some(resource) = self.resources.remove(index) {
                self.total_bytes = self.total_bytes.saturating_sub(resource.bytes.len());
            }
        }
    }

    fn prune_expired(&mut self, now: Instant) {
        while self.resources.front().is_some_and(|resource| {
            now.saturating_duration_since(resource.created_at) >= PREVIEW_RESOURCE_TTL
        }) {
            if let Some(resource) = self.resources.pop_front() {
                self.total_bytes = self.total_bytes.saturating_sub(resource.bytes.len());
            }
        }
    }
}

#[derive(Debug)]
pub struct PreviewManager {
    requests: Mutex<HashMap<String, Arc<PreviewCancellation>>>,
    resources: Mutex<PreviewResourceStore>,
    workers: Arc<Semaphore>,
}

impl Default for PreviewManager {
    fn default() -> Self {
        Self {
            requests: Mutex::new(HashMap::new()),
            resources: Mutex::new(PreviewResourceStore::default()),
            workers: Arc::new(Semaphore::new(MAX_CONCURRENT_PREVIEWS)),
        }
    }
}

impl PreviewManager {
    pub async fn prepare_local(
        &self,
        request_id: String,
        entry_id: String,
        path: PathBuf,
        image_mode: ImagePreviewMode,
    ) -> Result<PreviewResultDto, ExplorerError> {
        let cancellation = self.begin_request(&request_id)?;
        let result = self
            .prepare_local_inner(entry_id, path, cancellation, image_mode)
            .await;
        self.finish_request(&request_id);
        result
    }

    async fn prepare_local_inner(
        &self,
        entry_id: String,
        path: PathBuf,
        cancellation: Arc<PreviewCancellation>,
        image_mode: ImagePreviewMode,
    ) -> Result<PreviewResultDto, ExplorerError> {
        let permit = tokio::select! {
            permit = self.workers.clone().acquire_owned() => {
                permit.map_err(|_| ExplorerError::StateUnavailable)?
            }
            () = cancellation.cancelled() => return Err(ExplorerError::Cancelled),
        };

        let task_cancellation = cancellation.clone();
        let task = tauri::async_runtime::spawn_blocking(move || {
            let _permit = permit;
            prepare_local_file(&path, &task_cancellation, image_mode)
        });

        let prepared = tokio::select! {
            result = tokio::time::timeout(PREVIEW_TIMEOUT, task) => {
                match result {
                    Ok(joined) => joined.map_err(|error| {
                        ExplorerError::Unexpected(format!("The preview task failed: {error}"))
                    })??,
                    Err(_) => {
                        cancellation.cancel();
                        return Ok(metadata_result(
                            entry_id,
                            None,
                            None,
                            PreviewUnavailableReason::TimedOut,
                            "This file took too long to preview safely.",
                        ));
                    }
                }
            }
            () = cancellation.cancelled() => return Err(ExplorerError::Cancelled),
        };

        if cancellation.is_cancelled() {
            return Err(ExplorerError::Cancelled);
        }

        match prepared.content {
            PreparedContent::Metadata { reason, message } => Ok(metadata_result(
                entry_id,
                prepared.size,
                prepared.modified_at,
                reason,
                message,
            )),
            PreparedContent::Text {
                text,
                truncated,
                encoding,
            } => Ok(PreviewResultDto {
                entry_id,
                size: prepared.size,
                modified_at: prepared.modified_at,
                content: PreviewContentDto::Text {
                    text,
                    truncated,
                    encoding,
                },
            }),
            PreparedContent::Image {
                bytes,
                media_type,
                image_mode,
                width,
                height,
                original_width,
                original_height,
            } => {
                let resource_id = self
                    .resources
                    .lock()
                    .map_err(|_| ExplorerError::StateUnavailable)?
                    .insert(bytes)?;
                Ok(PreviewResultDto {
                    entry_id,
                    size: prepared.size,
                    modified_at: prepared.modified_at,
                    content: PreviewContentDto::Image {
                        resource_id,
                        media_type,
                        image_mode,
                        width,
                        height,
                        original_width,
                        original_height,
                    },
                })
            }
            PreparedContent::Pdf { bytes } => {
                let resource_id = self
                    .resources
                    .lock()
                    .map_err(|_| ExplorerError::StateUnavailable)?
                    .insert(bytes)?;
                Ok(PreviewResultDto {
                    entry_id,
                    size: prepared.size,
                    modified_at: prepared.modified_at,
                    content: PreviewContentDto::Pdf {
                        resource_id,
                        media_type: "application/pdf",
                    },
                })
            }
        }
    }

    pub fn cancel(&self, request_id: &str) -> Result<(), ExplorerError> {
        if let Some(cancellation) = self
            .requests
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .get(request_id)
        {
            cancellation.cancel();
        }
        Ok(())
    }

    pub fn take_resource(&self, resource_id: &str) -> Result<Vec<u8>, ExplorerError> {
        self.resources
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .take(resource_id)
            .ok_or(ExplorerError::InvalidReference)
    }

    pub fn discard_resource(&self, resource_id: &str) -> Result<(), ExplorerError> {
        self.resources
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .discard(resource_id);
        Ok(())
    }

    fn begin_request(&self, request_id: &str) -> Result<Arc<PreviewCancellation>, ExplorerError> {
        let cancellation = Arc::new(PreviewCancellation::new());
        let mut requests = self
            .requests
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        if requests.contains_key(request_id) {
            return Err(ExplorerError::InvalidReference);
        }
        requests.insert(request_id.to_owned(), cancellation.clone());
        Ok(cancellation)
    }

    fn finish_request(&self, request_id: &str) {
        if let Ok(mut requests) = self.requests.lock() {
            requests.remove(request_id);
        }
    }
}

#[derive(Debug)]
struct PreparedPreview {
    size: Option<String>,
    modified_at: Option<u64>,
    content: PreparedContent,
}

#[derive(Debug)]
enum PreparedContent {
    Metadata {
        reason: PreviewUnavailableReason,
        message: &'static str,
    },
    Text {
        text: String,
        truncated: bool,
        encoding: &'static str,
    },
    Image {
        bytes: Vec<u8>,
        media_type: &'static str,
        image_mode: ImagePreviewMode,
        width: u32,
        height: u32,
        original_width: u32,
        original_height: u32,
    },
    Pdf {
        bytes: Vec<u8>,
    },
}

fn prepare_local_file(
    path: &Path,
    cancellation: &PreviewCancellation,
    image_mode: ImagePreviewMode,
) -> Result<PreparedPreview, ExplorerError> {
    ensure_not_cancelled(cancellation)?;
    let metadata =
        fs::symlink_metadata(path).map_err(|error| ExplorerError::io("inspect", path, error))?;
    let size = Some(metadata.len().to_string());
    let modified_at = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| u64::try_from(duration.as_millis()).ok());

    let metadata_only = |reason, message| PreparedPreview {
        size: size.clone(),
        modified_at,
        content: PreparedContent::Metadata { reason, message },
    };

    if metadata.file_type().is_symlink() {
        return Ok(metadata_only(
            PreviewUnavailableReason::Symlink,
            "Explora does not follow symbolic links while previewing.",
        ));
    }
    if metadata.is_dir() {
        return Ok(metadata_only(
            PreviewUnavailableReason::Directory,
            "Folders provide metadata rather than a content preview.",
        ));
    }
    if !metadata.is_file() {
        return Ok(metadata_only(
            PreviewUnavailableReason::Unsupported,
            "This filesystem entry cannot be previewed.",
        ));
    }

    let extension = path
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase);
    if extension.as_deref().is_some_and(is_image_extension) {
        if metadata.len() > MAX_IMAGE_FILE_BYTES {
            return Ok(metadata_only(
                PreviewUnavailableReason::TooLarge,
                "This image is too large to preview safely.",
            ));
        }
        return prepare_image(
            path,
            metadata.len(),
            size,
            modified_at,
            cancellation,
            image_mode,
        );
    }
    if extension.as_deref() == Some("svg") {
        return Ok(metadata_only(
            PreviewUnavailableReason::Unsupported,
            "SVG preview requires a separately isolated renderer.",
        ));
    }
    if extension.as_deref() == Some("pdf") {
        return prepare_pdf(path, metadata.len(), size, modified_at, cancellation);
    }
    if extension.as_deref().is_some_and(is_known_binary_extension) {
        return Ok(metadata_only(
            PreviewUnavailableReason::Unsupported,
            "Content preview is not available for this file type yet.",
        ));
    }

    let mut file = File::open(path).map_err(|error| ExplorerError::io("open", path, error))?;
    let mut bytes = Vec::with_capacity(MAX_TEXT_BYTES.saturating_add(4));
    file.by_ref()
        .take((MAX_TEXT_BYTES + 4) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| ExplorerError::io("read", path, error))?;
    ensure_not_cancelled(cancellation)?;
    let truncated = metadata.len() > MAX_TEXT_BYTES as u64;
    if bytes.len() > MAX_TEXT_BYTES {
        bytes.truncate(MAX_TEXT_BYTES);
    }

    let Some((text, encoding)) = decode_text(&bytes, truncated) else {
        return Ok(metadata_only(
            PreviewUnavailableReason::Binary,
            "This file appears to contain binary data.",
        ));
    };

    Ok(PreparedPreview {
        size,
        modified_at,
        content: PreparedContent::Text {
            text,
            truncated,
            encoding,
        },
    })
}

fn prepare_pdf(
    path: &Path,
    file_size: u64,
    size: Option<String>,
    modified_at: Option<u64>,
    cancellation: &PreviewCancellation,
) -> Result<PreparedPreview, ExplorerError> {
    let unavailable = |reason, message| PreparedPreview {
        size: size.clone(),
        modified_at,
        content: PreparedContent::Metadata { reason, message },
    };

    if file_size > MAX_PDF_FILE_BYTES {
        return Ok(unavailable(
            PreviewUnavailableReason::TooLarge,
            "PDF is too large to preview.",
        ));
    }

    ensure_not_cancelled(cancellation)?;
    let file = File::open(path).map_err(|error| ExplorerError::io("open", path, error))?;
    let capacity = usize::try_from(file_size)
        .unwrap_or(MAX_PREVIEW_RESOURCE_BYTES)
        .min(MAX_PREVIEW_RESOURCE_BYTES);
    let mut bytes = Vec::with_capacity(capacity);
    file.take(MAX_PDF_FILE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| ExplorerError::io("read", path, error))?;
    ensure_not_cancelled(cancellation)?;

    if bytes.len() > MAX_PREVIEW_RESOURCE_BYTES {
        return Ok(unavailable(
            PreviewUnavailableReason::TooLarge,
            "PDF is too large to preview.",
        ));
    }
    if !bytes.starts_with(b"%PDF-") {
        return Ok(unavailable(
            PreviewUnavailableReason::Malformed,
            "This PDF couldn't be displayed.",
        ));
    }

    Ok(PreparedPreview {
        size,
        modified_at,
        content: PreparedContent::Pdf { bytes },
    })
}

fn prepare_image(
    path: &Path,
    file_size: u64,
    size: Option<String>,
    modified_at: Option<u64>,
    cancellation: &PreviewCancellation,
    image_mode: ImagePreviewMode,
) -> Result<PreparedPreview, ExplorerError> {
    match image_mode {
        ImagePreviewMode::Direct => {
            prepare_direct_image(path, file_size, size, modified_at, cancellation)
        }
        ImagePreviewMode::Sanitized => {
            prepare_sanitized_image(path, size, modified_at, cancellation)
        }
    }
}

fn prepare_direct_image(
    path: &Path,
    file_size: u64,
    size: Option<String>,
    modified_at: Option<u64>,
    cancellation: &PreviewCancellation,
) -> Result<PreparedPreview, ExplorerError> {
    let unavailable = |reason, message| PreparedPreview {
        size: size.clone(),
        modified_at,
        content: PreparedContent::Metadata { reason, message },
    };

    if file_size > MAX_DIRECT_IMAGE_FILE_BYTES {
        return Ok(unavailable(
            PreviewUnavailableReason::TooLarge,
            "This image is too large for direct preview. Enable sanitized image preview to resize it first.",
        ));
    }

    ensure_not_cancelled(cancellation)?;
    let file = File::open(path).map_err(|error| ExplorerError::io("open", path, error))?;
    let capacity = usize::try_from(file_size)
        .unwrap_or(MAX_IMAGE_PREVIEW_RESOURCE_BYTES)
        .min(MAX_IMAGE_PREVIEW_RESOURCE_BYTES);
    let mut bytes = Vec::with_capacity(capacity);
    file.take(MAX_DIRECT_IMAGE_FILE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| ExplorerError::io("read", path, error))?;
    if bytes.len() > MAX_IMAGE_PREVIEW_RESOURCE_BYTES {
        return Ok(unavailable(
            PreviewUnavailableReason::TooLarge,
            "This image is too large for direct preview. Enable sanitized image preview to resize it first.",
        ));
    }
    ensure_not_cancelled(cancellation)?;

    let mut reader = match ImageReader::new(Cursor::new(bytes.as_slice())).with_guessed_format() {
        Ok(reader) => reader,
        Err(_) => {
            return Ok(unavailable(
                PreviewUnavailableReason::Malformed,
                "This image is unsupported or damaged.",
            ));
        }
    };
    let Some(format) = reader.format() else {
        return Ok(unavailable(
            PreviewUnavailableReason::Malformed,
            "This image is unsupported or damaged.",
        ));
    };
    let Some(media_type) = direct_media_type(format) else {
        return Ok(unavailable(
            PreviewUnavailableReason::Unsupported,
            "This format requires sanitized image preview.",
        ));
    };
    if direct_image_requires_sanitizing(format, &bytes) {
        return Ok(unavailable(
            PreviewUnavailableReason::Unsupported,
            "This image requires sanitized image preview.",
        ));
    }

    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_IMAGE_ALLOCATION_BYTES);
    reader.limits(limits);
    let mut decoder = match reader.into_decoder() {
        Ok(decoder) => decoder,
        Err(image::ImageError::Limits(_)) => {
            return Ok(unavailable(
                PreviewUnavailableReason::TooLarge,
                "This image exceeds Explora's safe preview limits.",
            ));
        }
        Err(_) => {
            return Ok(unavailable(
                PreviewUnavailableReason::Malformed,
                "This image is unsupported or damaged.",
            ));
        }
    };
    let (original_width, original_height) = decoder.dimensions();
    if !image_dimensions_are_safe(original_width, original_height) {
        return Ok(unavailable(
            PreviewUnavailableReason::TooLarge,
            "This image exceeds Explora's safe preview limits.",
        ));
    }
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let (width, height) = oriented_dimensions(original_width, original_height, orientation);
    drop(decoder);
    ensure_not_cancelled(cancellation)?;

    Ok(PreparedPreview {
        size,
        modified_at,
        content: PreparedContent::Image {
            bytes,
            media_type,
            image_mode: ImagePreviewMode::Direct,
            width,
            height,
            original_width,
            original_height,
        },
    })
}

fn prepare_sanitized_image(
    path: &Path,
    size: Option<String>,
    modified_at: Option<u64>,
    cancellation: &PreviewCancellation,
) -> Result<PreparedPreview, ExplorerError> {
    let malformed = || PreparedPreview {
        size: size.clone(),
        modified_at,
        content: PreparedContent::Metadata {
            reason: PreviewUnavailableReason::Malformed,
            message: "This image is unsupported or damaged.",
        },
    };
    let too_large = || PreparedPreview {
        size: size.clone(),
        modified_at,
        content: PreparedContent::Metadata {
            reason: PreviewUnavailableReason::TooLarge,
            message: "This image exceeds Explora's safe preview limits.",
        },
    };

    ensure_not_cancelled(cancellation)?;
    let file = File::open(path).map_err(|error| ExplorerError::io("open", path, error))?;
    let mut reader = match ImageReader::new(BufReader::new(file)).with_guessed_format() {
        Ok(reader) => reader,
        Err(_) => return Ok(malformed()),
    };
    let Some(format) = reader.format() else {
        return Ok(malformed());
    };
    if !matches!(
        format,
        ImageFormat::Png
            | ImageFormat::Jpeg
            | ImageFormat::Gif
            | ImageFormat::WebP
            | ImageFormat::Bmp
            | ImageFormat::Tiff
    ) {
        return Ok(malformed());
    }

    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_IMAGE_ALLOCATION_BYTES);
    reader.limits(limits);
    let mut decoder = match reader.into_decoder() {
        Ok(decoder) => decoder,
        Err(image::ImageError::Limits(_)) => return Ok(too_large()),
        Err(_) => return Ok(malformed()),
    };
    let (original_width, original_height) = decoder.dimensions();
    if !image_dimensions_are_safe(original_width, original_height) {
        return Ok(too_large());
    }
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    ensure_not_cancelled(cancellation)?;
    let mut image = match DynamicImage::from_decoder(decoder) {
        Ok(image) => image,
        Err(image::ImageError::Limits(_)) => return Ok(too_large()),
        Err(_) => return Ok(malformed()),
    };
    ensure_not_cancelled(cancellation)?;
    image.apply_orientation(orientation);
    let thumbnail = image.thumbnail(MAX_PREVIEW_IMAGE_DIMENSION, MAX_PREVIEW_IMAGE_DIMENSION);
    let (width, height) = thumbnail.dimensions();
    ensure_not_cancelled(cancellation)?;
    let mut encoded = Cursor::new(Vec::new());
    if thumbnail.write_to(&mut encoded, ImageFormat::Png).is_err() {
        return Ok(malformed());
    }
    let bytes = encoded.into_inner();
    if bytes.len() > MAX_IMAGE_PREVIEW_RESOURCE_BYTES {
        return Ok(too_large());
    }
    ensure_not_cancelled(cancellation)?;

    Ok(PreparedPreview {
        size,
        modified_at,
        content: PreparedContent::Image {
            bytes,
            media_type: "image/png",
            image_mode: ImagePreviewMode::Sanitized,
            width,
            height,
            original_width,
            original_height,
        },
    })
}

fn direct_media_type(format: ImageFormat) -> Option<&'static str> {
    match format {
        ImageFormat::Bmp => Some("image/bmp"),
        ImageFormat::Jpeg => Some("image/jpeg"),
        ImageFormat::Png => Some("image/png"),
        ImageFormat::WebP => Some("image/webp"),
        _ => None,
    }
}

fn direct_image_requires_sanitizing(format: ImageFormat, bytes: &[u8]) -> bool {
    match format {
        ImageFormat::Png => png_is_animated(bytes),
        ImageFormat::WebP => webp_is_animated(bytes),
        _ => false,
    }
}

fn png_is_animated(bytes: &[u8]) -> bool {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if !bytes.starts_with(SIGNATURE) {
        return true;
    }

    let mut offset = SIGNATURE.len();
    loop {
        let Some(header_end) = offset.checked_add(8) else {
            return true;
        };
        if header_end > bytes.len() {
            return true;
        }
        let length = u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]) as usize;
        let Some(chunk_end) = header_end
            .checked_add(length)
            .and_then(|data_end| data_end.checked_add(4))
        else {
            return true;
        };
        if chunk_end > bytes.len() {
            return true;
        }
        let chunk_type = &bytes[offset + 4..header_end];
        if chunk_type == b"acTL" {
            return true;
        }
        if chunk_type == b"IDAT" || chunk_type == b"IEND" {
            return false;
        }
        offset = chunk_end;
    }
}

fn webp_is_animated(bytes: &[u8]) -> bool {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return true;
    }

    let mut offset = 12usize;
    while offset.saturating_add(8) <= bytes.len() {
        let chunk_type = &bytes[offset..offset + 4];
        let chunk_size = u32::from_le_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]) as usize;
        let data_start = offset + 8;
        let Some(data_end) = data_start.checked_add(chunk_size) else {
            return true;
        };
        if data_end > bytes.len() {
            return true;
        }
        if chunk_type == b"ANIM" || chunk_type == b"ANMF" {
            return true;
        }
        if chunk_type == b"VP8X" && chunk_size > 0 && bytes[data_start] & 0x02 != 0 {
            return true;
        }
        let Some(next_offset) = data_end.checked_add(chunk_size % 2) else {
            return true;
        };
        offset = next_offset;
    }
    false
}

fn oriented_dimensions(width: u32, height: u32, orientation: Orientation) -> (u32, u32) {
    if matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    ) {
        (height, width)
    } else {
        (width, height)
    }
}

fn image_dimensions_are_safe(width: u32, height: u32) -> bool {
    width > 0
        && height > 0
        && width <= MAX_IMAGE_DIMENSION
        && height <= MAX_IMAGE_DIMENSION
        && u64::from(width).saturating_mul(u64::from(height)) <= MAX_IMAGE_PIXELS
}

fn decode_text(bytes: &[u8], truncated: bool) -> Option<(String, &'static str)> {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let body = even_prefix(&bytes[2..]);
        let (decoded, _) = UTF_16LE.decode_without_bom_handling(body);
        return Some((sanitize_text(decoded.as_ref()), "UTF-16 LE"));
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let body = even_prefix(&bytes[2..]);
        let (decoded, _) = UTF_16BE.decode_without_bom_handling(body);
        return Some((sanitize_text(decoded.as_ref()), "UTF-16 BE"));
    }
    if looks_binary(bytes) {
        return None;
    }

    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => Some((sanitize_text(text), "UTF-8")),
        Err(error) if truncated && error.error_len().is_none() => {
            let text = std::str::from_utf8(&bytes[..error.valid_up_to()]).ok()?;
            Some((sanitize_text(text), "UTF-8"))
        }
        Err(_) => {
            let (decoded, _) = WINDOWS_1252.decode_without_bom_handling(bytes);
            Some((sanitize_text(decoded.as_ref()), "Windows-1252"))
        }
    }
}

fn even_prefix(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes.len() - (bytes.len() % 2)]
}

fn looks_binary(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(8 * 1024)];
    if sample.contains(&0) {
        return true;
    }
    if sample.is_empty() {
        return false;
    }
    let controls = sample
        .iter()
        .filter(|byte| **byte < 0x20 && !matches!(**byte, b'\n' | b'\r' | b'\t' | 0x0C))
        .count();
    controls.saturating_mul(10) > sample.len()
}

fn sanitize_text(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() && !matches!(character, '\n' | '\r' | '\t') {
                '\u{FFFD}'
            } else {
                character
            }
        })
        .collect()
}

fn is_image_extension(extension: &str) -> bool {
    matches!(
        extension,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff"
    )
}

fn is_known_binary_extension(extension: &str) -> bool {
    matches!(
        extension,
        "rtf"
            | "doc"
            | "docx"
            | "odt"
            | "pages"
            | "mp3"
            | "m4a"
            | "aac"
            | "wav"
            | "flac"
            | "ogg"
            | "mp4"
            | "m4v"
            | "mov"
            | "mkv"
            | "webm"
            | "avi"
            | "zip"
            | "tar"
            | "gz"
            | "bz2"
            | "xz"
            | "zst"
            | "7z"
            | "rar"
    )
}

fn ensure_not_cancelled(cancellation: &PreviewCancellation) -> Result<(), ExplorerError> {
    if cancellation.is_cancelled() {
        Err(ExplorerError::Cancelled)
    } else {
        Ok(())
    }
}

pub fn metadata_result(
    entry_id: String,
    size: Option<String>,
    modified_at: Option<u64>,
    reason: PreviewUnavailableReason,
    message: impl Into<String>,
) -> PreviewResultDto {
    PreviewResultDto {
        entry_id,
        size,
        modified_at,
        content: PreviewContentDto::Metadata {
            reason,
            message: message.into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Write};

    use image::{DynamicImage, Rgba, RgbaImage};
    use tempfile::TempDir;

    use super::*;

    fn prepare(path: &Path) -> PreparedPreview {
        prepare_local_file(
            path,
            &PreviewCancellation::new(),
            ImagePreviewMode::Sanitized,
        )
        .expect("prepare preview")
    }

    fn prepare_direct(path: &Path) -> PreparedPreview {
        prepare_local_file(path, &PreviewCancellation::new(), ImagePreviewMode::Direct)
            .expect("prepare direct preview")
    }

    #[test]
    fn decodes_supported_text_encodings_and_sanitizes_controls() {
        assert_eq!(
            decode_text(b"hello\nworld", false),
            Some(("hello\nworld".to_owned(), "UTF-8"))
        );
        assert_eq!(
            decode_text(&[0xFF, 0xFE, b'h', 0, b'i', 0], false),
            Some(("hi".to_owned(), "UTF-16 LE"))
        );
        assert_eq!(
            decode_text(&[b'c', b'a', b'f', 0xE9], false),
            Some(("café".to_owned(), "Windows-1252"))
        );
        assert_eq!(
            decode_text(b"hello\x01world", false),
            Some(("hello�world".to_owned(), "UTF-8"))
        );
    }

    #[test]
    fn rejects_binary_data() {
        assert_eq!(decode_text(b"hello\0world", false), None);
        assert_eq!(decode_text(&[1, 2, 3, 4, 5, b'a'], false), None);
    }

    #[test]
    fn reads_only_a_bounded_text_prefix() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("large.txt");
        File::create(&path)
            .expect("create text")
            .write_all(&vec![b'a'; MAX_TEXT_BYTES + 64])
            .expect("write text");

        let preview = prepare(&path);
        let PreparedContent::Text {
            text, truncated, ..
        } = preview.content
        else {
            panic!("expected text preview");
        };
        assert!(truncated);
        assert_eq!(text.len(), MAX_TEXT_BYTES);
    }

    #[test]
    fn decodes_and_bounds_a_raster_image() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("photo.png");
        let source = RgbaImage::from_pixel(2_400, 1_200, Rgba([34, 91, 143, 255]));
        DynamicImage::ImageRgba8(source)
            .save(&path)
            .expect("write source image");

        let preview = prepare(&path);
        let PreparedContent::Image {
            bytes,
            media_type,
            image_mode,
            width,
            height,
            original_width,
            original_height,
        } = preview.content
        else {
            panic!("expected image preview");
        };
        assert_eq!((original_width, original_height), (2_400, 1_200));
        assert_eq!((width, height), (1_920, 960));
        assert_eq!(media_type, "image/png");
        assert_eq!(image_mode, ImagePreviewMode::Sanitized);
        assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']));
        assert!(bytes.len() <= MAX_IMAGE_PREVIEW_RESOURCE_BYTES);
    }

    #[test]
    fn direct_preview_returns_validated_original_bytes() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("photo.jpg");
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(640, 480, Rgba([34, 91, 143, 255])))
            .save(&path)
            .expect("write source image");
        let original = fs::read(&path).expect("read source image");

        let preview = prepare_direct(&path);
        let PreparedContent::Image {
            bytes,
            media_type,
            image_mode,
            width,
            height,
            original_width,
            original_height,
        } = preview.content
        else {
            panic!("expected direct image preview");
        };

        assert_eq!(bytes, original);
        assert_eq!(media_type, "image/jpeg");
        assert_eq!(image_mode, ImagePreviewMode::Direct);
        assert_eq!((width, height), (640, 480));
        assert_eq!((original_width, original_height), (640, 480));
    }

    #[test]
    fn pdf_preview_returns_bounded_original_bytes() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("brief.pdf");
        let source = b"%PDF-1.7\n1 0 obj\n<<>>\nendobj\n%%EOF\n";
        fs::write(&path, source).expect("write PDF");

        let preview = prepare_direct(&path);
        let PreparedContent::Pdf { bytes } = preview.content else {
            panic!("expected PDF preview");
        };
        assert_eq!(bytes, source);
    }

    #[test]
    fn pdf_preview_rejects_malformed_and_oversized_files() {
        let temp = TempDir::new().expect("temporary directory");
        let malformed = temp.path().join("broken.pdf");
        fs::write(&malformed, b"not a PDF").expect("write malformed PDF");
        assert!(matches!(
            prepare_direct(&malformed).content,
            PreparedContent::Metadata {
                reason: PreviewUnavailableReason::Malformed,
                ..
            }
        ));

        let oversized = temp.path().join("oversized.pdf");
        File::create(&oversized)
            .expect("create sparse PDF")
            .set_len(MAX_PDF_FILE_BYTES + 1)
            .expect("size sparse PDF");
        assert!(matches!(
            prepare_direct(&oversized).content,
            PreparedContent::Metadata {
                reason: PreviewUnavailableReason::TooLarge,
                ..
            }
        ));
    }

    #[test]
    fn direct_preview_rejects_formats_that_require_sanitizing() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("animation.gif");
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 6, Rgba([34, 91, 143, 255])))
            .save(&path)
            .expect("write source image");

        assert!(matches!(
            prepare_direct(&path).content,
            PreparedContent::Metadata {
                reason: PreviewUnavailableReason::Unsupported,
                ..
            }
        ));
    }

    #[test]
    fn direct_animation_detection_rejects_bounded_container_signals() {
        let mut animated_png = b"\x89PNG\r\n\x1a\n".to_vec();
        animated_png.extend_from_slice(&0u32.to_be_bytes());
        animated_png.extend_from_slice(b"acTL");
        animated_png.extend_from_slice(&0u32.to_be_bytes());
        assert!(png_is_animated(&animated_png));

        let mut static_png = b"\x89PNG\r\n\x1a\n".to_vec();
        static_png.extend_from_slice(&0u32.to_be_bytes());
        static_png.extend_from_slice(b"IDAT");
        static_png.extend_from_slice(&0u32.to_be_bytes());
        assert!(!png_is_animated(&static_png));

        let animated_webp = b"RIFF\x0e\0\0\0WEBPVP8X\x01\0\0\0\x02\0";
        assert!(webp_is_animated(animated_webp));
        assert!(webp_is_animated(b"malformed"));
    }

    #[test]
    fn supports_each_enabled_raster_family() {
        let temp = TempDir::new().expect("temporary directory");
        let source =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 6, Rgba([34, 91, 143, 255])));

        for (extension, format) in [
            ("png", ImageFormat::Png),
            ("jpg", ImageFormat::Jpeg),
            ("gif", ImageFormat::Gif),
            ("webp", ImageFormat::WebP),
            ("bmp", ImageFormat::Bmp),
            ("tiff", ImageFormat::Tiff),
        ] {
            let path = temp.path().join(format!("sample.{extension}"));
            let mut encoded = Cursor::new(Vec::new());
            source
                .write_to(&mut encoded, format)
                .unwrap_or_else(|error| panic!("encode {extension}: {error}"));
            fs::write(&path, encoded.into_inner()).expect("write image fixture");

            assert!(
                matches!(prepare(&path).content, PreparedContent::Image { .. }),
                "{extension} should produce an image preview"
            );
        }
    }

    #[test]
    fn image_limits_reject_oversized_inputs_and_dimensions() {
        assert!(image_dimensions_are_safe(8_000, 5_000));
        assert!(!image_dimensions_are_safe(8_001, 5_000));
        assert!(!image_dimensions_are_safe(MAX_IMAGE_DIMENSION + 1, 1));

        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("oversized.png");
        let file = File::create(&path).expect("create sparse image");
        file.set_len(MAX_IMAGE_FILE_BYTES + 1)
            .expect("size sparse image");

        assert!(matches!(
            prepare(&path).content,
            PreparedContent::Metadata {
                reason: PreviewUnavailableReason::TooLarge,
                ..
            }
        ));

        let direct_path = temp.path().join("direct-too-large.png");
        let direct_file = File::create(&direct_path).expect("create direct sparse image");
        direct_file
            .set_len(MAX_DIRECT_IMAGE_FILE_BYTES + 1)
            .expect("size direct sparse image");
        assert!(matches!(
            prepare_direct(&direct_path).content,
            PreparedContent::Metadata {
                reason: PreviewUnavailableReason::TooLarge,
                ..
            }
        ));
    }

    #[test]
    fn rejects_malformed_images_without_exposing_decoder_errors() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("broken.png");
        fs::write(&path, b"not an image").expect("write malformed image");

        let preview = prepare(&path);
        assert!(matches!(
            preview.content,
            PreparedContent::Metadata {
                reason: PreviewUnavailableReason::Malformed,
                ..
            }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn never_follows_symlinks_for_preview() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temporary directory");
        let target = temp.path().join("target.txt");
        let link = temp.path().join("link.txt");
        fs::write(&target, b"secret").expect("write target");
        symlink(&target, &link).expect("create symlink");

        let preview = prepare(&link);
        assert!(matches!(
            preview.content,
            PreparedContent::Metadata {
                reason: PreviewUnavailableReason::Symlink,
                ..
            }
        ));
    }

    #[test]
    fn cancellation_is_checked_before_reading() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("notes.txt");
        fs::write(&path, b"hello").expect("write text");
        let cancellation = PreviewCancellation::new();
        cancellation.cancel();

        assert!(matches!(
            prepare_local_file(&path, &cancellation, ImagePreviewMode::Direct),
            Err(ExplorerError::Cancelled)
        ));
    }

    #[test]
    fn resources_are_single_use_bounded_and_expire() {
        let mut store = PreviewResourceStore::default();
        let start = Instant::now();
        let first = store
            .insert_at(vec![1, 2, 3], start)
            .expect("insert resource");
        assert_eq!(store.take_at(&first, start), Some(vec![1, 2, 3]));
        assert_eq!(store.take_at(&first, start), None);

        let expired = store
            .insert_at(vec![4, 5], start)
            .expect("insert expiring resource");
        assert_eq!(store.take_at(&expired, start + PREVIEW_RESOURCE_TTL), None);

        for value in 0..=MAX_PREVIEW_RESOURCE_COUNT {
            store
                .insert_at(vec![value as u8], start)
                .expect("insert bounded resource");
        }
        assert_eq!(store.resources.len(), MAX_PREVIEW_RESOURCE_COUNT);
    }

    #[tokio::test]
    async fn manager_returns_one_shot_binary_resources() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("photo.png");
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(4, 3, Rgba([1, 2, 3, 255])))
            .save(&path)
            .expect("write image");
        let manager = PreviewManager::default();

        let preview = manager
            .prepare_local(
                "request-1".to_owned(),
                "entry-1".to_owned(),
                path,
                ImagePreviewMode::Direct,
            )
            .await
            .expect("prepare image");
        let PreviewContentDto::Image {
            resource_id,
            image_mode,
            ..
        } = preview.content
        else {
            panic!("expected image resource");
        };
        assert_eq!(image_mode, ImagePreviewMode::Direct);
        let bytes = manager
            .take_resource(&resource_id)
            .expect("consume resource");
        assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']));
        assert!(matches!(
            manager.take_resource(&resource_id),
            Err(ExplorerError::InvalidReference)
        ));
    }

    #[tokio::test]
    async fn manager_returns_one_shot_pdf_resources() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("brief.pdf");
        let source = b"%PDF-1.7\n%%EOF\n";
        fs::write(&path, source).expect("write PDF");
        let manager = PreviewManager::default();

        let preview = manager
            .prepare_local(
                "request-pdf".to_owned(),
                "entry-pdf".to_owned(),
                path,
                ImagePreviewMode::Direct,
            )
            .await
            .expect("prepare PDF");
        let PreviewContentDto::Pdf {
            resource_id,
            media_type,
        } = preview.content
        else {
            panic!("expected PDF resource");
        };
        assert_eq!(media_type, "application/pdf");
        assert_eq!(
            manager
                .take_resource(&resource_id)
                .expect("consume resource"),
            source
        );
        assert!(matches!(
            manager.take_resource(&resource_id),
            Err(ExplorerError::InvalidReference)
        ));
    }
}
