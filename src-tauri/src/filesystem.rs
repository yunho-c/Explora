use std::{path::Path, time::Duration};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const LISTING_BATCH_SIZE: usize = 256;
pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(300);
pub const SSH_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
pub const SSH_KEEPALIVE_MAX: usize = 3;
pub const SFTP_REQUEST_TIMEOUT_SECONDS: u64 = 30;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ImagePreviewMode {
    Direct,
    Sanitized,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PreviewUnavailableReason {
    Unsupported,
    Remote,
    Directory,
    Symlink,
    TooLarge,
    Binary,
    Malformed,
    TimedOut,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PreviewContentDto {
    Metadata {
        reason: PreviewUnavailableReason,
        message: String,
    },
    Text {
        text: String,
        truncated: bool,
        encoding: &'static str,
    },
    Image {
        resource_id: String,
        media_type: &'static str,
        image_mode: ImagePreviewMode,
        width: u32,
        height: u32,
        original_width: u32,
        original_height: u32,
    },
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewResultDto {
    pub entry_id: String,
    pub size: Option<String>,
    pub modified_at: Option<u64>,
    pub content: PreviewContentDto,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LocationRole {
    Home,
    Desktop,
    Documents,
    Downloads,
    Pictures,
    Music,
    Videos,
    Ssh,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EntryRefDto {
    pub id: String,
    pub location_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryRefDto {
    pub id: String,
    pub location_id: String,
    pub name: String,
    pub display_path: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BreadcrumbSegmentDto {
    pub label: String,
    pub directory: DirectoryRefDto,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocationSummaryDto {
    pub id: String,
    pub name: String,
    pub kind: &'static str,
    pub role: LocationRole,
    pub status: &'static str,
    pub display_path: String,
    pub detail: String,
    pub root: DirectoryRefDto,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileEntrySummaryDto {
    pub reference: EntryRefDto,
    pub name: String,
    pub kind: &'static str,
    pub content_kind: &'static str,
    pub size: Option<String>,
    pub modified_at: Option<u64>,
    pub display_path: String,
    pub directory: Option<DirectoryRefDto>,
    pub detail: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(
    tag = "event",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DirectoryListingEvent {
    Started {
        directory: DirectoryRefDto,
        parent: Option<DirectoryRefDto>,
        breadcrumbs: Vec<BreadcrumbSegmentDto>,
    },
    Entries {
        entries: Vec<FileEntrySummaryDto>,
        replace: bool,
    },
    Complete {
        skipped_entries: usize,
    },
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ExplorerErrorCode {
    InvalidReference,
    NotFound,
    PermissionDenied,
    NotDirectory,
    Cancelled,
    Offline,
    AuthenticationFailed,
    HostKeyFailure,
    Unsupported,
    InvalidConfiguration,
    Unexpected,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExplorerErrorDto {
    pub code: ExplorerErrorCode,
    pub message: String,
}

#[derive(Debug, Error)]
pub enum ExplorerError {
    #[error("This filesystem reference is no longer valid.")]
    InvalidReference,
    #[error("The request was cancelled.")]
    Cancelled,
    #[error("Explora's filesystem state is unavailable.")]
    StateUnavailable,
    #[error("{message}")]
    Io {
        message: String,
        kind: std::io::ErrorKind,
    },
    #[error("The response channel closed unexpectedly.")]
    ChannelClosed,
    #[error("{0}")]
    Offline(String),
    #[error("{0}")]
    AuthenticationFailed(String),
    #[error("{0}")]
    HostKeyFailure(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    InvalidConfiguration(String),
    #[error("{0}")]
    Unexpected(String),
}

impl ExplorerError {
    pub(crate) fn io(action: &str, path: &Path, error: std::io::Error) -> Self {
        Self::Io {
            message: format!("Explora could not {action} {}: {error}", path.display()),
            kind: error.kind(),
        }
    }
}

impl From<ExplorerError> for ExplorerErrorDto {
    fn from(error: ExplorerError) -> Self {
        let code = match &error {
            ExplorerError::InvalidReference => ExplorerErrorCode::InvalidReference,
            ExplorerError::Cancelled => ExplorerErrorCode::Cancelled,
            ExplorerError::Io { kind, .. } => match kind {
                std::io::ErrorKind::NotFound => ExplorerErrorCode::NotFound,
                std::io::ErrorKind::PermissionDenied => ExplorerErrorCode::PermissionDenied,
                std::io::ErrorKind::NotADirectory => ExplorerErrorCode::NotDirectory,
                _ => ExplorerErrorCode::Unexpected,
            },
            ExplorerError::Offline(_) => ExplorerErrorCode::Offline,
            ExplorerError::AuthenticationFailed(_) => ExplorerErrorCode::AuthenticationFailed,
            ExplorerError::HostKeyFailure(_) => ExplorerErrorCode::HostKeyFailure,
            ExplorerError::Unsupported(_) => ExplorerErrorCode::Unsupported,
            ExplorerError::InvalidConfiguration(_) => ExplorerErrorCode::InvalidConfiguration,
            ExplorerError::StateUnavailable
            | ExplorerError::ChannelClosed
            | ExplorerError::Unexpected(_) => ExplorerErrorCode::Unexpected,
        };

        Self {
            code,
            message: error.to_string(),
        }
    }
}
