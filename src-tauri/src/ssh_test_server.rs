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
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};
use tempfile::TempDir;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{watch, Mutex, Notify},
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
    mutation_delay: Duration,
    mutation_started: Arc<Notify>,
    filesystem: Arc<Mutex<TestRemoteFilesystem>>,
}

#[derive(Clone)]
enum TestNode {
    Directory,
    File(Vec<u8>),
    Symlink(String),
}

struct TestRemoteFilesystem {
    nodes: HashMap<String, TestNode>,
}

impl Default for TestRemoteFilesystem {
    fn default() -> Self {
        Self {
            nodes: HashMap::from([
                ("/".to_owned(), TestNode::Directory),
                ("/projects".to_owned(), TestNode::Directory),
                ("/projects/explora".to_owned(), TestNode::Directory),
                (
                    "/projects/notes.txt".to_owned(),
                    TestNode::File(vec![0; 42]),
                ),
                ("/README.md".to_owned(), TestNode::File(vec![0; 128])),
                (
                    "/project-link".to_owned(),
                    TestNode::Symlink("/projects".to_owned()),
                ),
                ("/private".to_owned(), TestNode::Directory),
                (
                    "/private/secret.txt".to_owned(),
                    TestNode::File(vec![0; 16]),
                ),
                ("/slow".to_owned(), TestNode::Directory),
                (
                    "/slow/eventually.txt".to_owned(),
                    TestNode::File(vec![0; 7]),
                ),
                ("/partial".to_owned(), TestNode::Directory),
                ("/partial/a.txt".to_owned(), TestNode::File(vec![0; 4])),
                ("/partial/locked.txt".to_owned(), TestNode::File(vec![0; 8])),
                ("/locked.txt".to_owned(), TestNode::File(vec![0; 8])),
            ]),
        }
    }
}

impl TestRemoteFilesystem {
    fn metadata(&self, path: &str, follow_symlink: bool) -> Option<FileAttributes> {
        let path = canonical_path(path);
        let node = self.nodes.get(&path)?;
        match node {
            TestNode::Directory => Some(directory_attrs()),
            TestNode::File(bytes) => Some(file_attrs(bytes.len() as u64)),
            TestNode::Symlink(target) if follow_symlink => self.metadata(target, true),
            TestNode::Symlink(_) => Some(symlink_attrs()),
        }
    }

    fn entries(&self, directory: &str) -> Option<Vec<File>> {
        let directory = canonical_path(directory);
        if !matches!(self.nodes.get(&directory), Some(TestNode::Directory)) {
            return None;
        }
        let mut entries = self
            .nodes
            .iter()
            .filter(|(path, _)| {
                *path != &directory && test_parent(path).as_deref() == Some(directory.as_str())
            })
            .map(|(path, node)| {
                let name = path.rsplit('/').next().unwrap_or(path).to_owned();
                let attrs = match node {
                    TestNode::Directory => directory_attrs(),
                    TestNode::File(bytes) => file_attrs(bytes.len() as u64),
                    TestNode::Symlink(_) => symlink_attrs(),
                };
                File::new(name, attrs)
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.filename.cmp(&right.filename));
        Some(entries)
    }

    fn rename(&mut self, old_path: &str, new_path: &str) -> Result<(), StatusCode> {
        let old_path = canonical_path(old_path);
        let new_path = canonical_path(new_path);
        if old_path == "/" || is_mutation_denied(&old_path) || is_mutation_denied(&new_path) {
            return Err(StatusCode::PermissionDenied);
        }
        if !self.nodes.contains_key(&old_path) {
            return Err(StatusCode::NoSuchFile);
        }
        if self.nodes.contains_key(&new_path)
            || remote_test_is_same_or_descendant(&new_path, &old_path)
        {
            return Err(StatusCode::Failure);
        }
        let parent = test_parent(&new_path).ok_or(StatusCode::Failure)?;
        if !matches!(self.nodes.get(&parent), Some(TestNode::Directory)) {
            return Err(StatusCode::NoSuchFile);
        }
        let moved = self
            .nodes
            .iter()
            .filter(|(path, _)| remote_test_is_same_or_descendant(path, &old_path))
            .map(|(path, node)| {
                let suffix = path.strip_prefix(&old_path).unwrap_or_default();
                (path.clone(), format!("{new_path}{suffix}"), node.clone())
            })
            .collect::<Vec<_>>();
        if moved.iter().any(|(_, destination, _)| {
            self.nodes.contains_key(destination)
                && !moved.iter().any(|(source, _, _)| source == destination)
        }) {
            return Err(StatusCode::Failure);
        }
        for (source, _, _) in &moved {
            self.nodes.remove(source);
        }
        for (_, destination, node) in moved {
            self.nodes.insert(destination, node);
        }
        Ok(())
    }

    fn remove_file(&mut self, path: &str) -> Result<(), StatusCode> {
        let path = canonical_path(path);
        if is_mutation_denied(&path) {
            return Err(StatusCode::PermissionDenied);
        }
        match self.nodes.get(&path) {
            Some(TestNode::Directory) => Err(StatusCode::Failure),
            Some(_) => {
                self.nodes.remove(&path);
                Ok(())
            }
            None => Err(StatusCode::NoSuchFile),
        }
    }

    fn remove_dir(&mut self, path: &str) -> Result<(), StatusCode> {
        let path = canonical_path(path);
        if path == "/" || is_mutation_denied(&path) {
            return Err(StatusCode::PermissionDenied);
        }
        if !matches!(self.nodes.get(&path), Some(TestNode::Directory)) {
            return Err(StatusCode::NoSuchFile);
        }
        if self.nodes.keys().any(|candidate| {
            candidate != &path && remote_test_is_same_or_descendant(candidate, &path)
        }) {
            return Err(StatusCode::Failure);
        }
        self.nodes.remove(&path);
        Ok(())
    }

    fn create_dir(&mut self, path: &str) -> Result<(), StatusCode> {
        let path = canonical_path(path);
        if is_mutation_denied(&path) {
            return Err(StatusCode::PermissionDenied);
        }
        if self.nodes.contains_key(&path) {
            return Err(StatusCode::Failure);
        }
        let parent = test_parent(&path).ok_or(StatusCode::Failure)?;
        if !matches!(self.nodes.get(&parent), Some(TestNode::Directory)) {
            return Err(StatusCode::NoSuchFile);
        }
        self.nodes.insert(path, TestNode::Directory);
        Ok(())
    }

    fn create_symlink(&mut self, link: &str, target: &str) -> Result<(), StatusCode> {
        let link = canonical_path(link);
        if is_mutation_denied(&link) {
            return Err(StatusCode::PermissionDenied);
        }
        if self.nodes.contains_key(&link) {
            return Err(StatusCode::Failure);
        }
        let parent = test_parent(&link).ok_or(StatusCode::Failure)?;
        if !matches!(self.nodes.get(&parent), Some(TestNode::Directory)) {
            return Err(StatusCode::NoSuchFile);
        }
        self.nodes
            .insert(link, TestNode::Symlink(target.to_owned()));
        Ok(())
    }
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
        Self::start_with_delays(auth_mode, sftp_enabled, listing_delay, Duration::ZERO).await
    }

    pub async fn start_with_delays(
        auth_mode: TestAuthMode,
        sftp_enabled: bool,
        listing_delay: Duration,
        mutation_delay: Duration,
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
            mutation_delay,
            mutation_started: Arc::new(Notify::new()),
            filesystem: Arc::new(Mutex::new(TestRemoteFilesystem::default())),
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

    pub async fn wait_for_mutation(&self) {
        self.state.mutation_started.notified().await;
    }

    pub async fn path_exists(&self, path: &str) -> bool {
        self.state
            .filesystem
            .lock()
            .await
            .nodes
            .contains_key(&canonical_path(path))
    }

    pub async fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        match self
            .state
            .filesystem
            .lock()
            .await
            .nodes
            .get(&canonical_path(path))
        {
            Some(TestNode::File(bytes)) => Some(bytes.clone()),
            _ => None,
        }
    }

    pub async fn write_file(&self, path: &str, bytes: Vec<u8>) {
        let path = canonical_path(path);
        let parent = test_parent(&path).expect("test file parent");
        let mut filesystem = self.state.filesystem.lock().await;
        assert!(matches!(
            filesystem.nodes.get(&parent),
            Some(TestNode::Directory)
        ));
        filesystem.nodes.insert(path, TestNode::File(bytes));
    }

    pub async fn create_symlink(&self, path: &str, target: &str) {
        self.state
            .filesystem
            .lock()
            .await
            .create_symlink(path, target)
            .expect("create test symbolic link");
    }

    pub async fn create_dir(&self, path: &str) {
        self.state
            .filesystem
            .lock()
            .await
            .create_dir(path)
            .expect("create test directory");
    }

    pub async fn read_link(&self, path: &str) -> Option<String> {
        match self
            .state
            .filesystem
            .lock()
            .await
            .nodes
            .get(&canonical_path(path))
        {
            Some(TestNode::Symlink(target)) => Some(target.clone()),
            _ => None,
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
            TestSftpHandler::new(
                self.state.listing_delay,
                self.state.mutation_delay,
                self.state.mutation_started.clone(),
                self.state.filesystem.clone(),
            ),
        )
        .await;
        Ok(())
    }
}

struct TestSftpHandler {
    completed_directories: HashSet<String>,
    listing_delay: Duration,
    mutation_delay: Duration,
    mutation_started: Arc<Notify>,
    filesystem: Arc<Mutex<TestRemoteFilesystem>>,
}

impl TestSftpHandler {
    fn new(
        listing_delay: Duration,
        mutation_delay: Duration,
        mutation_started: Arc<Notify>,
        filesystem: Arc<Mutex<TestRemoteFilesystem>>,
    ) -> Self {
        Self {
            completed_directories: HashSet::new(),
            listing_delay,
            mutation_delay,
            mutation_started,
            filesystem,
        }
    }

    async fn before_mutation(&self) {
        self.mutation_started.notify_one();
        if !self.mutation_delay.is_zero() {
            tokio::time::sleep(self.mutation_delay).await;
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

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let path = canonical_path(&filename);
        if is_mutation_denied(&path) && pflags.contains(OpenFlags::WRITE) {
            return Err(StatusCode::PermissionDenied);
        }
        let mut filesystem = self.filesystem.lock().await;
        let exists = filesystem.nodes.contains_key(&path);
        if pflags.contains(OpenFlags::CREATE) {
            if pflags.contains(OpenFlags::EXCLUDE) && exists {
                return Err(StatusCode::Failure);
            }
            if !exists {
                let parent = test_parent(&path).ok_or(StatusCode::Failure)?;
                if !matches!(filesystem.nodes.get(&parent), Some(TestNode::Directory)) {
                    return Err(StatusCode::NoSuchFile);
                }
                filesystem
                    .nodes
                    .insert(path.clone(), TestNode::File(Vec::new()));
            }
        }
        let Some(TestNode::File(bytes)) = filesystem.nodes.get_mut(&path) else {
            return Err(StatusCode::NoSuchFile);
        };
        if pflags.contains(OpenFlags::TRUNCATE) {
            bytes.clear();
        }
        Ok(Handle { id, handle: path })
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let filesystem = self.filesystem.lock().await;
        let Some(TestNode::File(bytes)) = filesystem.nodes.get(&canonical_path(&handle)) else {
            return Err(StatusCode::NoSuchFile);
        };
        let offset = usize::try_from(offset).map_err(|_| StatusCode::Failure)?;
        if offset >= bytes.len() {
            return Err(StatusCode::Eof);
        }
        let end = offset.saturating_add(len as usize).min(bytes.len());
        Ok(Data {
            id,
            data: bytes[offset..end].to_vec(),
        })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        self.before_mutation().await;
        let path = canonical_path(&handle);
        if is_mutation_denied(&path) {
            return Err(StatusCode::PermissionDenied);
        }
        let offset = usize::try_from(offset).map_err(|_| StatusCode::Failure)?;
        let end = offset.checked_add(data.len()).ok_or(StatusCode::Failure)?;
        let mut filesystem = self.filesystem.lock().await;
        let Some(TestNode::File(bytes)) = filesystem.nodes.get_mut(&path) else {
            return Err(StatusCode::NoSuchFile);
        };
        if bytes.len() < end {
            bytes.resize(end, 0);
        }
        bytes[offset..end].copy_from_slice(&data);
        Ok(ok_status(id))
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let path = canonical_path(&handle);
        Ok(Attrs {
            id,
            attrs: self
                .filesystem
                .lock()
                .await
                .metadata(&path, false)
                .ok_or(StatusCode::NoSuchFile)?,
        })
    }

    async fn close(&mut self, id: u32, _handle: String) -> Result<Status, Self::Error> {
        Ok(ok_status(id))
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let path = canonical_path(&path);
        if path == "/private" {
            return Err(StatusCode::PermissionDenied);
        }
        if !matches!(
            self.filesystem.lock().await.nodes.get(&path),
            Some(TestNode::Directory)
        ) {
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
            files: self
                .filesystem
                .lock()
                .await
                .entries(&handle)
                .ok_or(StatusCode::NoSuchFile)?,
        })
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let path = canonical_path(&path);
        if self.filesystem.lock().await.metadata(&path, true).is_none() {
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
            attrs: self
                .filesystem
                .lock()
                .await
                .metadata(&path, true)
                .ok_or(StatusCode::NoSuchFile)?,
        })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let path = canonical_path(&path);
        Ok(Attrs {
            id,
            attrs: self
                .filesystem
                .lock()
                .await
                .metadata(&path, false)
                .ok_or(StatusCode::NoSuchFile)?,
        })
    }

    async fn setstat(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        self.before_mutation().await;
        let path = canonical_path(&path);
        if is_mutation_denied(&path) {
            return Err(StatusCode::PermissionDenied);
        }
        if !self.filesystem.lock().await.nodes.contains_key(&path) {
            return Err(StatusCode::NoSuchFile);
        }
        Ok(ok_status(id))
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        self.before_mutation().await;
        self.filesystem.lock().await.rename(&oldpath, &newpath)?;
        Ok(ok_status(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        self.before_mutation().await;
        self.filesystem.lock().await.remove_file(&filename)?;
        Ok(ok_status(id))
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        self.before_mutation().await;
        self.filesystem.lock().await.create_dir(&path)?;
        Ok(ok_status(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        self.before_mutation().await;
        self.filesystem.lock().await.remove_dir(&path)?;
        Ok(ok_status(id))
    }

    async fn readlink(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let path = canonical_path(&path);
        let filesystem = self.filesystem.lock().await;
        let Some(TestNode::Symlink(target)) = filesystem.nodes.get(&path) else {
            return Err(StatusCode::NoSuchFile);
        };
        Ok(Name {
            id,
            files: vec![File::dummy(target.clone())],
        })
    }

    async fn symlink(
        &mut self,
        id: u32,
        linkpath: String,
        targetpath: String,
    ) -> Result<Status, Self::Error> {
        self.before_mutation().await;
        self.filesystem
            .lock()
            .await
            .create_symlink(&linkpath, &targetpath)?;
        Ok(ok_status(id))
    }
}

fn canonical_path(path: &str) -> String {
    match path.trim_end_matches('/') {
        "" | "." => "/".to_owned(),
        path if path.starts_with('/') => path.to_owned(),
        path => format!("/{path}"),
    }
}

fn test_parent(path: &str) -> Option<String> {
    let path = canonical_path(path);
    if path == "/" {
        return None;
    }
    let parent = path
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("");
    Some(if parent.is_empty() { "/" } else { parent }.to_owned())
}

fn remote_test_is_same_or_descendant(path: &str, ancestor: &str) -> bool {
    path == ancestor
        || (ancestor == "/" && path.starts_with('/'))
        || path
            .strip_prefix(ancestor)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn is_mutation_denied(path: &str) -> bool {
    path == "/locked.txt"
        || path.ends_with("/locked.txt")
        || remote_test_is_same_or_descendant(path, "/private")
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
