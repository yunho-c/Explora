use std::{
    collections::HashMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use serde::Serialize;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use crate::filesystem::ExplorerError;

pub const LARGE_REMOTE_OPEN_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_REMOTE_OPEN_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_OPEN_REQUESTS: usize = 128;
const MAX_CONCURRENT_REMOTE_OPENS: usize = 2;

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "phase",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum NativeOpenEventDto {
    Queued,
    Downloading {
        transferred_bytes: String,
        total_bytes: Option<String>,
    },
    Launching,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum NativeOpenOutcomeDto {
    Opened,
    ConfirmationRequired { size: Option<String> },
}

pub trait NativeFileOpener: Send + Sync {
    fn open(&self, path: &Path) -> Result<(), ExplorerError>;
}

#[derive(Default)]
pub struct SystemNativeFileOpener;

impl NativeFileOpener for SystemNativeFileOpener {
    fn open(&self, path: &Path) -> Result<(), ExplorerError> {
        tauri_plugin_opener::open_path(path, None::<&str>).map_err(|_| {
            ExplorerError::Unsupported(
                "The operating system could not open this item with its default application."
                    .to_owned(),
            )
        })
    }
}

pub struct NativeOpenManager {
    requests: Mutex<HashMap<String, Arc<AtomicBool>>>,
    download_slots: Arc<Semaphore>,
    session_directory: PathBuf,
    opener: Arc<dyn NativeFileOpener>,
    startup_warning: Option<String>,
}

impl NativeOpenManager {
    pub fn new(cache_root: PathBuf) -> Result<Self, ExplorerError> {
        Self::with_opener(cache_root, Arc::new(SystemNativeFileOpener))
    }

    fn with_opener(
        cache_root: PathBuf,
        opener: Arc<dyn NativeFileOpener>,
    ) -> Result<Self, ExplorerError> {
        fs::create_dir_all(&cache_root).map_err(|error| {
            ExplorerError::io("create its native-open cache", &cache_root, error)
        })?;
        set_private_directory_permissions(&cache_root)?;

        let mut cleanup_failed = false;
        let entries = fs::read_dir(&cache_root).map_err(|error| {
            ExplorerError::io("inspect its native-open cache", &cache_root, error)
        })?;
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir())
                && fs::remove_dir_all(entry.path()).is_err()
            {
                cleanup_failed = true;
            }
        }

        let session_directory = cache_root.join(Uuid::new_v4().to_string());
        fs::create_dir(&session_directory).map_err(|error| {
            ExplorerError::io("create its native-open session", &session_directory, error)
        })?;
        set_private_directory_permissions(&session_directory)?;

        Ok(Self {
            requests: Mutex::new(HashMap::new()),
            download_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_REMOTE_OPENS)),
            session_directory,
            opener,
            startup_warning: cleanup_failed.then(|| {
                "Some temporary remote-file snapshots could not be removed. Explora will retry next time it starts."
                    .to_owned()
            }),
        })
    }

    pub fn startup_warning(&self) -> Option<String> {
        self.startup_warning.clone()
    }

    pub fn begin(&self, request_id: &str) -> Result<Arc<AtomicBool>, ExplorerError> {
        if request_id.is_empty() || request_id.len() > 128 {
            return Err(ExplorerError::InvalidReference);
        }
        let mut requests = self
            .requests
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        if requests.len() >= MAX_OPEN_REQUESTS || requests.contains_key(request_id) {
            return Err(ExplorerError::InvalidReference);
        }
        let cancellation = Arc::new(AtomicBool::new(false));
        requests.insert(request_id.to_owned(), cancellation.clone());
        Ok(cancellation)
    }

    pub fn finish(&self, request_id: &str) {
        if let Ok(mut requests) = self.requests.lock() {
            requests.remove(request_id);
        }
    }

    pub fn cancel(&self, request_id: &str) -> Result<(), ExplorerError> {
        if let Some(cancellation) = self
            .requests
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .get(request_id)
        {
            cancellation.store(true, Ordering::Relaxed);
        }
        Ok(())
    }

    pub async fn acquire_download_slot(
        &self,
        cancellation: &AtomicBool,
    ) -> Result<OwnedSemaphorePermit, ExplorerError> {
        if cancellation.load(Ordering::Relaxed) {
            return Err(ExplorerError::Cancelled);
        }
        tokio::select! {
            permit = self.download_slots.clone().acquire_owned() => {
                permit.map_err(|_| ExplorerError::StateUnavailable)
            }
            () = wait_for_cancellation(cancellation) => Err(ExplorerError::Cancelled),
        }
    }

    pub fn destination(&self, name: &str) -> Result<(PathBuf, PathBuf), ExplorerError> {
        let directory = self.session_directory.join(Uuid::new_v4().to_string());
        fs::create_dir(&directory).map_err(|error| {
            ExplorerError::io("prepare a remote-file snapshot", &directory, error)
        })?;
        set_private_directory_permissions(&directory)?;
        let final_path = directory.join(safe_snapshot_name(name));
        let partial_path = directory.join("download.partial");
        Ok((partial_path, final_path))
    }

    pub fn finalize_remote_snapshot(
        &self,
        partial_path: &Path,
        final_path: &Path,
        executable: bool,
    ) -> Result<(), ExplorerError> {
        fs::rename(partial_path, final_path).map_err(|error| {
            ExplorerError::io("finalize a remote-file snapshot", final_path, error)
        })?;
        mark_remote_origin(final_path)?;
        set_snapshot_permissions(final_path, executable)?;
        Ok(())
    }

    pub fn discard_snapshot(&self, path: &Path) {
        let directory = path.parent().unwrap_or(path);
        let _ = fs::remove_dir_all(directory);
    }

    pub fn open(&self, path: &Path) -> Result<(), ExplorerError> {
        self.opener.open(path)
    }
}

async fn wait_for_cancellation(cancelled: &AtomicBool) {
    while !cancelled.load(Ordering::Relaxed) {
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
}

fn safe_snapshot_name(name: &str) -> String {
    let basename = Path::new(name)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("remote-file");
    let filtered = basename
        .chars()
        .map(|character| {
            if character.is_control() || matches!(character, '/' | '\\' | ':') {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    let trimmed = filtered.trim_matches(['.', ' ']);
    if trimmed.is_empty() || trimmed.len() > 240 {
        "remote-file".to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<(), ExplorerError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| ExplorerError::io("secure a temporary directory", path, error))
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<(), ExplorerError> {
    Ok(())
}

#[cfg(unix)]
fn set_snapshot_permissions(path: &Path, executable: bool) -> Result<(), ExplorerError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if executable { 0o500 } else { 0o400 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| ExplorerError::io("secure a remote-file snapshot", path, error))
}

#[cfg(windows)]
fn set_snapshot_permissions(path: &Path, _executable: bool) -> Result<(), ExplorerError> {
    let mut permissions = fs::metadata(path)
        .map_err(|error| ExplorerError::io("inspect a remote-file snapshot", path, error))?
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions)
        .map_err(|error| ExplorerError::io("secure a remote-file snapshot", path, error))
}

#[cfg(not(any(unix, windows)))]
fn set_snapshot_permissions(_path: &Path, _executable: bool) -> Result<(), ExplorerError> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn mark_remote_origin(path: &Path) -> Result<(), ExplorerError> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let path =
        CString::new(path.as_os_str().as_bytes()).map_err(|_| ExplorerError::InvalidReference)?;
    let name = c"com.apple.quarantine";
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let value = format!("0083;{timestamp:x};Explora;{}", Uuid::new_v4());
    // SAFETY: all pointers remain valid for the duration of the call and their
    // lengths match the byte slices passed to the operating system.
    let result = unsafe {
        libc::setxattr(
            path.as_ptr(),
            name.as_ptr(),
            value.as_bytes().as_ptr().cast(),
            value.len(),
            0,
            0,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(ExplorerError::Io {
            message: "Explora could not mark the downloaded file as remote content.".to_owned(),
            kind: std::io::Error::last_os_error().kind(),
        })
    }
}

#[cfg(windows)]
fn mark_remote_origin(path: &Path) -> Result<(), ExplorerError> {
    use std::io::Write;
    let mut zone_name = path.as_os_str().to_os_string();
    zone_name.push(":Zone.Identifier");
    let zone_path = PathBuf::from(zone_name);
    let mut zone = fs::File::create(&zone_path)
        .map_err(|error| ExplorerError::io("mark downloaded content as remote", path, error))?;
    zone.write_all(b"[ZoneTransfer]\r\nZoneId=3\r\n")
        .map_err(|error| ExplorerError::io("mark downloaded content as remote", path, error))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn mark_remote_origin(_path: &Path) -> Result<(), ExplorerError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RecordingOpener(Mutex<Vec<PathBuf>>);

    impl NativeFileOpener for RecordingOpener {
        fn open(&self, path: &Path) -> Result<(), ExplorerError> {
            self.0
                .lock()
                .expect("recording opener")
                .push(path.to_owned());
            Ok(())
        }
    }

    #[test]
    fn snapshot_names_cannot_escape_the_owned_directory() {
        assert_eq!(safe_snapshot_name("../../report.pdf"), "report.pdf");
        assert_eq!(safe_snapshot_name(".."), "remote-file");
        assert_eq!(safe_snapshot_name("a:b.txt"), "a_b.txt");
    }

    #[test]
    fn opener_receives_only_the_resolved_path() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let opener = Arc::new(RecordingOpener(Mutex::new(Vec::new())));
        let manager = NativeOpenManager::with_opener(temporary.path().join("open"), opener.clone())
            .expect("manager");
        let path = temporary.path().join("document.txt");
        manager.open(&path).expect("open");
        assert_eq!(
            opener.0.lock().expect("recording opener").as_slice(),
            &[path]
        );
    }

    #[test]
    fn startup_removes_snapshots_from_the_previous_session() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let cache = temporary.path().join("native-open");
        let previous_session = cache.join("previous-session");
        fs::create_dir_all(&previous_session).expect("previous session directory");
        fs::write(previous_session.join("report.pdf"), b"snapshot").expect("previous snapshot");

        let manager = NativeOpenManager::with_opener(
            cache,
            Arc::new(RecordingOpener(Mutex::new(Vec::new()))),
        )
        .expect("manager");

        assert!(!previous_session.exists());
        assert!(manager.session_directory.exists());
    }

    #[tokio::test]
    async fn remote_open_downloads_are_limited_to_two_concurrent_slots() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let manager = NativeOpenManager::with_opener(
            temporary.path().join("open"),
            Arc::new(RecordingOpener(Mutex::new(Vec::new()))),
        )
        .expect("manager");
        let cancellation = AtomicBool::new(false);
        let first = manager
            .acquire_download_slot(&cancellation)
            .await
            .expect("first slot");
        let second = manager
            .acquire_download_slot(&cancellation)
            .await
            .expect("second slot");
        let third = manager.acquire_download_slot(&cancellation);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), third)
                .await
                .is_err()
        );

        drop(first);
        let third = manager
            .acquire_download_slot(&cancellation)
            .await
            .expect("released slot");
        drop((second, third));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn finalized_macos_snapshots_are_quarantined_and_read_only() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().expect("temporary directory");
        let manager = NativeOpenManager::with_opener(
            temporary.path().join("open"),
            Arc::new(RecordingOpener(Mutex::new(Vec::new()))),
        )
        .expect("manager");
        let (partial, finalized) = manager.destination("report.pdf").expect("destination");
        fs::write(&partial, b"snapshot").expect("partial snapshot");

        manager
            .finalize_remote_snapshot(&partial, &finalized, false)
            .expect("finalized snapshot");

        assert!(!partial.exists());
        assert!(finalized.exists());
        assert_eq!(
            fs::metadata(&finalized)
                .expect("snapshot metadata")
                .permissions()
                .mode()
                & 0o777,
            0o400
        );
    }
}
