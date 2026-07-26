use std::time::Duration;

use portable_pty::PtySize;
use serde::{Deserialize, Serialize};

use crate::filesystem::{ExplorerError, ExplorerErrorDto};

pub const TERMINAL_OUTPUT_FRAME_VERSION: u8 = 1;
pub const TERMINAL_OUTPUT_FRAME_HEADER_BYTES: usize = 10;

#[derive(Debug, Clone, Copy)]
pub struct TerminalPolicy;

impl TerminalPolicy {
    pub const MAX_SESSIONS_PER_WINDOW: usize = 6;
    pub const MIN_COLUMNS: u16 = 2;
    pub const MAX_COLUMNS: u16 = 1_000;
    pub const MIN_ROWS: u16 = 1;
    pub const MAX_ROWS: u16 = 500;
    pub const MAX_PIXEL_DIMENSION: u16 = 32_768;
    pub const MAX_INPUT_BYTES: usize = 64 * 1024;
    pub const MAX_PENDING_REMOTE_COMMANDS: usize = 16;
    pub const OUTPUT_CHUNK_BYTES: usize = 16 * 1024;
    pub const MAX_IN_FLIGHT_OUTPUT_BYTES: usize = 1024 * 1024;
    pub const CLOSE_GRACE_PERIOD: Duration = Duration::from_millis(750);
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSizeDto {
    pub columns: u16,
    pub rows: u16,
    pub pixel_width: Option<u16>,
    pub pixel_height: Option<u16>,
}

impl TerminalSizeDto {
    pub fn validate(self) -> Result<Self, ExplorerError> {
        if !(TerminalPolicy::MIN_COLUMNS..=TerminalPolicy::MAX_COLUMNS).contains(&self.columns)
            || !(TerminalPolicy::MIN_ROWS..=TerminalPolicy::MAX_ROWS).contains(&self.rows)
            || self
                .pixel_width
                .is_some_and(|value| value > TerminalPolicy::MAX_PIXEL_DIMENSION)
            || self
                .pixel_height
                .is_some_and(|value| value > TerminalPolicy::MAX_PIXEL_DIMENSION)
        {
            return Err(ExplorerError::InvalidReference);
        }
        Ok(self)
    }

    pub fn into_pty_size(self) -> PtySize {
        PtySize {
            rows: self.rows,
            cols: self.columns,
            pixel_width: self.pixel_width.unwrap_or_default(),
            pixel_height: self.pixel_height.unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TerminalSessionState {
    Starting,
    Running,
    Exited,
    Failed,
    Closing,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TerminalSessionKind {
    Local,
    Ssh,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSessionSummaryDto {
    pub id: String,
    pub state: TerminalSessionState,
    pub kind: TerminalSessionKind,
    pub location_id: String,
    pub title: String,
    pub context_label: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TerminalCloseReason {
    User,
    Restart,
    WindowClosed,
    ApplicationExit,
    ChannelClosed,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TerminalExitReason {
    Completed,
    Terminated,
    TransportClosed,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(
    tag = "event",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TerminalControlEventDto {
    Started {
        session: TerminalSessionSummaryDto,
    },
    Exited {
        exit_code: Option<u32>,
        signal: Option<String>,
        reason: TerminalExitReason,
    },
    Failed {
        error: ExplorerErrorDto,
    },
}

pub fn encode_output_frame(sequence: u64, bytes: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(TERMINAL_OUTPUT_FRAME_HEADER_BYTES + bytes.len());
    frame.push(TERMINAL_OUTPUT_FRAME_VERSION);
    frame.push(1);
    frame.extend_from_slice(&sequence.to_be_bytes());
    frame.extend_from_slice(bytes);
    frame
}

#[cfg(test)]
mod tests {
    use super::{
        encode_output_frame, TerminalPolicy, TerminalSizeDto, TERMINAL_OUTPUT_FRAME_HEADER_BYTES,
        TERMINAL_OUTPUT_FRAME_VERSION,
    };

    #[test]
    fn terminal_size_enforces_centralized_bounds() {
        let valid = TerminalSizeDto {
            columns: TerminalPolicy::MIN_COLUMNS,
            rows: TerminalPolicy::MAX_ROWS,
            pixel_width: Some(TerminalPolicy::MAX_PIXEL_DIMENSION),
            pixel_height: None,
        };
        assert_eq!(valid.validate().unwrap(), valid);

        for invalid in [
            TerminalSizeDto {
                columns: 1,
                ..valid
            },
            TerminalSizeDto {
                rows: TerminalPolicy::MAX_ROWS + 1,
                ..valid
            },
            TerminalSizeDto {
                pixel_width: Some(TerminalPolicy::MAX_PIXEL_DIMENSION + 1),
                ..valid
            },
        ] {
            assert!(invalid.validate().is_err());
        }
    }

    #[test]
    fn output_frame_preserves_sequence_and_arbitrary_bytes() {
        let bytes = [0, 0xff, b'\r', b'\n', 0x1b];
        let frame = encode_output_frame(42, &bytes);
        assert_eq!(
            frame.len(),
            TERMINAL_OUTPUT_FRAME_HEADER_BYTES + bytes.len()
        );
        assert_eq!(frame[0], TERMINAL_OUTPUT_FRAME_VERSION);
        assert_eq!(frame[1], 1);
        assert_eq!(u64::from_be_bytes(frame[2..10].try_into().unwrap()), 42);
        assert_eq!(&frame[10..], bytes.as_slice());
    }
}
