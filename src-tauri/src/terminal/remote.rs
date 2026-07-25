use std::sync::{Arc, Mutex};

use russh::{client, Channel, ChannelMsg};
use tauri::ipc::{Channel as TauriChannel, Response};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::filesystem::{ExplorerError, ExplorerErrorDto};

use super::{
    flow::OutputWindow,
    types::{
        encode_output_frame, TerminalCloseReason, TerminalControlEventDto, TerminalExitReason,
        TerminalPolicy, TerminalSessionKind, TerminalSessionState, TerminalSessionSummaryDto,
        TerminalSizeDto,
    },
};

type TerminalChannel = TauriChannel<Response>;

pub struct SshTerminalLaunch<'a> {
    pub window_label: &'a str,
    pub location_id: &'a str,
    pub title: &'a str,
    pub context_label: &'a str,
    pub channel: Channel<client::Msg>,
    pub on_event: TerminalChannel,
}

enum RemoteCommand {
    Write(Vec<u8>),
    Resize(TerminalSizeDto),
    Close,
}

pub(super) struct RemoteTerminalSession {
    id: String,
    window_label: String,
    location_id: String,
    title: String,
    context_label: String,
    state: Mutex<TerminalSessionState>,
    next_input_sequence: Mutex<u64>,
    commands: Mutex<Option<mpsc::Sender<RemoteCommand>>>,
    output_window: OutputWindow,
    on_event: TerminalChannel,
}

impl RemoteTerminalSession {
    pub(super) fn start(
        launch: SshTerminalLaunch<'_>,
    ) -> Result<(Arc<Self>, oneshot::Sender<()>), ExplorerError> {
        let (commands, command_receiver) =
            mpsc::channel(TerminalPolicy::MAX_PENDING_REMOTE_COMMANDS);
        let (start_sender, start_receiver) = oneshot::channel();
        let session = Arc::new(Self {
            id: Uuid::new_v4().to_string(),
            window_label: launch.window_label.to_owned(),
            location_id: launch.location_id.to_owned(),
            title: launch.title.to_owned(),
            context_label: launch.context_label.to_owned(),
            state: Mutex::new(TerminalSessionState::Running),
            next_input_sequence: Mutex::new(0),
            commands: Mutex::new(Some(commands)),
            output_window: OutputWindow::new(TerminalPolicy::MAX_IN_FLIGHT_OUTPUT_BYTES),
            on_event: launch.on_event,
        });

        let worker = session.clone();
        let channel = launch.channel;
        tauri::async_runtime::spawn(async move {
            if start_receiver.await.is_ok() {
                worker.run(channel, command_receiver).await;
            } else {
                let _ = channel.eof().await;
                let _ = channel.close().await;
            }
        });
        Ok((session, start_sender))
    }

    pub(super) fn summary(&self) -> Result<TerminalSessionSummaryDto, ExplorerError> {
        Ok(TerminalSessionSummaryDto {
            id: self.id.clone(),
            state: *self
                .state
                .lock()
                .map_err(|_| ExplorerError::StateUnavailable)?,
            kind: TerminalSessionKind::Ssh,
            location_id: self.location_id.clone(),
            title: self.title.clone(),
            context_label: self.context_label.clone(),
        })
    }

    pub(super) fn id(&self) -> &str {
        &self.id
    }

    pub(super) fn send_started(&self) -> Result<(), ExplorerError> {
        self.send_control(TerminalControlEventDto::Started {
            session: self.summary()?,
        })
    }

    pub(super) fn ensure_owner(&self, window_label: &str) -> Result<(), ExplorerError> {
        if self.window_label == window_label {
            Ok(())
        } else {
            Err(ExplorerError::InvalidReference)
        }
    }

    pub(super) fn belongs_to_window(&self, window_label: &str) -> bool {
        self.window_label == window_label
    }

    pub(super) fn write(&self, input_sequence: u64, bytes: &[u8]) -> Result<(), ExplorerError> {
        if bytes.is_empty() || bytes.len() > TerminalPolicy::MAX_INPUT_BYTES {
            return Err(ExplorerError::InvalidReference);
        }
        if *self
            .state
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            != TerminalSessionState::Running
        {
            return Err(ExplorerError::InvalidReference);
        }
        let mut next_sequence = self
            .next_input_sequence
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        if input_sequence != *next_sequence {
            return Err(ExplorerError::InvalidReference);
        }
        self.send_command(RemoteCommand::Write(bytes.to_vec()))?;
        *next_sequence = next_sequence.checked_add(1).ok_or_else(|| {
            ExplorerError::Unexpected("Terminal input sequence exhausted.".into())
        })?;
        Ok(())
    }

    pub(super) fn resize(&self, size: TerminalSizeDto) -> Result<(), ExplorerError> {
        if *self
            .state
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            != TerminalSessionState::Running
        {
            return Err(ExplorerError::InvalidReference);
        }
        self.send_command(RemoteCommand::Resize(size.validate()?))
    }

    pub(super) fn acknowledge(&self, output_sequence: u64) -> Result<(), ExplorerError> {
        self.output_window.acknowledge(output_sequence)
    }

    pub(super) fn begin_close(&self, _reason: TerminalCloseReason) {
        let should_close = self
            .state
            .lock()
            .map(|mut state| {
                if matches!(
                    *state,
                    TerminalSessionState::Starting | TerminalSessionState::Running
                ) {
                    *state = TerminalSessionState::Closing;
                    true
                } else {
                    false
                }
            })
            .unwrap_or(true);
        self.output_window.close();
        if should_close && self.send_command(RemoteCommand::Close).is_err() {
            self.finish(None, None, TerminalExitReason::TransportClosed);
        }
    }

    async fn run(
        self: Arc<Self>,
        mut channel: Channel<client::Msg>,
        mut commands: mpsc::Receiver<RemoteCommand>,
    ) {
        let mut exit_code = None;
        let mut exit_signal = None;
        loop {
            tokio::select! {
                command = commands.recv() => {
                    match command {
                        Some(RemoteCommand::Write(bytes)) => {
                            if channel.data_bytes(bytes).await.is_err() {
                                self.fail(ExplorerError::Offline(
                                    "The SSH terminal stopped accepting input.".to_owned(),
                                ));
                                return;
                            }
                        }
                        Some(RemoteCommand::Resize(size)) => {
                            if channel.window_change(
                                u32::from(size.columns),
                                u32::from(size.rows),
                                u32::from(size.pixel_width.unwrap_or_default()),
                                u32::from(size.pixel_height.unwrap_or_default()),
                            ).await.is_err() {
                                self.fail(ExplorerError::Offline(
                                    "The SSH terminal could not be resized.".to_owned(),
                                ));
                                return;
                            }
                        }
                        Some(RemoteCommand::Close) | None => {
                            let _ = channel.eof().await;
                            let _ = channel.close().await;
                            self.finish(exit_code, exit_signal, TerminalExitReason::Terminated);
                            return;
                        }
                    }
                }
                message = channel.wait() => {
                    match message {
                        Some(ChannelMsg::Data { data })
                        | Some(ChannelMsg::ExtendedData { data, .. }) => {
                            if self.emit_output(data.as_ref()).await.is_err() {
                                let _ = channel.close().await;
                                return;
                            }
                        }
                        Some(ChannelMsg::ExitStatus { exit_status }) => {
                            exit_code = Some(exit_status);
                        }
                        Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                            exit_signal = Some(format_signal(signal_name));
                        }
                        Some(ChannelMsg::Close) | None => {
                            let reason = if self.is_closing() {
                                TerminalExitReason::Terminated
                            } else if exit_code.is_some() || exit_signal.is_some() {
                                TerminalExitReason::Completed
                            } else {
                                TerminalExitReason::TransportClosed
                            };
                            self.finish(exit_code, exit_signal, reason);
                            return;
                        }
                        Some(ChannelMsg::Eof) => {}
                        Some(_) => {}
                    }
                }
            }
        }
    }

    async fn emit_output(self: &Arc<Self>, bytes: &[u8]) -> Result<(), ExplorerError> {
        for chunk in bytes.chunks(TerminalPolicy::OUTPUT_CHUNK_BYTES) {
            let session = self.clone();
            let byte_count = chunk.len();
            let sequence = tauri::async_runtime::spawn_blocking(move || {
                session.output_window.reserve(byte_count)
            })
            .await
            .map_err(|error| {
                ExplorerError::Unexpected(format!("The SSH terminal output worker failed: {error}"))
            })??;
            if self
                .on_event
                .send(Response::new(encode_output_frame(sequence, chunk)))
                .is_err()
            {
                self.begin_close(TerminalCloseReason::ChannelClosed);
                return Err(ExplorerError::ChannelClosed);
            }
        }
        Ok(())
    }

    fn send_command(&self, command: RemoteCommand) -> Result<(), ExplorerError> {
        self.commands
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .as_ref()
            .ok_or(ExplorerError::InvalidReference)?
            .try_send(command)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => ExplorerError::Io {
                    message: "The SSH terminal input queue is full.".to_owned(),
                    kind: std::io::ErrorKind::WouldBlock,
                },
                mpsc::error::TrySendError::Closed(_) => ExplorerError::InvalidReference,
            })
    }

    fn is_closing(&self) -> bool {
        self.state
            .lock()
            .map(|state| *state == TerminalSessionState::Closing)
            .unwrap_or(true)
    }

    fn finish(&self, exit_code: Option<u32>, signal: Option<String>, reason: TerminalExitReason) {
        if !self.transition_to_terminal_state(TerminalSessionState::Exited) {
            return;
        }
        self.release_transport();
        let _ = self.send_control(TerminalControlEventDto::Exited {
            exit_code,
            signal,
            reason,
        });
    }

    fn fail(&self, error: ExplorerError) {
        if !self.transition_to_terminal_state(TerminalSessionState::Failed) {
            return;
        }
        self.release_transport();
        let _ = self.send_control(TerminalControlEventDto::Failed {
            error: ExplorerErrorDto::from(error),
        });
    }

    fn transition_to_terminal_state(&self, next: TerminalSessionState) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if matches!(
            *state,
            TerminalSessionState::Exited | TerminalSessionState::Failed
        ) {
            return false;
        }
        *state = next;
        true
    }

    fn release_transport(&self) {
        self.output_window.close();
        if let Ok(mut commands) = self.commands.lock() {
            commands.take();
        }
    }

    fn send_control(&self, event: TerminalControlEventDto) -> Result<(), ExplorerError> {
        let body = serde_json::to_string(&event).map_err(|_| {
            ExplorerError::Unexpected("Terminal event serialization failed.".into())
        })?;
        self.on_event
            .send(Response::new(body))
            .map_err(|_| ExplorerError::ChannelClosed)
    }
}

fn format_signal(signal: russh::Sig) -> String {
    match signal {
        russh::Sig::ABRT => "ABRT".to_owned(),
        russh::Sig::ALRM => "ALRM".to_owned(),
        russh::Sig::FPE => "FPE".to_owned(),
        russh::Sig::HUP => "HUP".to_owned(),
        russh::Sig::ILL => "ILL".to_owned(),
        russh::Sig::INT => "INT".to_owned(),
        russh::Sig::KILL => "KILL".to_owned(),
        russh::Sig::PIPE => "PIPE".to_owned(),
        russh::Sig::QUIT => "QUIT".to_owned(),
        russh::Sig::SEGV => "SEGV".to_owned(),
        russh::Sig::TERM => "TERM".to_owned(),
        russh::Sig::USR1 => "USR1".to_owned(),
        russh::Sig::Custom(name) => name,
    }
}
