use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use serde::Serialize;
use tauri::ipc::Channel;
use uuid::Uuid;

use crate::{
    filesystem::{
        ExplorerError, LocationSummaryDto, SyncedFolderMetadataDto, SyncedFolderProvider,
        SyncedFolderStatus,
    },
    local_filesystem::{LocalFilesystem, SyncedFolderRoot},
};

const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const SYNCED_FOLDER_NAMESPACE: Uuid = Uuid::from_u128(0xdf7e7d36_18a1_44bf_8241_ea6deac9d0d1);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncedFolderSnapshotEventDto {
    pub revision: u64,
    pub folders: Vec<LocationSummaryDto>,
    pub warning: Option<String>,
}

trait SyncedFolderDiscovery: Send + Sync + 'static {
    fn discover(&self) -> Result<Vec<SyncedFolderRoot>, ExplorerError>;
}

struct SystemSyncedFolderDiscovery {
    home_dir: PathBuf,
}

impl SystemSyncedFolderDiscovery {
    fn new(home_dir: PathBuf) -> Self {
        Self { home_dir }
    }
}

impl SyncedFolderDiscovery for SystemSyncedFolderDiscovery {
    fn discover(&self) -> Result<Vec<SyncedFolderRoot>, ExplorerError> {
        #[cfg(target_os = "macos")]
        {
            discover_macos_roots(&self.home_dir)
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = &self.home_dir;
            Ok(Vec::new())
        }
    }
}

struct SyncedFolderState {
    revision: u64,
    roots: Vec<SyncedFolderRoot>,
    summaries: Vec<LocationSummaryDto>,
    warning: Option<String>,
}

pub struct SyncedFolderManager {
    filesystem: Arc<LocalFilesystem>,
    discovery: Arc<dyn SyncedFolderDiscovery>,
    state: Mutex<SyncedFolderState>,
    subscribers: Mutex<HashMap<String, Channel<SyncedFolderSnapshotEventDto>>>,
    stopped: Arc<AtomicBool>,
}

impl SyncedFolderManager {
    pub fn start(
        filesystem: Arc<LocalFilesystem>,
        home_dir: PathBuf,
    ) -> Result<Arc<Self>, ExplorerError> {
        let stopped = Arc::new(AtomicBool::new(false));
        let manager = Arc::new(Self {
            filesystem,
            discovery: Arc::new(SystemSyncedFolderDiscovery::new(home_dir)),
            state: Mutex::new(SyncedFolderState {
                revision: 0,
                roots: Vec::new(),
                summaries: Vec::new(),
                warning: None,
            }),
            subscribers: Mutex::new(HashMap::new()),
            stopped,
        });
        manager.refresh()?;

        let weak = Arc::downgrade(&manager);
        thread::Builder::new()
            .name("explora-synced-folder-watch".to_owned())
            .spawn(move || {
                while let Some(manager) = weak.upgrade() {
                    if manager.stopped.load(Ordering::Relaxed) {
                        break;
                    }
                    thread::sleep(REFRESH_INTERVAL);
                    if manager.stopped.load(Ordering::Relaxed) {
                        break;
                    }
                    let _ = manager.refresh();
                }
            })
            .map_err(|error| {
                ExplorerError::Unexpected(format!(
                    "Explora could not start synced-folder discovery: {error}"
                ))
            })?;
        Ok(manager)
    }

    pub fn subscribe(
        &self,
        request_id: String,
        channel: Channel<SyncedFolderSnapshotEventDto>,
    ) -> Result<(), ExplorerError> {
        // Holding the state lock until the subscriber is registered prevents a
        // refresh from landing between the initial snapshot and subscription.
        let state = self
            .state
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let event = SyncedFolderSnapshotEventDto {
            revision: state.revision,
            folders: state.summaries.clone(),
            warning: state.warning.clone(),
        };
        let mut subscribers = self
            .subscribers
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        channel
            .send(event)
            .map_err(|_| ExplorerError::ChannelClosed)?;
        subscribers.insert(request_id, channel);
        Ok(())
    }

    pub fn unsubscribe(&self, request_id: &str) -> Result<(), ExplorerError> {
        self.subscribers
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .remove(request_id);
        Ok(())
    }

    fn refresh(&self) -> Result<(), ExplorerError> {
        let roots = match self.discovery.discover() {
            Ok(roots) => roots,
            Err(error) => {
                let mut state = self
                    .state
                    .lock()
                    .map_err(|_| ExplorerError::StateUnavailable)?;
                let warning = Some(format!("Explora could not refresh synced folders: {error}"));
                if state.warning != warning {
                    state.warning = warning;
                    state.revision = state.revision.saturating_add(1);
                    let event = SyncedFolderSnapshotEventDto {
                        revision: state.revision,
                        folders: state.summaries.clone(),
                        warning: state.warning.clone(),
                    };
                    drop(state);
                    self.broadcast(event)?;
                }
                return Ok(());
            }
        };

        let mut state = self
            .state
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        if state.roots == roots && state.warning.is_none() {
            return Ok(());
        }
        let summaries = self.filesystem.replace_synced_folders(roots.clone())?;
        state.roots = roots;
        state.summaries = summaries;
        state.warning = None;
        state.revision = state.revision.saturating_add(1);
        let event = SyncedFolderSnapshotEventDto {
            revision: state.revision,
            folders: state.summaries.clone(),
            warning: None,
        };
        drop(state);
        self.broadcast(event)
    }

    fn broadcast(&self, event: SyncedFolderSnapshotEventDto) -> Result<(), ExplorerError> {
        let mut subscribers = self
            .subscribers
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        subscribers.retain(|_, channel| channel.send(event.clone()).is_ok());
        Ok(())
    }
}

impl Drop for SyncedFolderManager {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}

fn discover_macos_roots(home_dir: &Path) -> Result<Vec<SyncedFolderRoot>, ExplorerError> {
    let mut candidates = Vec::<(PathBuf, SyncedFolderProvider)>::new();
    let icloud = home_dir.join("Library/Mobile Documents/com~apple~CloudDocs");
    if directory_without_following_symlinks(&icloud)? {
        candidates.push((icloud, SyncedFolderProvider::ICloud));
    }

    let cloud_storage = home_dir.join("Library/CloudStorage");
    match fs::read_dir(&cloud_storage) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.map_err(|error| {
                    discovery_io("read an operating-system cloud-storage entry", error)
                })?;
                if !entry
                    .file_type()
                    .map_err(|error| discovery_io("inspect a cloud-storage entry", error))?
                    .is_dir()
                {
                    continue;
                }
                let provider = provider_from_name(&entry.file_name());
                candidates.push((entry.path(), provider));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(discovery_io("read operating-system cloud storage", error));
        }
    }

    candidates.sort_by(|(left_path, left_provider), (right_path, right_provider)| {
        left_provider
            .cmp(right_provider)
            .then_with(|| path_identity(left_path).cmp(&path_identity(right_path)))
    });
    candidates.dedup_by(|(left, _), (right, _)| left == right);

    let provider_counts = candidates
        .iter()
        .fold(BTreeMap::new(), |mut counts, (_, provider)| {
            *counts.entry(*provider).or_insert(0_usize) += 1;
            counts
        });
    let mut provider_indexes = BTreeMap::<SyncedFolderProvider, usize>::new();
    let mut seen_ids = HashSet::new();
    let mut roots = Vec::with_capacity(candidates.len());
    for (path, provider) in candidates {
        let id = format!(
            "synced:{}",
            Uuid::new_v5(&SYNCED_FOLDER_NAMESPACE, &path_identity(&path))
        );
        if !seen_ids.insert(id.clone()) {
            continue;
        }
        let index = provider_indexes.entry(provider).or_insert(0);
        *index += 1;
        let base_name = provider.display_name();
        let name = if provider_counts.get(&provider).copied().unwrap_or(0) > 1 {
            format!("{base_name} {}", *index)
        } else {
            base_name.to_owned()
        };
        roots.push(SyncedFolderRoot {
            id,
            name,
            path,
            detail: format!("{} · Synced folder", provider.display_name()),
            metadata: SyncedFolderMetadataDto {
                provider,
                status: SyncedFolderStatus::Available,
            },
        });
    }
    Ok(roots)
}

fn directory_without_following_symlinks(path: &Path) -> Result<bool, ExplorerError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_dir()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(discovery_io("inspect a synced-folder root", error)),
    }
}

fn discovery_io(action: &str, error: std::io::Error) -> ExplorerError {
    ExplorerError::Io {
        message: format!("Explora could not {action}: {error}"),
        kind: error.kind(),
    }
}

fn provider_from_name(name: &OsStr) -> SyncedFolderProvider {
    let normalized = name.to_string_lossy().to_lowercase();
    if normalized.starts_with("onedrive") {
        SyncedFolderProvider::OneDrive
    } else if normalized.starts_with("google") {
        SyncedFolderProvider::GoogleDrive
    } else {
        SyncedFolderProvider::Other
    }
}

#[cfg(unix)]
fn path_identity(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;

    path.as_os_str().as_bytes().to_vec()
}

#[cfg(windows)]
fn path_identity(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;

    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn discovers_and_sanitizes_macos_provider_roots() {
        let temp = TempDir::new().expect("temporary home");
        let cloud_storage = temp.path().join("Library/CloudStorage");
        fs::create_dir_all(cloud_storage.join("OneDrive-person@example.com"))
            .expect("first OneDrive root");
        fs::create_dir_all(cloud_storage.join("OneDrive-Work Tenant"))
            .expect("second OneDrive root");
        fs::create_dir_all(cloud_storage.join("GoogleDrive-user@example.com"))
            .expect("Google Drive root");
        fs::create_dir_all(
            temp.path()
                .join("Library/Mobile Documents/com~apple~CloudDocs"),
        )
        .expect("iCloud root");

        let roots = discover_macos_roots(temp.path()).expect("synced folders");
        let names = roots
            .iter()
            .map(|root| root.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec!["iCloud Drive", "OneDrive 1", "OneDrive 2", "Google Drive"]
        );
        assert!(roots.iter().all(|root| !root.name.contains('@')));
        assert!(roots.iter().all(|root| root.id.starts_with("synced:")));
    }

    #[cfg(unix)]
    #[test]
    fn ignores_symlinked_roots() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temporary home");
        let cloud_storage = temp.path().join("Library/CloudStorage");
        let target = temp.path().join("target");
        fs::create_dir_all(&cloud_storage).expect("cloud storage");
        fs::create_dir(&target).expect("target");
        symlink(&target, cloud_storage.join("OneDrive-linked")).expect("symlinked provider root");

        assert!(discover_macos_roots(temp.path())
            .expect("synced folders")
            .is_empty());
    }

    #[test]
    fn provider_names_are_hints_only() {
        assert_eq!(
            provider_from_name(OsStr::new("OneDrive-Example")),
            SyncedFolderProvider::OneDrive
        );
        assert_eq!(
            provider_from_name(OsStr::new("GoogleDrive-example")),
            SyncedFolderProvider::GoogleDrive
        );
        assert_eq!(
            provider_from_name(OsStr::new("Acme Cloud")),
            SyncedFolderProvider::Other
        );
    }

    #[test]
    fn discovery_errors_do_not_expose_provider_paths_or_accounts() {
        let error = discovery_io(
            "inspect a synced-folder root",
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "operation not permitted",
            ),
        )
        .to_string();

        assert_eq!(
            error,
            "Explora could not inspect a synced-folder root: operation not permitted"
        );
        assert!(!error.contains("CloudStorage"));
        assert!(!error.contains('@'));
    }
}
