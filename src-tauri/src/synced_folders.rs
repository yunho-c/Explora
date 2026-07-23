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
        SyncedFolderSource, SyncedFolderStatus,
    },
    gio_filesystem::GioFilesystem,
    local_filesystem::{LocalFilesystem, SyncedFolderRoot},
    manual_synced_folders::ManualSyncedFolderStore,
    synced_availability::SyncedAvailabilityPolicy,
};

const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const SYNCED_FOLDER_NAMESPACE: Uuid = Uuid::from_u128(0xdf7e7d36_18a1_44bf_8241_ea6deac9d0d1);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncedFolderSnapshotEventDto {
    pub revision: u64,
    pub folders: Vec<LocationSummaryDto>,
    pub warning: Option<String>,
    pub can_add_folder: bool,
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

        #[cfg(target_os = "windows")]
        {
            let _ = &self.home_dir;
            discover_windows_roots()
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = &self.home_dir;
            Ok(Vec::new())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyncedFolderCandidate {
    identity: Vec<u8>,
    path: PathBuf,
    provider: SyncedFolderProvider,
    status: SyncedFolderStatus,
    availability: SyncedAvailabilityPolicy,
}

struct SyncedFolderState {
    revision: u64,
    roots: Vec<SyncedFolderRoot>,
    summaries: Vec<LocationSummaryDto>,
    warning: Option<String>,
}

pub struct SyncedFolderManager {
    filesystem: Arc<LocalFilesystem>,
    gio_filesystem: Arc<GioFilesystem>,
    discovery: Arc<dyn SyncedFolderDiscovery>,
    manual: ManualSyncedFolderStore,
    state: Mutex<SyncedFolderState>,
    subscribers: Mutex<HashMap<String, Channel<SyncedFolderSnapshotEventDto>>>,
    stopped: Arc<AtomicBool>,
}

impl SyncedFolderManager {
    pub fn start(
        filesystem: Arc<LocalFilesystem>,
        gio_filesystem: Arc<GioFilesystem>,
        home_dir: PathBuf,
        storage_path: PathBuf,
    ) -> Result<Arc<Self>, ExplorerError> {
        let stopped = Arc::new(AtomicBool::new(false));
        let manager = Arc::new(Self {
            filesystem,
            gio_filesystem,
            discovery: Arc::new(SystemSyncedFolderDiscovery::new(home_dir)),
            manual: ManualSyncedFolderStore::new(storage_path, cfg!(target_os = "linux"))?,
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
            can_add_folder: self.manual.enabled(),
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

    pub fn add_manual_folder(&self, path: PathBuf) -> Result<String, ExplorerError> {
        let id = self.manual.add(path)?;
        self.refresh()?;
        Ok(id)
    }

    pub const fn can_add_folder(&self) -> bool {
        self.manual.enabled()
    }

    pub fn remove_manual_folder(&self, folder_id: &str) -> Result<(), ExplorerError> {
        self.manual.remove(folder_id)?;
        self.refresh()
    }

    fn refresh(&self) -> Result<(), ExplorerError> {
        let roots = match self.discovery.discover().and_then(|mut roots| {
            roots.extend(self.manual.discover()?);
            Ok(roots)
        }) {
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
                        can_add_folder: self.manual.enabled(),
                    };
                    drop(state);
                    self.broadcast(event)?;
                }
                return Ok(());
            }
        };

        let mut summaries = self.filesystem.replace_synced_folders(roots.clone())?;
        summaries.extend(self.gio_filesystem.locations()?);

        let mut state = self
            .state
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        if state.roots == roots && state.summaries == summaries && state.warning.is_none() {
            return Ok(());
        }
        state.roots = roots;
        state.summaries = summaries;
        state.warning = None;
        state.revision = state.revision.saturating_add(1);
        let event = SyncedFolderSnapshotEventDto {
            revision: state.revision,
            folders: state.summaries.clone(),
            warning: None,
            can_add_folder: self.manual.enabled(),
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
    let mut candidates = Vec::new();
    let icloud = home_dir.join("Library/Mobile Documents/com~apple~CloudDocs");
    if directory_without_following_symlinks(&icloud)? {
        candidates.push(SyncedFolderCandidate {
            identity: path_identity(&icloud),
            path: icloud,
            provider: SyncedFolderProvider::ICloud,
            // macOS exposes the namespace and item availability, but no
            // provider-neutral root connection status. Directory presence is
            // not proof that the provider service is responsive.
            status: SyncedFolderStatus::Unknown,
            availability: SyncedAvailabilityPolicy::ICloud,
        });
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
                let path = entry.path();
                candidates.push(SyncedFolderCandidate {
                    identity: path_identity(&path),
                    path,
                    provider,
                    status: SyncedFolderStatus::Unknown,
                    availability: SyncedAvailabilityPolicy::Unknown,
                });
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(discovery_io("read operating-system cloud storage", error));
        }
    }

    roots_from_candidates(candidates)
}

fn roots_from_candidates(
    mut candidates: Vec<SyncedFolderCandidate>,
) -> Result<Vec<SyncedFolderRoot>, ExplorerError> {
    candidates.sort_by(|left, right| {
        left.provider
            .cmp(&right.provider)
            .then_with(|| left.identity.cmp(&right.identity))
    });
    let mut seen_identities = HashSet::new();
    let mut seen_paths = HashSet::new();
    candidates.retain(|candidate| {
        seen_identities.insert(candidate.identity.clone())
            && seen_paths.insert(candidate.path.clone())
    });

    let provider_counts = candidates
        .iter()
        .fold(BTreeMap::new(), |mut counts, candidate| {
            *counts.entry(candidate.provider).or_insert(0_usize) += 1;
            counts
        });
    let mut provider_indexes = BTreeMap::<SyncedFolderProvider, usize>::new();
    let mut seen_ids = HashSet::new();
    let mut roots = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let id = format!(
            "synced:{}",
            Uuid::new_v5(&SYNCED_FOLDER_NAMESPACE, &candidate.identity)
        );
        if !seen_ids.insert(id.clone()) {
            continue;
        }
        let index = provider_indexes.entry(candidate.provider).or_insert(0);
        *index += 1;
        let base_name = candidate.provider.display_name();
        let name = if provider_counts
            .get(&candidate.provider)
            .copied()
            .unwrap_or(0)
            > 1
        {
            format!("{base_name} {}", *index)
        } else {
            base_name.to_owned()
        };
        roots.push(SyncedFolderRoot {
            id,
            name,
            path: candidate.path,
            detail: synced_folder_detail(candidate.provider, candidate.status),
            metadata: SyncedFolderMetadataDto {
                provider: candidate.provider,
                status: candidate.status,
                source: SyncedFolderSource::System,
            },
            availability: candidate.availability,
        });
    }
    Ok(roots)
}

#[cfg(target_os = "windows")]
fn discover_windows_roots() -> Result<Vec<SyncedFolderRoot>, ExplorerError> {
    let mut candidates = Vec::new();
    for root in crate::windows_synced_folders::discover().map_err(|error| {
        ExplorerError::Unexpected(format!(
            "Explora could not {} (Windows error 0x{:08X}).",
            error.action, error.code
        ))
    })? {
        if !directory_without_following_symlinks(&root.path)? {
            continue;
        }
        candidates.push(SyncedFolderCandidate {
            identity: root.identity,
            path: root.path,
            provider: provider_from_windows_registration_provider_id(
                &root.registration_provider_id,
            ),
            status: synced_status_from_windows_provider_status(root.provider_status),
            availability: SyncedAvailabilityPolicy::WindowsCloudFiles,
        });
    }

    roots_from_candidates(candidates)
}

fn synced_folder_detail(provider: SyncedFolderProvider, status: SyncedFolderStatus) -> String {
    let state = match status {
        SyncedFolderStatus::Available => "Synced folder",
        SyncedFolderStatus::Offline => "Provider offline",
        SyncedFolderStatus::Paused => "Sync paused",
        SyncedFolderStatus::Error => "Provider error",
        SyncedFolderStatus::Unknown => "Provider status unknown",
    };
    format!("{} · {state}", provider.display_name())
}

#[cfg(any(target_os = "windows", test))]
fn synced_status_from_windows_provider_status(status: Option<u32>) -> SyncedFolderStatus {
    const DISCONNECTED: u32 = 0x0000_0000;
    const IDLE: u32 = 0x0000_0001;
    const POPULATE_NAMESPACE: u32 = 0x0000_0002;
    const POPULATE_METADATA: u32 = 0x0000_0004;
    const POPULATE_CONTENT: u32 = 0x0000_0008;
    const SYNC_INCREMENTAL: u32 = 0x0000_0010;
    const SYNC_FULL: u32 = 0x0000_0020;
    const CONNECTIVITY_LOST: u32 = 0x0000_0040;
    const TERMINATED: u32 = 0xC000_0001;
    const ERROR: u32 = 0xC000_0002;
    const ACTIVE: u32 = IDLE
        | POPULATE_NAMESPACE
        | POPULATE_METADATA
        | POPULATE_CONTENT
        | SYNC_INCREMENTAL
        | SYNC_FULL;

    match status {
        Some(ERROR) => SyncedFolderStatus::Error,
        Some(DISCONNECTED) | Some(TERMINATED) => SyncedFolderStatus::Offline,
        Some(raw) if raw & CONNECTIVITY_LOST != 0 => SyncedFolderStatus::Offline,
        Some(raw) if raw & ACTIVE != 0 => SyncedFolderStatus::Available,
        Some(_) | None => SyncedFolderStatus::Unknown,
    }
}

#[cfg(any(target_os = "windows", test))]
fn provider_from_windows_registration_provider_id(provider_id: &str) -> SyncedFolderProvider {
    if provider_id.eq_ignore_ascii_case("OneDrive") {
        SyncedFolderProvider::OneDrive
    } else if provider_id.eq_ignore_ascii_case("GoogleDrive")
        || provider_id.eq_ignore_ascii_case("GoogleDriveFS")
    {
        SyncedFolderProvider::GoogleDrive
    } else {
        SyncedFolderProvider::Other
    }
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
        assert!(roots
            .iter()
            .all(|root| root.metadata.status == SyncedFolderStatus::Unknown));
        assert!(roots
            .iter()
            .all(|root| root.detail.ends_with("Provider status unknown")));
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
    fn windows_registration_provider_component_is_a_hint_without_exposing_account_data() {
        assert_eq!(
            provider_from_windows_registration_provider_id("OneDrive"),
            SyncedFolderProvider::OneDrive
        );
        assert_eq!(
            provider_from_windows_registration_provider_id("GoogleDriveFS"),
            SyncedFolderProvider::GoogleDrive
        );
        assert_eq!(
            provider_from_windows_registration_provider_id("AcmeCloud"),
            SyncedFolderProvider::Other
        );
    }

    #[test]
    fn maps_windows_provider_status_without_guessing_unknown_values() {
        assert_eq!(
            synced_status_from_windows_provider_status(None),
            SyncedFolderStatus::Unknown
        );
        assert_eq!(
            synced_status_from_windows_provider_status(Some(0)),
            SyncedFolderStatus::Offline
        );
        assert_eq!(
            synced_status_from_windows_provider_status(Some(0x40 | 0x01)),
            SyncedFolderStatus::Offline
        );
        assert_eq!(
            synced_status_from_windows_provider_status(Some(0xC000_0001)),
            SyncedFolderStatus::Offline
        );
        assert_eq!(
            synced_status_from_windows_provider_status(Some(0xC000_0002)),
            SyncedFolderStatus::Error
        );
        for status in [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x31] {
            assert_eq!(
                synced_status_from_windows_provider_status(Some(status)),
                SyncedFolderStatus::Available
            );
        }
        assert_eq!(
            synced_status_from_windows_provider_status(Some(0x8000_0000)),
            SyncedFolderStatus::Unknown
        );
        assert_eq!(
            synced_status_from_windows_provider_status(Some(0x80)),
            SyncedFolderStatus::Unknown
        );
    }

    #[test]
    fn provider_details_expose_only_sanitized_status() {
        assert_eq!(
            synced_folder_detail(SyncedFolderProvider::OneDrive, SyncedFolderStatus::Offline),
            "OneDrive · Provider offline"
        );
        assert_eq!(
            synced_folder_detail(SyncedFolderProvider::GoogleDrive, SyncedFolderStatus::Error),
            "Google Drive · Provider error"
        );
    }

    #[test]
    fn normalizes_platform_candidates_with_private_stable_identities() {
        let temp = TempDir::new().expect("temporary roots");
        let one = temp.path().join("one");
        let two = temp.path().join("two");
        let duplicate_path = two.clone();
        let roots = roots_from_candidates(vec![
            SyncedFolderCandidate {
                identity: b"OneDrive!private-account-one".to_vec(),
                path: one,
                provider: SyncedFolderProvider::OneDrive,
                status: SyncedFolderStatus::Available,
                availability: SyncedAvailabilityPolicy::Unknown,
            },
            SyncedFolderCandidate {
                identity: b"OneDrive!private-account-two".to_vec(),
                path: two,
                provider: SyncedFolderProvider::OneDrive,
                status: SyncedFolderStatus::Available,
                availability: SyncedAvailabilityPolicy::Unknown,
            },
            SyncedFolderCandidate {
                identity: b"duplicate-registration".to_vec(),
                path: duplicate_path,
                provider: SyncedFolderProvider::Other,
                status: SyncedFolderStatus::Available,
                availability: SyncedAvailabilityPolicy::Unknown,
            },
        ])
        .expect("normalized candidates");

        assert_eq!(roots.len(), 2);
        assert_eq!(
            roots
                .iter()
                .map(|root| root.name.as_str())
                .collect::<Vec<_>>(),
            vec!["OneDrive 1", "OneDrive 2"]
        );
        assert!(roots.iter().all(|root| root.id.starts_with("synced:")));
        assert!(roots
            .iter()
            .all(|root| !root.id.contains("private-account")));
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

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn run_native_local_synced_folder_smoke(
        platform: &str,
        home: PathBuf,
        roots: Vec<SyncedFolderRoot>,
    ) {
        use std::{sync::mpsc, time::Duration};

        use crate::{
            filesystem::{DirectoryListingEvent, LocationRole},
            local_filesystem::LocalRoot,
        };

        assert!(
            !roots.is_empty(),
            "native smoke requires at least one configured synced-folder root"
        );
        let expected_root_count = roots.len();
        let roots_to_open = roots.clone();
        let filesystem = LocalFilesystem::new(vec![LocalRoot {
            id: "native-smoke-home",
            name: "Home",
            role: LocationRole::Home,
            path: home.clone(),
        }])
        .unwrap_or_else(|_| panic!("native synced-folder local filesystem setup failed"));
        let locations = filesystem
            .replace_synced_folders(roots)
            .unwrap_or_else(|_| panic!("native synced-folder registration failed"));
        assert_eq!(locations.len(), expected_root_count);
        assert!(locations.iter().all(|location| {
            location.display_path == location.name && location.root.display_path == location.name
        }));
        assert!(locations.iter().all(|location| {
            location
                .synced_folder
                .as_ref()
                .is_some_and(|metadata| metadata.source == SyncedFolderSource::System)
        }));
        let serialized = serde_json::to_string(&locations)
            .expect("native synced-folder locations should serialize");
        assert!(!serialized.contains('@'));

        let mut opened_roots = 0_usize;
        for root in roots_to_open {
            let (sender, receiver) = mpsc::sync_channel(1);
            let home = home.clone();
            std::thread::spawn(move || {
                let opened = (|| {
                    let filesystem = LocalFilesystem::new(vec![LocalRoot {
                        id: "native-smoke-home",
                        name: "Home",
                        role: LocationRole::Home,
                        path: home,
                    }])
                    .ok()?;
                    let location = filesystem
                        .replace_synced_folders(vec![root])
                        .ok()?
                        .into_iter()
                        .next()?;
                    let mut opened = false;
                    let result = filesystem.list_directory(
                        &location.root.id,
                        &location.id,
                        &AtomicBool::new(false),
                        |event| {
                            if matches!(event, DirectoryListingEvent::Started { .. }) {
                                opened = true;
                                // Opening the provider namespace is enough for
                                // this smoke. Enumerating real user entries
                                // belongs in a controlled fixture or interactive
                                // packaged-app scenario.
                                return Err(crate::filesystem::ExplorerError::Cancelled);
                            }
                            Ok(())
                        },
                    );
                    Some(
                        opened
                            && matches!(result, Err(crate::filesystem::ExplorerError::Cancelled)),
                    )
                })()
                .unwrap_or(false);
                let _ = sender.send(opened);
            });

            if receiver
                .recv_timeout(Duration::from_secs(5))
                .unwrap_or(false)
            {
                opened_roots += 1;
            }
        }

        eprintln!(
            "native {platform} synced-folder smoke: discovered={expected_root_count}, opened={opened_roots}, stalled_or_failed={}",
            expected_root_count - opened_roots
        );
        assert!(
            opened_roots > 0,
            "no discovered synced-folder root opened through the local backend"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires real macOS iCloud Drive or File Provider roots"]
    fn native_macos_roots_register_and_open_without_provider_authority_crossing_ipc() {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
            .expect("native smoke requires an accessible macOS home directory");
        let roots = discover_macos_roots(&home)
            .unwrap_or_else(|_| panic!("native macOS synced-folder discovery failed"));
        assert!(roots
            .iter()
            .all(|root| root.metadata.status == SyncedFolderStatus::Unknown));

        run_native_local_synced_folder_smoke("macOS", home, roots);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires real Windows Cloud Files sync roots"]
    fn native_windows_roots_register_and_open_without_provider_authority_crossing_ipc() {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
            .expect("native smoke requires an accessible Windows home directory");
        let roots = discover_windows_roots()
            .unwrap_or_else(|_| panic!("native Windows synced-folder discovery failed"));

        run_native_local_synced_folder_smoke("Windows", home, roots);
    }
}
