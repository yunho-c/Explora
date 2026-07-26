use std::{
    collections::HashMap,
    io::{Read, Write},
    path::Path,
    sync::{
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex,
    },
    thread,
};

use portable_pty::{Child, ChildKiller, ExitStatus, MasterPty};
use tauri::ipc::{Channel, Response};
use uuid::Uuid;

use crate::filesystem::{ExplorerError, ExplorerErrorDto};

use super::{
    flow::OutputWindow,
    local::{self, SpawnedLocalPty},
    remote::{RemoteTerminalSession, SshTerminalLaunch},
    types::{
        encode_output_frame, TerminalCloseReason, TerminalControlEventDto, TerminalExitReason,
        TerminalPolicy, TerminalSessionKind, TerminalSessionState, TerminalSessionSummaryDto,
        TerminalSizeDto,
    },
};

type TerminalChannel = Channel<Response>;
type ChildWaitResult = Result<ExitStatus, std::io::Error>;

pub struct LocalTerminalLaunch<'a> {
    pub window_label: &'a str,
    pub location_id: &'a str,
    pub working_directory: &'a Path,
    pub title: &'a str,
    pub context_label: &'a str,
    pub size: TerminalSizeDto,
    pub on_event: TerminalChannel,
}

struct TerminalSession {
    id: String,
    window_label: String,
    location_id: String,
    title: String,
    context_label: String,
    state: Mutex<TerminalSessionState>,
    close_reason: Mutex<Option<TerminalCloseReason>>,
    next_input_sequence: Mutex<u64>,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    killer: Mutex<Option<Box<dyn ChildKiller + Send + Sync>>>,
    output_window: OutputWindow,
    on_event: TerminalChannel,
}

impl TerminalSession {
    fn new(
        launch: &LocalTerminalLaunch<'_>,
        master: Box<dyn MasterPty + Send>,
        writer: Box<dyn Write + Send>,
        killer: Box<dyn ChildKiller + Send + Sync>,
    ) -> Arc<Self> {
        Arc::new(Self {
            id: Uuid::new_v4().to_string(),
            window_label: launch.window_label.to_owned(),
            location_id: launch.location_id.to_owned(),
            title: launch.title.to_owned(),
            context_label: launch.context_label.to_owned(),
            state: Mutex::new(TerminalSessionState::Starting),
            close_reason: Mutex::new(None),
            next_input_sequence: Mutex::new(0),
            writer: Mutex::new(Some(writer)),
            master: Mutex::new(Some(master)),
            killer: Mutex::new(Some(killer)),
            output_window: OutputWindow::new(TerminalPolicy::MAX_IN_FLIGHT_OUTPUT_BYTES),
            on_event: launch.on_event.clone(),
        })
    }

    fn summary(&self) -> Result<TerminalSessionSummaryDto, ExplorerError> {
        Ok(TerminalSessionSummaryDto {
            id: self.id.clone(),
            state: *self
                .state
                .lock()
                .map_err(|_| ExplorerError::StateUnavailable)?,
            kind: TerminalSessionKind::Local,
            location_id: self.location_id.clone(),
            title: self.title.clone(),
            context_label: self.context_label.clone(),
        })
    }

    fn mark_running(&self) -> Result<(), ExplorerError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        if *state != TerminalSessionState::Starting {
            return Err(ExplorerError::InvalidReference);
        }
        *state = TerminalSessionState::Running;
        Ok(())
    }

    fn ensure_owner(&self, window_label: &str) -> Result<(), ExplorerError> {
        if self.window_label == window_label {
            Ok(())
        } else {
            Err(ExplorerError::InvalidReference)
        }
    }

    fn start_workers(
        self: &Arc<Self>,
        reader: Box<dyn Read + Send>,
        mut child: Box<dyn Child + Send + Sync>,
    ) -> Result<SyncSender<()>, ExplorerError> {
        let (status_sender, status_receiver) = mpsc::sync_channel(1);
        if thread::Builder::new()
            .name(format!("terminal-wait-{}", self.id))
            .spawn(move || {
                let _ = status_sender.send(child.wait());
            })
            .is_err()
        {
            self.force_terminate();
            return Err(ExplorerError::Unexpected(
                "Explora could not start the terminal wait worker.".into(),
            ));
        }

        let (start_sender, start_receiver) = mpsc::sync_channel(1);
        let session = self.clone();
        if thread::Builder::new()
            .name(format!("terminal-output-{}", self.id))
            .spawn(move || {
                if start_receiver.recv().is_ok() {
                    session.consume_output(reader, status_receiver);
                }
            })
            .is_err()
        {
            self.force_terminate();
            return Err(ExplorerError::Unexpected(
                "Explora could not start the terminal output worker.".into(),
            ));
        }
        Ok(start_sender)
    }

    fn consume_output(
        self: Arc<Self>,
        mut reader: Box<dyn Read + Send>,
        status_receiver: Receiver<ChildWaitResult>,
    ) {
        let mut buffer = vec![0_u8; TerminalPolicy::OUTPUT_CHUNK_BYTES];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(byte_count) => {
                    let sequence = match self.output_window.reserve(byte_count) {
                        Ok(sequence) => sequence,
                        Err(ExplorerError::Cancelled) if self.is_closing() => break,
                        Err(error) => {
                            self.fail(error);
                            return;
                        }
                    };
                    if self
                        .on_event
                        .send(Response::new(encode_output_frame(
                            sequence,
                            &buffer[..byte_count],
                        )))
                        .is_err()
                    {
                        self.begin_close(TerminalCloseReason::ChannelClosed);
                        break;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) if self.is_closing() => break,
                Err(_) => {
                    self.fail(terminal_io_error("Terminal output stopped unexpectedly."));
                    return;
                }
            }
        }

        match status_receiver.recv() {
            Ok(Ok(status)) => self.finish_exit(status),
            Ok(Err(_)) | Err(_) if self.is_closing() => self.finish_transport_close(),
            Ok(Err(_)) | Err(_) => {
                self.fail(terminal_io_error(
                    "Explora could not read the terminal exit status.",
                ));
            }
        }
    }

    fn write(&self, input_sequence: u64, bytes: &[u8]) -> Result<(), ExplorerError> {
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
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let writer = writer.as_mut().ok_or(ExplorerError::InvalidReference)?;
        writer
            .write_all(bytes)
            .and_then(|_| writer.flush())
            .map_err(|_| terminal_io_error("Explora could not write to the terminal."))?;
        *next_sequence = next_sequence.checked_add(1).ok_or_else(|| {
            ExplorerError::Unexpected("Terminal input sequence exhausted.".into())
        })?;
        Ok(())
    }

    fn resize(&self, size: TerminalSizeDto) -> Result<(), ExplorerError> {
        if *self
            .state
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            != TerminalSessionState::Running
        {
            return Err(ExplorerError::InvalidReference);
        }
        self.master
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .as_ref()
            .ok_or(ExplorerError::InvalidReference)?
            .resize(size.validate()?.into_pty_size())
            .map_err(|_| terminal_io_error("Explora could not resize the terminal."))
    }

    fn acknowledge(&self, output_sequence: u64) -> Result<(), ExplorerError> {
        self.output_window.acknowledge(output_sequence)
    }

    fn is_closing(&self) -> bool {
        self.state
            .lock()
            .map(|state| *state == TerminalSessionState::Closing)
            .unwrap_or(true)
    }

    fn begin_close(self: &Arc<Self>, reason: TerminalCloseReason) {
        let should_escalate = if let Ok(mut state) = self.state.lock() {
            match *state {
                TerminalSessionState::Starting | TerminalSessionState::Running => {
                    *state = TerminalSessionState::Closing;
                    true
                }
                TerminalSessionState::Closing
                | TerminalSessionState::Exited
                | TerminalSessionState::Failed => false,
            }
        } else {
            true
        };
        if let Ok(mut close_reason) = self.close_reason.lock() {
            close_reason.get_or_insert(reason);
        }
        self.output_window.close();
        if let Ok(mut writer) = self.writer.lock() {
            writer.take();
        }

        if should_escalate {
            let session = self.clone();
            let _ = thread::Builder::new()
                .name(format!("terminal-close-{}", self.id))
                .spawn(move || {
                    thread::sleep(TerminalPolicy::CLOSE_GRACE_PERIOD);
                    if session.is_closing() {
                        session.force_terminate();
                    }
                });
        }
    }

    fn force_terminate(&self) {
        if let Ok(mut killer) = self.killer.lock() {
            if let Some(killer) = killer.as_mut() {
                let _ = killer.kill();
            }
        }
    }

    fn finish_exit(&self, status: ExitStatus) {
        let reason = self
            .close_reason
            .lock()
            .ok()
            .and_then(|reason| *reason)
            .map_or(TerminalExitReason::Completed, |_| {
                TerminalExitReason::Terminated
            });
        if !self.transition_to_terminal_state(TerminalSessionState::Exited) {
            return;
        }
        self.release_transport();
        let _ = self.send_control(TerminalControlEventDto::Exited {
            exit_code: Some(status.exit_code()),
            signal: status.signal().map(str::to_owned),
            reason,
        });
    }

    fn finish_transport_close(&self) {
        if !self.transition_to_terminal_state(TerminalSessionState::Exited) {
            return;
        }
        self.release_transport();
        let _ = self.send_control(TerminalControlEventDto::Exited {
            exit_code: None,
            signal: None,
            reason: TerminalExitReason::TransportClosed,
        });
    }

    fn fail(&self, error: ExplorerError) {
        if !self.transition_to_terminal_state(TerminalSessionState::Failed) {
            return;
        }
        self.output_window.close();
        self.force_terminate();
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
        if let Ok(mut writer) = self.writer.lock() {
            writer.take();
        }
        if let Ok(mut master) = self.master.lock() {
            master.take();
        }
        if let Ok(mut killer) = self.killer.lock() {
            killer.take();
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

#[derive(Default)]
pub struct TerminalCoordinator {
    sessions: Mutex<HashMap<String, Arc<TerminalSession>>>,
    remote_sessions: Mutex<HashMap<String, Arc<RemoteTerminalSession>>>,
}

impl TerminalCoordinator {
    pub fn create_local(
        &self,
        launch: LocalTerminalLaunch<'_>,
    ) -> Result<TerminalSessionSummaryDto, ExplorerError> {
        let size = launch.size.validate()?;
        self.ensure_capacity(launch.window_label)?;
        let SpawnedLocalPty {
            master,
            reader,
            writer,
            child,
            mut killer,
        } = local::spawn_default_shell(launch.working_directory, size.into_pty_size())?;
        self.create_with_transport(launch, master, reader, writer, child, &mut killer)
    }

    pub fn create_ssh(
        &self,
        launch: SshTerminalLaunch<'_>,
    ) -> Result<TerminalSessionSummaryDto, ExplorerError> {
        let window_label = launch.window_label.to_owned();
        let local_sessions = self
            .sessions
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let mut remote_sessions = self
            .remote_sessions
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        if Self::session_count_for_window(&local_sessions, &remote_sessions, &window_label)
            >= TerminalPolicy::MAX_SESSIONS_PER_WINDOW
        {
            let channel = launch.channel;
            tauri::async_runtime::spawn(async move {
                let _ = channel.eof().await;
                let _ = channel.close().await;
            });
            return Err(ExplorerError::Unsupported(format!(
                "A window can have at most {} terminal sessions.",
                TerminalPolicy::MAX_SESSIONS_PER_WINDOW
            )));
        }
        let (session, start_sender) = RemoteTerminalSession::start(launch)?;
        let summary = session.summary()?;
        remote_sessions.insert(session.id().to_owned(), session);
        drop(remote_sessions);
        drop(local_sessions);
        let session = self.resolve_remote(&window_label, &summary.id)?;
        if let Err(error) = session.send_started() {
            let _ = self.remove_and_close_remote(
                &window_label,
                &summary.id,
                TerminalCloseReason::ChannelClosed,
            );
            return Err(error);
        }
        if start_sender.send(()).is_err() {
            let _ = self.remove_and_close_remote(
                &window_label,
                &summary.id,
                TerminalCloseReason::ChannelClosed,
            );
            return Err(ExplorerError::Unexpected(
                "The SSH terminal output worker stopped during startup.".into(),
            ));
        }
        Ok(summary)
    }

    fn ensure_capacity(&self, window_label: &str) -> Result<(), ExplorerError> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let remote_sessions = self
            .remote_sessions
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        if Self::session_count_for_window(&sessions, &remote_sessions, window_label)
            >= TerminalPolicy::MAX_SESSIONS_PER_WINDOW
        {
            return Err(ExplorerError::Unsupported(format!(
                "A window can have at most {} terminal sessions.",
                TerminalPolicy::MAX_SESSIONS_PER_WINDOW
            )));
        }
        Ok(())
    }

    fn session_count_for_window(
        sessions: &HashMap<String, Arc<TerminalSession>>,
        remote_sessions: &HashMap<String, Arc<RemoteTerminalSession>>,
        window_label: &str,
    ) -> usize {
        sessions
            .values()
            .filter(|session| session.window_label == window_label)
            .count()
            + remote_sessions
                .values()
                .filter(|session| session.belongs_to_window(window_label))
                .count()
    }

    fn create_with_transport(
        &self,
        launch: LocalTerminalLaunch<'_>,
        master: Box<dyn MasterPty + Send>,
        reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
        child: Box<dyn Child + Send + Sync>,
        killer: &mut Box<dyn ChildKiller + Send + Sync>,
    ) -> Result<TerminalSessionSummaryDto, ExplorerError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let remote_sessions = self
            .remote_sessions
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        if Self::session_count_for_window(&sessions, &remote_sessions, launch.window_label)
            >= TerminalPolicy::MAX_SESSIONS_PER_WINDOW
        {
            let _ = killer.kill();
            return Err(ExplorerError::Unsupported(format!(
                "A window can have at most {} terminal sessions.",
                TerminalPolicy::MAX_SESSIONS_PER_WINDOW
            )));
        }
        drop(remote_sessions);

        let session = TerminalSession::new(&launch, master, writer, killer.clone_killer());
        let start_sender = session.start_workers(reader, child)?;
        session.mark_running()?;
        let summary = session.summary()?;
        sessions.insert(session.id.clone(), session.clone());
        drop(sessions);

        if let Err(error) = session.send_control(TerminalControlEventDto::Started {
            session: summary.clone(),
        }) {
            let _ = self.remove_and_close(
                launch.window_label,
                &session.id,
                TerminalCloseReason::ChannelClosed,
            );
            let _ = start_sender.send(());
            return Err(error);
        }
        if start_sender.send(()).is_err() {
            let _ = self.remove_and_close(
                launch.window_label,
                &session.id,
                TerminalCloseReason::ChannelClosed,
            );
            return Err(ExplorerError::Unexpected(
                "The terminal output worker stopped during startup.".into(),
            ));
        }
        Ok(summary)
    }

    pub fn write(
        &self,
        window_label: &str,
        session_id: &str,
        input_sequence: u64,
        bytes: &[u8],
    ) -> Result<(), ExplorerError> {
        if let Some(session) = self.resolve_local(window_label, session_id)? {
            return session.write(input_sequence, bytes);
        }
        self.resolve_remote(window_label, session_id)?
            .write(input_sequence, bytes)
    }

    pub fn resize(
        &self,
        window_label: &str,
        session_id: &str,
        size: TerminalSizeDto,
    ) -> Result<(), ExplorerError> {
        if let Some(session) = self.resolve_local(window_label, session_id)? {
            return session.resize(size);
        }
        self.resolve_remote(window_label, session_id)?.resize(size)
    }

    pub fn acknowledge(
        &self,
        window_label: &str,
        session_id: &str,
        output_sequence: u64,
    ) -> Result<(), ExplorerError> {
        if let Some(session) = self.resolve_local(window_label, session_id)? {
            return session.acknowledge(output_sequence);
        }
        self.resolve_remote(window_label, session_id)?
            .acknowledge(output_sequence)
    }

    pub fn close(
        &self,
        window_label: &str,
        session_id: &str,
        reason: TerminalCloseReason,
    ) -> Result<(), ExplorerError> {
        if self.remove_and_close(window_label, session_id, reason)? {
            return Ok(());
        }
        self.remove_and_close_remote(window_label, session_id, reason)
    }

    pub fn close_window(&self, window_label: &str, reason: TerminalCloseReason) {
        let sessions = if let Ok(mut sessions) = self.sessions.lock() {
            let ids = sessions
                .iter()
                .filter_map(|(id, session)| {
                    (session.window_label == window_label).then_some(id.clone())
                })
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| sessions.remove(&id))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        for session in sessions {
            session.begin_close(reason);
        }
        let remote_sessions = if let Ok(mut sessions) = self.remote_sessions.lock() {
            let ids = sessions
                .iter()
                .filter_map(|(id, session)| {
                    session
                        .belongs_to_window(window_label)
                        .then_some(id.clone())
                })
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| sessions.remove(&id))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        for session in remote_sessions {
            session.begin_close(reason);
        }
    }

    pub fn close_all(&self, reason: TerminalCloseReason) {
        let sessions: Vec<Arc<TerminalSession>> = self
            .sessions
            .lock()
            .map(|mut sessions| sessions.drain().map(|(_, session)| session).collect())
            .unwrap_or_default();
        for session in sessions {
            session.begin_close(reason);
        }
        let remote_sessions: Vec<Arc<RemoteTerminalSession>> = self
            .remote_sessions
            .lock()
            .map(|mut sessions| sessions.drain().map(|(_, session)| session).collect())
            .unwrap_or_default();
        for session in remote_sessions {
            session.begin_close(reason);
        }
    }

    fn resolve_local(
        &self,
        window_label: &str,
        session_id: &str,
    ) -> Result<Option<Arc<TerminalSession>>, ExplorerError> {
        let session = self
            .sessions
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .get(session_id)
            .cloned();
        if let Some(session) = &session {
            session.ensure_owner(window_label)?;
        }
        Ok(session)
    }

    fn resolve_remote(
        &self,
        window_label: &str,
        session_id: &str,
    ) -> Result<Arc<RemoteTerminalSession>, ExplorerError> {
        let session = self
            .remote_sessions
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .get(session_id)
            .cloned()
            .ok_or(ExplorerError::InvalidReference)?;
        session.ensure_owner(window_label)?;
        Ok(session)
    }

    fn remove_and_close(
        &self,
        window_label: &str,
        session_id: &str,
        reason: TerminalCloseReason,
    ) -> Result<bool, ExplorerError> {
        let session = {
            let mut sessions = self
                .sessions
                .lock()
                .map_err(|_| ExplorerError::StateUnavailable)?;
            match sessions.get(session_id) {
                Some(session) if session.window_label != window_label => {
                    return Err(ExplorerError::InvalidReference);
                }
                Some(_) => sessions.remove(session_id),
                None => None,
            }
        };
        if let Some(session) = session {
            session.begin_close(reason);
            return Ok(true);
        }
        Ok(false)
    }

    fn remove_and_close_remote(
        &self,
        window_label: &str,
        session_id: &str,
        reason: TerminalCloseReason,
    ) -> Result<(), ExplorerError> {
        let session = {
            let mut sessions = self
                .remote_sessions
                .lock()
                .map_err(|_| ExplorerError::StateUnavailable)?;
            match sessions.get(session_id) {
                Some(session) if !session.belongs_to_window(window_label) => {
                    return Err(ExplorerError::InvalidReference);
                }
                Some(_) => sessions.remove(session_id),
                None => None,
            }
        };
        if let Some(session) = session {
            session.begin_close(reason);
        }
        Ok(())
    }
}

impl Drop for TerminalCoordinator {
    fn drop(&mut self) {
        self.close_all(TerminalCloseReason::ApplicationExit);
    }
}

fn terminal_io_error(message: &str) -> ExplorerError {
    ExplorerError::Io {
        message: message.to_owned(),
        kind: std::io::ErrorKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    use portable_pty::{CommandBuilder, PtySize};
    use tauri::ipc::InvokeResponseBody;
    use tempfile::TempDir;

    use super::*;
    use crate::terminal::{
        local,
        types::{TERMINAL_OUTPUT_FRAME_HEADER_BYTES, TERMINAL_OUTPUT_FRAME_VERSION},
    };

    type FixtureTransport = (
        Box<dyn MasterPty + Send>,
        Box<dyn Read + Send>,
        Box<dyn Write + Send>,
        Box<dyn Child + Send + Sync>,
        Box<dyn ChildKiller + Send + Sync>,
    );

    fn fixture_transport(program: &str) -> FixtureTransport {
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", program]);
        let SpawnedLocalPty {
            master,
            reader,
            writer,
            child,
            killer,
        } = local::spawn_command(command, PtySize::default()).expect("fixture PTY");
        (master, reader, writer, child, killer)
    }

    fn channel() -> (TerminalChannel, mpsc::Receiver<InvokeResponseBody>) {
        let (sender, receiver) = mpsc::channel();
        let channel = Channel::new(move |body| {
            let _ = sender.send(body);
            Ok(())
        });
        (channel, receiver)
    }

    #[cfg(unix)]
    #[test]
    fn coordinator_streams_binary_output_in_order_and_reports_exit() {
        let temp = TempDir::new().expect("temporary directory");
        let coordinator = TerminalCoordinator::default();
        let (channel, receiver) = channel();
        let (master, reader, writer, child, mut killer) =
            fixture_transport("printf 'coordinator-fixture\\n'; exit 9");
        let summary = coordinator
            .create_with_transport(
                LocalTerminalLaunch {
                    window_label: "main",
                    location_id: "home",
                    working_directory: temp.path(),
                    title: "Fixture",
                    context_label: "Fixture directory",
                    size: TerminalSizeDto {
                        columns: 80,
                        rows: 24,
                        pixel_width: None,
                        pixel_height: None,
                    },
                    on_event: channel,
                },
                master,
                reader,
                writer,
                child,
                &mut killer,
            )
            .expect("create terminal");

        assert_eq!(summary.state, TerminalSessionState::Running);
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut output = Vec::new();
        let mut started = false;
        let mut exit_code = None;
        while Instant::now() < deadline && exit_code.is_none() {
            let body = receiver
                .recv_timeout(Duration::from_millis(100))
                .expect("terminal event");
            match body {
                InvokeResponseBody::Json(json) => {
                    let event: serde_json::Value =
                        serde_json::from_str(&json).expect("control event");
                    match event["event"].as_str() {
                        Some("started") => started = true,
                        Some("exited") => exit_code = event["exitCode"].as_u64(),
                        other => panic!("unexpected control event: {other:?}"),
                    }
                }
                InvokeResponseBody::Raw(frame) => {
                    assert_eq!(frame[0], TERMINAL_OUTPUT_FRAME_VERSION);
                    assert_eq!(frame[1], 1);
                    let sequence =
                        u64::from_be_bytes(frame[2..10].try_into().expect("sequence bytes"));
                    output.extend_from_slice(&frame[TERMINAL_OUTPUT_FRAME_HEADER_BYTES..]);
                    coordinator
                        .acknowledge("main", &summary.id, sequence)
                        .expect("acknowledge output");
                }
            }
        }

        assert!(started);
        assert_eq!(exit_code, Some(9));
        assert!(output
            .windows(b"coordinator-fixture".len())
            .any(|part| part == b"coordinator-fixture"));
        assert!(matches!(
            coordinator.close("other-window", &summary.id, TerminalCloseReason::User),
            Err(ExplorerError::InvalidReference)
        ));
        coordinator
            .close("main", &summary.id, TerminalCloseReason::User)
            .expect("close exited session");
        coordinator
            .close("main", &summary.id, TerminalCloseReason::User)
            .expect("idempotent close");
    }

    #[cfg(unix)]
    #[test]
    fn coordinator_rejects_cross_window_and_out_of_order_input() {
        let temp = TempDir::new().expect("temporary directory");
        let coordinator = TerminalCoordinator::default();
        let (channel, _receiver) = channel();
        let (master, reader, writer, child, mut killer) = fixture_transport("sleep 30");
        let summary = coordinator
            .create_with_transport(
                LocalTerminalLaunch {
                    window_label: "main",
                    location_id: "home",
                    working_directory: temp.path(),
                    title: "Fixture",
                    context_label: "Fixture directory",
                    size: TerminalSizeDto {
                        columns: 80,
                        rows: 24,
                        pixel_width: None,
                        pixel_height: None,
                    },
                    on_event: channel,
                },
                master,
                reader,
                writer,
                child,
                &mut killer,
            )
            .expect("create terminal");

        assert!(matches!(
            coordinator.write("other-window", &summary.id, 0, b"x"),
            Err(ExplorerError::InvalidReference)
        ));
        assert!(matches!(
            coordinator.write("main", &summary.id, 1, b"x"),
            Err(ExplorerError::InvalidReference)
        ));
        coordinator
            .write("main", &summary.id, 0, b"x")
            .expect("first input");
        assert!(matches!(
            coordinator.write("main", &summary.id, 0, b"x"),
            Err(ExplorerError::InvalidReference)
        ));
        coordinator
            .close("main", &summary.id, TerminalCloseReason::User)
            .expect("close running fixture");
    }
}
