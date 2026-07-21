use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use rand::rng;
use russh::{
    keys::{ssh_key::LineEnding, Algorithm, PrivateKey, PublicKey},
    server::{self, Auth, ChannelOpenHandle, Msg, Response, Session},
    Channel, ChannelId, Disconnect, MethodKind, MethodSet,
};
use russh_sftp::protocol::{
    Attrs, File, FileAttributes, Handle, Name, Status, StatusCode, Version,
};
use tempfile::TempDir;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{watch, Mutex},
    task::{JoinHandle, JoinSet},
};

const TEST_USERNAME: &str = "explora";
const TEST_PASSWORD: &str = "correct horse battery staple";
const TEST_PASSPHRASE: &str = "private key passphrase";
const TEST_CHALLENGE_ANSWER: &str = "123456";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestAuthMode {
    PublicKey,
    Password,
    KeyboardInteractive,
}

#[derive(Clone)]
struct ServerState {
    auth_mode: TestAuthMode,
    user_public_key: PublicKey,
    sftp_enabled: bool,
    listing_delay: Duration,
}

pub struct TestSshServer {
    address: SocketAddr,
    state: Arc<ServerState>,
    config: Arc<Mutex<Arc<server::Config>>>,
    handles: Arc<Mutex<Vec<server::Handle>>>,
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
    temp_dir: TempDir,
    identity_file: PathBuf,
    encrypted_identity_file: PathBuf,
    user_key: PrivateKey,
}

impl TestSshServer {
    pub async fn start(auth_mode: TestAuthMode) -> Self {
        Self::start_with_options(auth_mode, true, Duration::ZERO).await
    }

    pub async fn start_with_options(
        auth_mode: TestAuthMode,
        sftp_enabled: bool,
        listing_delay: Duration,
    ) -> Self {
        let temp_dir = tempfile::tempdir().expect("test SSH directory");
        let user_key =
            PrivateKey::random(&mut rng(), Algorithm::Ed25519).expect("test SSH user key");
        let identity_file = temp_dir.path().join("id_ed25519");
        user_key
            .write_openssh_file(&identity_file, LineEnding::LF)
            .expect("write test identity");
        let encrypted_identity_file = temp_dir.path().join("id_ed25519_encrypted");
        user_key
            .encrypt(&mut rng(), TEST_PASSPHRASE)
            .expect("encrypt test identity")
            .write_openssh_file(&encrypted_identity_file, LineEnding::LF)
            .expect("write encrypted test identity");

        let state = Arc::new(ServerState {
            auth_mode,
            user_public_key: user_key.public_key().clone(),
            sftp_enabled,
            listing_delay,
        });
        let config = Arc::new(Mutex::new(Arc::new(server_config(
            auth_mode,
            PrivateKey::random(&mut rng(), Algorithm::Ed25519).expect("test SSH host key"),
        ))));
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind test SSH server");
        let address = listener.local_addr().expect("test SSH address");
        let handles = Arc::new(Mutex::new(Vec::new()));
        let (shutdown, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(run_server(
            listener,
            state.clone(),
            config.clone(),
            handles.clone(),
            shutdown_rx,
        ));

        Self {
            address,
            state,
            config,
            handles,
            shutdown,
            task,
            temp_dir,
            identity_file,
            encrypted_identity_file,
            user_key,
        }
    }

    pub fn host(&self) -> &'static str {
        "127.0.0.1"
    }

    pub fn port(&self) -> u16 {
        self.address.port()
    }

    pub fn username(&self) -> &'static str {
        TEST_USERNAME
    }

    pub fn password(&self) -> &'static str {
        TEST_PASSWORD
    }

    pub fn challenge_answer(&self) -> &'static str {
        TEST_CHALLENGE_ANSWER
    }

    pub fn passphrase(&self) -> &'static str {
        TEST_PASSPHRASE
    }

    pub fn identity_file(&self) -> &Path {
        &self.identity_file
    }

    pub fn encrypted_identity_file(&self) -> &Path {
        &self.encrypted_identity_file
    }

    #[cfg(unix)]
    pub async fn start_agent(&self) -> TestAgent {
        use futures::stream;
        use russh::keys::agent::{client::AgentClient, server};
        use tokio::net::UnixListener;

        let socket_path = self.temp_dir.path().join("test-agent.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind test SSH agent");
        let incoming = Box::pin(stream::unfold(listener, |listener| async move {
            let next = listener.accept().await.map(|(stream, _)| stream);
            Some((next, listener))
        }));
        let task = tokio::spawn(async move {
            let _ = server::serve(incoming, ()).await;
        });
        let mut client = AgentClient::connect_uds(&socket_path)
            .await
            .expect("connect to test SSH agent");
        client
            .add_identity(&self.user_key, &[])
            .await
            .expect("add identity to test SSH agent");
        TestAgent { socket_path, task }
    }

    pub fn known_hosts_path(&self) -> PathBuf {
        self.temp_dir.path().join("known_hosts")
    }

    pub async fn rotate_host_key(&self) {
        *self.config.lock().await = Arc::new(server_config(
            self.state.auth_mode,
            PrivateKey::random(&mut rng(), Algorithm::Ed25519)
                .expect("replacement test SSH host key"),
        ));
    }

    pub async fn disconnect_clients(&self) {
        let handles = self.handles.lock().await.clone();
        for handle in handles {
            let _ = handle
                .disconnect(
                    Disconnect::ConnectionLost,
                    "Test server dropped the connection".to_owned(),
                    "en".to_owned(),
                )
                .await;
        }
    }

    pub async fn shutdown(self) {
        self.disconnect_clients().await;
        let _ = self.shutdown.send(true);
        let _ = tokio::time::timeout(Duration::from_secs(2), self.task).await;
    }
}

#[cfg(unix)]
pub struct TestAgent {
    socket_path: PathBuf,
    task: JoinHandle<()>,
}

#[cfg(unix)]
impl TestAgent {
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub async fn shutdown(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

fn server_config(auth_mode: TestAuthMode, host_key: PrivateKey) -> server::Config {
    let method = match auth_mode {
        TestAuthMode::PublicKey => MethodKind::PublicKey,
        TestAuthMode::Password => MethodKind::Password,
        TestAuthMode::KeyboardInteractive => MethodKind::KeyboardInteractive,
    };
    server::Config {
        methods: MethodSet::from(&[method][..]),
        auth_rejection_time: Duration::from_millis(1),
        auth_rejection_time_initial: Some(Duration::ZERO),
        keys: vec![host_key],
        nodelay: true,
        ..server::Config::default()
    }
}

async fn run_server(
    listener: TcpListener,
    state: Arc<ServerState>,
    config: Arc<Mutex<Arc<server::Config>>>,
    handles: Arc<Mutex<Vec<server::Handle>>>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut sessions = JoinSet::new();
    loop {
        tokio::select! {
            accept = listener.accept() => {
                let Ok((stream, _)) = accept else { break };
                let state = state.clone();
                let config = config.lock().await.clone();
                let handles = handles.clone();
                sessions.spawn(async move {
                    run_session(stream, state, config, handles).await;
                });
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { break }
            }
            Some(_) = sessions.join_next(), if !sessions.is_empty() => {}
        }
    }
    sessions.abort_all();
    while sessions.join_next().await.is_some() {}
}

async fn run_session(
    stream: TcpStream,
    state: Arc<ServerState>,
    config: Arc<server::Config>,
    handles: Arc<Mutex<Vec<server::Handle>>>,
) {
    let handler = TestSshHandler {
        state,
        channels: HashMap::new(),
    };
    if let Ok(session) = server::run_stream(config, stream, handler).await {
        handles.lock().await.push(session.handle());
        let _ = session.await;
    }
}

struct TestSshHandler {
    state: Arc<ServerState>,
    channels: HashMap<ChannelId, Channel<Msg>>,
}

impl server::Handler for TestSshHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(
            if self.state.auth_mode == TestAuthMode::Password
                && user == TEST_USERNAME
                && password == TEST_PASSWORD
            {
                Auth::Accept
            } else {
                Auth::reject()
            },
        )
    }

    async fn auth_publickey(
        &mut self,
        user: &str,
        public_key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(
            if self.state.auth_mode == TestAuthMode::PublicKey
                && user == TEST_USERNAME
                && public_key == &self.state.user_public_key
            {
                Auth::Accept
            } else {
                Auth::reject()
            },
        )
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        user: &str,
        _submethods: &str,
        response: Option<Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        if self.state.auth_mode != TestAuthMode::KeyboardInteractive || user != TEST_USERNAME {
            return Ok(Auth::reject());
        }
        if let Some(mut response) = response {
            return Ok(
                if response
                    .next()
                    .is_some_and(|answer| answer.as_ref() == TEST_CHALLENGE_ANSWER.as_bytes())
                {
                    Auth::Accept
                } else {
                    Auth::reject()
                },
            );
        }
        Ok(Auth::Partial {
            name: Cow::Borrowed("Verification"),
            instructions: Cow::Borrowed("Enter the test verification code."),
            prompts: Cow::Borrowed(&[(Cow::Borrowed("Verification code"), false)]),
        })
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name != "sftp" || !self.state.sftp_enabled {
            session.channel_failure(channel_id)?;
            return Ok(());
        }
        let Some(channel) = self.channels.remove(&channel_id) else {
            session.channel_failure(channel_id)?;
            return Ok(());
        };
        session.channel_success(channel_id)?;
        russh_sftp::server::run(
            channel.into_stream(),
            TestSftpHandler::new(self.state.listing_delay),
        )
        .await;
        Ok(())
    }
}

struct TestSftpHandler {
    completed_directories: HashSet<String>,
    listing_delay: Duration,
}

impl TestSftpHandler {
    fn new(listing_delay: Duration) -> Self {
        Self {
            completed_directories: HashSet::new(),
            listing_delay,
        }
    }
}

impl russh_sftp::server::Handler for TestSftpHandler {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn close(&mut self, id: u32, _handle: String) -> Result<Status, Self::Error> {
        Ok(ok_status(id))
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let path = canonical_path(&path);
        if path == "/private" {
            return Err(StatusCode::PermissionDenied);
        }
        if !matches!(path.as_str(), "/" | "/projects" | "/slow") {
            return Err(StatusCode::NoSuchFile);
        }
        if path == "/slow" && !self.listing_delay.is_zero() {
            tokio::time::sleep(self.listing_delay).await;
        }
        self.completed_directories.remove(&path);
        Ok(Handle { id, handle: path })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        if !self.completed_directories.insert(handle.clone()) {
            return Err(StatusCode::Eof);
        }
        Ok(Name {
            id,
            files: entries_for(&handle).ok_or(StatusCode::NoSuchFile)?,
        })
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let path = canonical_path(&path);
        if metadata_for(&path, true).is_none() {
            return Err(StatusCode::NoSuchFile);
        }
        Ok(Name {
            id,
            files: vec![File::dummy(path)],
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let path = canonical_path(&path);
        Ok(Attrs {
            id,
            attrs: metadata_for(&path, true).ok_or(StatusCode::NoSuchFile)?,
        })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let path = canonical_path(&path);
        Ok(Attrs {
            id,
            attrs: metadata_for(&path, false).ok_or(StatusCode::NoSuchFile)?,
        })
    }
}

fn canonical_path(path: &str) -> String {
    match path.trim_end_matches('/') {
        "" | "." => "/".to_owned(),
        path if path.starts_with('/') => path.to_owned(),
        path => format!("/{path}"),
    }
}

fn entries_for(path: &str) -> Option<Vec<File>> {
    match path {
        "/" => Some(vec![
            File::new("projects", directory_attrs()),
            File::new("README.md", file_attrs(128)),
            File::new("project-link", symlink_attrs()),
            File::new("private", directory_attrs()),
            File::new("slow", directory_attrs()),
        ]),
        "/projects" => Some(vec![
            File::new("explora", directory_attrs()),
            File::new("notes.txt", file_attrs(42)),
        ]),
        "/slow" => Some(vec![File::new("eventually.txt", file_attrs(7))]),
        _ => None,
    }
}

fn metadata_for(path: &str, follow_symlink: bool) -> Option<FileAttributes> {
    match path {
        "/" | "/projects" | "/projects/explora" | "/private" | "/slow" => Some(directory_attrs()),
        "/README.md" => Some(file_attrs(128)),
        "/projects/notes.txt" => Some(file_attrs(42)),
        "/slow/eventually.txt" => Some(file_attrs(7)),
        "/project-link" if follow_symlink => Some(directory_attrs()),
        "/project-link" => Some(symlink_attrs()),
        _ => None,
    }
}

fn directory_attrs() -> FileAttributes {
    let mut attrs = FileAttributes {
        permissions: Some(0o755),
        mtime: Some(1_700_000_000),
        ..FileAttributes::empty()
    };
    attrs.set_dir(true);
    attrs
}

fn file_attrs(size: u64) -> FileAttributes {
    let mut attrs = FileAttributes {
        size: Some(size),
        permissions: Some(0o644),
        mtime: Some(1_700_000_000),
        ..FileAttributes::empty()
    };
    attrs.set_regular(true);
    attrs
}

fn symlink_attrs() -> FileAttributes {
    let mut attrs = FileAttributes {
        permissions: Some(0o777),
        mtime: Some(1_700_000_000),
        ..FileAttributes::empty()
    };
    attrs.set_symlink(true);
    attrs
}

fn ok_status(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".to_owned(),
        language_tag: "en".to_owned(),
    }
}
