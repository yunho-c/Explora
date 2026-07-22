use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use tokio::sync::{oneshot, OwnedSemaphorePermit, Semaphore};

use crate::filesystem::ExplorerError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContentRequestPolicy {
    ICloud,
    WindowsCloudFiles,
}

trait ContentRequestStarter: Send + Sync {
    /// Starts an operating-system-owned content request. Returning does not
    /// imply that the bytes are local; callers must revalidate availability.
    fn start(&self, policy: ContentRequestPolicy, path: &Path) -> Result<(), ExplorerError>;
}

#[derive(Default)]
struct SystemContentRequestStarter;

pub(crate) struct ContentRequestManager {
    requests: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    starter: Arc<dyn ContentRequestStarter>,
    limiter: Arc<Semaphore>,
}

const MAX_CONCURRENT_CONTENT_REQUESTS: usize = 4;

impl Default for ContentRequestManager {
    fn default() -> Self {
        Self {
            requests: Arc::new(Mutex::new(HashMap::new())),
            starter: Arc::new(SystemContentRequestStarter),
            limiter: Arc::new(Semaphore::new(MAX_CONCURRENT_CONTENT_REQUESTS)),
        }
    }
}

impl ContentRequestManager {
    pub(crate) fn begin(
        &self,
        request_id: String,
        policy: ContentRequestPolicy,
        path: PathBuf,
    ) -> Result<ActiveContentRequest, ExplorerError> {
        let permit = self.limiter.clone().try_acquire_owned().map_err(|_| {
            ExplorerError::Unexpected(
                "Too many files are already being prepared for preview.".to_owned(),
            )
        })?;
        let cancellation = Arc::new(AtomicBool::new(false));
        let mut requests = self
            .requests
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        if requests.contains_key(&request_id) {
            return Err(ExplorerError::InvalidReference);
        }
        requests.insert(request_id.clone(), cancellation.clone());
        drop(requests);

        let starter = self.starter.clone();
        let (provider_sender, provider_result) = oneshot::channel();
        tauri::async_runtime::spawn_blocking(move || {
            let _ = provider_sender.send(starter.start(policy, path.as_path()));
        });

        Ok(ActiveContentRequest {
            request_id,
            cancellation,
            provider_result,
            requests: self.requests.clone(),
            provider_result_received: false,
            _permit: permit,
        })
    }

    pub(crate) fn cancel(&self, request_id: &str) -> Result<(), ExplorerError> {
        if let Some(cancellation) = self
            .requests
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .get(request_id)
        {
            cancellation.store(true, Ordering::Release);
        }
        Ok(())
    }
}

pub(crate) struct ActiveContentRequest {
    request_id: String,
    cancellation: Arc<AtomicBool>,
    provider_result: oneshot::Receiver<Result<(), ExplorerError>>,
    requests: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    provider_result_received: bool,
    _permit: OwnedSemaphorePermit,
}

impl ActiveContentRequest {
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancellation.load(Ordering::Acquire)
    }

    pub(crate) fn take_provider_result(&mut self) -> Option<Result<(), ExplorerError>> {
        if self.provider_result_received {
            return None;
        }
        match self.provider_result.try_recv() {
            Ok(result) => {
                self.provider_result_received = true;
                Some(result)
            }
            Err(oneshot::error::TryRecvError::Empty) => None,
            Err(oneshot::error::TryRecvError::Closed) => {
                self.provider_result_received = true;
                Some(Err(ExplorerError::Unexpected(
                    "The operating system content request stopped unexpectedly.".to_owned(),
                )))
            }
        }
    }
}

impl Drop for ActiveContentRequest {
    fn drop(&mut self) {
        if let Ok(mut requests) = self.requests.lock() {
            if requests
                .get(&self.request_id)
                .is_some_and(|registered| Arc::ptr_eq(registered, &self.cancellation))
            {
                requests.remove(&self.request_id);
            }
        }
    }
}

impl ContentRequestStarter for SystemContentRequestStarter {
    fn start(&self, policy: ContentRequestPolicy, path: &Path) -> Result<(), ExplorerError> {
        match policy {
            ContentRequestPolicy::ICloud => start_icloud_download(path),
            ContentRequestPolicy::WindowsCloudFiles => start_windows_hydration(path),
        }
    }
}

#[cfg(target_os = "macos")]
fn start_icloud_download(path: &Path) -> Result<(), ExplorerError> {
    use objc2_foundation::{NSFileManager, NSURL};

    let url = NSURL::from_path(path, false, None).ok_or_else(|| {
        ExplorerError::InvalidConfiguration(
            "macOS could not represent the selected iCloud item.".to_owned(),
        )
    })?;
    let manager = NSFileManager::defaultManager();
    if !manager.isUbiquitousItemAtURL(&url) {
        return Err(ExplorerError::Unsupported(
            "This item is not managed by iCloud Drive.".to_owned(),
        ));
    }
    manager
        .startDownloadingUbiquitousItemAtURL_error(&url)
        .map_err(|_| {
            ExplorerError::Unexpected(
                "macOS could not start downloading this iCloud item.".to_owned(),
            )
        })
}

#[cfg(not(target_os = "macos"))]
fn start_icloud_download(_path: &Path) -> Result<(), ExplorerError> {
    Err(ExplorerError::Unsupported(
        "iCloud content requests are available only on macOS.".to_owned(),
    ))
}

#[cfg(target_os = "windows")]
fn start_windows_hydration(path: &Path) -> Result<(), ExplorerError> {
    use std::os::windows::ffi::OsStrExt;

    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::{CloseHandle, HANDLE},
            Storage::{
                CloudFilters::{CfHydratePlaceholder, CF_HYDRATE_FLAG_NONE},
                FileSystem::{
                    CreateFileW, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
                    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
                },
            },
        },
    };

    struct FileHandle(HANDLE);

    impl Drop for FileHandle {
        fn drop(&mut self) {
            // SAFETY: The handle came from a successful CreateFileW call and
            // is closed exactly once by this guard.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    let mut wide_path = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if wide_path.is_empty() || wide_path.contains(&0) {
        return Err(ExplorerError::InvalidConfiguration(
            "Windows could not represent the selected cloud item.".to_owned(),
        ));
    }
    wide_path.push(0);

    // The Cloud Files API accepts an attribute-only handle. Opening the reparse
    // point itself avoids reading content before the explicit hydrate request.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide_path.as_ptr()),
            FILE_READ_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            None,
        )
    }
    .map(FileHandle)
    .map_err(|_| {
        ExplorerError::Unexpected(
            "Windows could not open this cloud placeholder for download.".to_owned(),
        )
    })?;

    // CF_EOF is documented as -1. The synchronous call runs on a blocking
    // worker so cancelling Explora's wait never blocks the async runtime.
    unsafe { CfHydratePlaceholder(handle.0, 0, -1, CF_HYDRATE_FLAG_NONE, None) }.map_err(|_| {
        ExplorerError::Unexpected("Windows could not download this cloud placeholder.".to_owned())
    })
}

#[cfg(not(target_os = "windows"))]
fn start_windows_hydration(_path: &Path) -> Result<(), ExplorerError> {
    Err(ExplorerError::Unsupported(
        "Cloud Files content requests are available only on Windows.".to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{mpsc, Mutex},
        time::Duration,
    };

    use super::*;

    struct RecordingStarter {
        calls: Mutex<Vec<(ContentRequestPolicy, PathBuf)>>,
        gate: Mutex<mpsc::Receiver<()>>,
    }

    struct FailingStarter;

    impl ContentRequestStarter for FailingStarter {
        fn start(&self, _policy: ContentRequestPolicy, _path: &Path) -> Result<(), ExplorerError> {
            Err(ExplorerError::Offline(
                "The provider is offline.".to_owned(),
            ))
        }
    }

    impl ContentRequestStarter for RecordingStarter {
        fn start(&self, policy: ContentRequestPolicy, path: &Path) -> Result<(), ExplorerError> {
            self.calls
                .lock()
                .expect("calls lock")
                .push((policy, path.to_path_buf()));
            self.gate
                .lock()
                .expect("gate lock")
                .recv_timeout(Duration::from_secs(2))
                .map_err(|_| ExplorerError::TimedOut("fixture timed out".to_owned()))
        }
    }

    fn manager_with_starter(starter: Arc<dyn ContentRequestStarter>) -> ContentRequestManager {
        ContentRequestManager {
            requests: Arc::new(Mutex::new(HashMap::new())),
            starter,
            limiter: Arc::new(Semaphore::new(MAX_CONCURRENT_CONTENT_REQUESTS)),
        }
    }

    #[test]
    fn cancellation_stops_waiting_without_reusing_the_request_id() {
        let (_gate_sender, gate_receiver) = mpsc::channel();
        let starter = Arc::new(RecordingStarter {
            calls: Mutex::new(Vec::new()),
            gate: Mutex::new(gate_receiver),
        });
        let manager = manager_with_starter(starter);
        let request = manager
            .begin(
                "content-1".to_owned(),
                ContentRequestPolicy::ICloud,
                PathBuf::from("fixture"),
            )
            .expect("begin request");

        manager.cancel("content-1").expect("cancel request");
        assert!(request.is_cancelled());
        assert!(matches!(
            manager.begin(
                "content-1".to_owned(),
                ContentRequestPolicy::ICloud,
                PathBuf::from("fixture"),
            ),
            Err(ExplorerError::InvalidReference)
        ));
    }

    #[test]
    fn dropping_the_wait_handle_allows_a_new_request_while_provider_work_finishes() {
        let (gate_sender, gate_receiver) = mpsc::channel();
        let starter = Arc::new(RecordingStarter {
            calls: Mutex::new(Vec::new()),
            gate: Mutex::new(gate_receiver),
        });
        let manager = manager_with_starter(starter.clone());
        let request = manager
            .begin(
                "content-1".to_owned(),
                ContentRequestPolicy::WindowsCloudFiles,
                PathBuf::from("placeholder"),
            )
            .expect("begin request");
        drop(request);

        let next = manager
            .begin(
                "content-1".to_owned(),
                ContentRequestPolicy::WindowsCloudFiles,
                PathBuf::from("placeholder"),
            )
            .expect("reuse request ID after wait ended");
        gate_sender.send(()).expect("release first worker");
        gate_sender.send(()).expect("release second worker");

        for _ in 0..50 {
            if starter.calls.lock().expect("calls lock").len() == 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(starter.calls.lock().expect("calls lock").len(), 2);
        drop(next);
    }

    #[test]
    fn provider_start_failures_cross_the_worker_boundary_as_structured_errors() {
        let manager = manager_with_starter(Arc::new(FailingStarter));
        let mut request = manager
            .begin(
                "content-failure".to_owned(),
                ContentRequestPolicy::ICloud,
                PathBuf::from("placeholder"),
            )
            .expect("begin request");

        let result = (0..100).find_map(|_| {
            let result = request.take_provider_result();
            if result.is_none() {
                std::thread::sleep(Duration::from_millis(10));
            }
            result
        });
        assert!(matches!(
            result,
            Some(Err(ExplorerError::Offline(message))) if message == "The provider is offline."
        ));
    }

    #[test]
    fn active_waits_are_bounded_even_with_distinct_request_ids() {
        let (gate_sender, gate_receiver) = mpsc::channel();
        let starter = Arc::new(RecordingStarter {
            calls: Mutex::new(Vec::new()),
            gate: Mutex::new(gate_receiver),
        });
        let manager = ContentRequestManager {
            requests: Arc::new(Mutex::new(HashMap::new())),
            starter,
            limiter: Arc::new(Semaphore::new(1)),
        };
        let first = manager
            .begin(
                "content-1".to_owned(),
                ContentRequestPolicy::ICloud,
                PathBuf::from("one"),
            )
            .expect("first request");
        assert!(matches!(
            manager.begin(
                "content-2".to_owned(),
                ContentRequestPolicy::ICloud,
                PathBuf::from("two"),
            ),
            Err(ExplorerError::Unexpected(message))
                if message.contains("Too many files")
        ));

        drop(first);
        let second = manager
            .begin(
                "content-2".to_owned(),
                ContentRequestPolicy::ICloud,
                PathBuf::from("two"),
            )
            .expect("permit released after wait ends");
        gate_sender.send(()).expect("release first worker");
        gate_sender.send(()).expect("release second worker");
        drop(second);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_adapter_does_not_treat_an_ordinary_local_file_as_icloud_content() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let path = temp.path().join("local.txt");
        std::fs::write(&path, b"local").expect("local fixture");

        assert!(matches!(
            start_icloud_download(&path),
            Err(ExplorerError::Unsupported(message))
                if message == "This item is not managed by iCloud Drive."
        ));
    }
}
