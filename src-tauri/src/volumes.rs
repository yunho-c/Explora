use std::{
    collections::HashMap,
    ffi::OsStr,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::Duration,
};

use serde::Serialize;
use sysinfo::Disks;
use tauri::ipc::Channel;
use uuid::Uuid;

use crate::{
    filesystem::{ExplorerError, LocationSummaryDto},
    local_filesystem::{LocalFilesystem, VolumeRoot},
};

const REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const VOLUME_NAMESPACE: Uuid = Uuid::from_u128(0x9b1c4b26_10a2_4cd6_91ea_251672c8f94d);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeSnapshotEventDto {
    pub revision: u64,
    pub volumes: Vec<LocationSummaryDto>,
    pub warning: Option<String>,
}

trait VolumeDiscovery: Send + Sync + 'static {
    fn discover(&self) -> Result<Vec<VolumeRoot>, ExplorerError>;
}

#[derive(Default)]
struct SystemVolumeDiscovery;

impl VolumeDiscovery for SystemVolumeDiscovery {
    fn discover(&self) -> Result<Vec<VolumeRoot>, ExplorerError> {
        let disks = Disks::new_with_refreshed_list();
        let mut volumes = disks
            .list()
            .iter()
            .filter(|disk| volume_is_browsable(disk.mount_point(), disk.file_system(), disk.name()))
            .map(|disk| {
                let mount_path = disk.mount_point().to_path_buf();
                let name = volume_name(disk.name(), &mount_path);
                let identity =
                    platform::volume_identity(&mount_path, disk.file_system(), disk.name());
                let id = format!(
                    "volume:{}",
                    Uuid::new_v5(&VOLUME_NAMESPACE, identity.as_bytes())
                );
                VolumeRoot {
                    id,
                    name,
                    path: mount_path,
                    detail: format_capacity(disk.available_space(), disk.total_space()),
                }
            })
            .collect::<Vec<_>>();
        volumes.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        volumes.dedup_by(|left, right| left.id == right.id || left.path == right.path);
        Ok(volumes)
    }
}

struct VolumeState {
    revision: u64,
    roots: Vec<VolumeRoot>,
    summaries: Vec<LocationSummaryDto>,
    warning: Option<String>,
}

pub struct VolumeManager {
    filesystem: Arc<LocalFilesystem>,
    discovery: Arc<dyn VolumeDiscovery>,
    state: Mutex<VolumeState>,
    subscribers: Mutex<HashMap<String, Channel<VolumeSnapshotEventDto>>>,
    platform_warning: Option<String>,
    stopped: Arc<AtomicBool>,
}

impl VolumeManager {
    pub fn start(filesystem: Arc<LocalFilesystem>) -> Result<Arc<Self>, ExplorerError> {
        let (change_sender, change_receiver) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let platform_warning = platform::start_change_watcher(change_sender, stopped.clone())?;
        let manager = Arc::new(Self {
            filesystem,
            discovery: Arc::new(SystemVolumeDiscovery),
            state: Mutex::new(VolumeState {
                revision: 0,
                roots: Vec::new(),
                summaries: Vec::new(),
                warning: None,
            }),
            subscribers: Mutex::new(HashMap::new()),
            platform_warning,
            stopped,
        });
        manager.refresh()?;

        let weak = Arc::downgrade(&manager);
        thread::Builder::new()
            .name("explora-volume-watch".to_owned())
            .spawn(move || {
                while let Some(manager) = weak.upgrade() {
                    if manager.stopped.load(Ordering::Relaxed) {
                        break;
                    }
                    let _ = change_receiver.recv_timeout(REFRESH_INTERVAL);
                    if manager.stopped.load(Ordering::Relaxed) {
                        break;
                    }
                    let _ = manager.refresh();
                }
            })
            .map_err(|error| {
                ExplorerError::Unexpected(format!(
                    "Explora could not start volume discovery: {error}"
                ))
            })?;
        Ok(manager)
    }

    pub fn subscribe(
        &self,
        request_id: String,
        channel: Channel<VolumeSnapshotEventDto>,
    ) -> Result<(), ExplorerError> {
        // Keep the state and subscriber locks in the same order as refresh/broadcast.
        // Registering while the state lock is held prevents a refresh from landing
        // between the initial snapshot and subscription, which would otherwise leave
        // the new subscriber stale until a later device change.
        let state = self
            .state
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let event = VolumeSnapshotEventDto {
            revision: state.revision,
            volumes: state.summaries.clone(),
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
                let warning = Some(format!(
                    "Explora could not refresh mounted volumes: {error}"
                ));
                if state.warning != warning {
                    state.warning = warning;
                    state.revision = state.revision.saturating_add(1);
                    let event = VolumeSnapshotEventDto {
                        revision: state.revision,
                        volumes: state.summaries.clone(),
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
        if state.roots == roots && state.warning == self.platform_warning {
            return Ok(());
        }
        let summaries = self.filesystem.replace_volumes(roots.clone())?;
        state.roots = roots;
        state.summaries = summaries;
        state.warning = self.platform_warning.clone();
        state.revision = state.revision.saturating_add(1);
        let event = VolumeSnapshotEventDto {
            revision: state.revision,
            volumes: state.summaries.clone(),
            warning: state.warning.clone(),
        };
        drop(state);
        self.broadcast(event)
    }

    fn broadcast(&self, event: VolumeSnapshotEventDto) -> Result<(), ExplorerError> {
        let mut subscribers = self
            .subscribers
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        subscribers.retain(|_, channel| channel.send(event.clone()).is_ok());
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::{
        ffi::{c_void, OsStr},
        os::unix::ffi::OsStrExt,
        path::Path,
        ptr::{self, NonNull},
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc::Sender,
            Arc,
        },
        thread,
        time::Duration,
    };

    use dispatch2::DispatchQueue;
    use objc2_core_foundation::{
        kCFAllocatorDefault, CFArray, CFDictionary, CFRetained, CFString, CFType, CFURL, CFUUID,
    };
    use objc2_disk_arbitration::{
        kDADiskDescriptionDeviceModelKey, kDADiskDescriptionDeviceProtocolKey,
        kDADiskDescriptionVolumeUUIDKey, DADisk, DARegisterDiskAppearedCallback,
        DARegisterDiskDescriptionChangedCallback, DARegisterDiskDisappearedCallback, DASession,
    };

    use crate::filesystem::ExplorerError;

    pub fn volume_identity(mount_path: &Path, file_system: &OsStr, name: &OsStr) -> String {
        disk_description(mount_path)
            .and_then(|description| description_uuid(&description))
            .map(|uuid| format!("macos-volume:{uuid}"))
            .unwrap_or_else(|| super::fallback_volume_identity(mount_path, file_system, name))
    }

    pub fn is_physical_volume(mount_path: &Path) -> bool {
        let Some(description) = disk_description(mount_path) else {
            // Discovery should remain useful if Disk Arbitration is temporarily
            // unavailable; the browsable/local CoreFoundation checks still apply.
            return true;
        };
        // These are immutable keys exported by Disk Arbitration for the life
        // of the process.
        let model = description_string(&description, unsafe { kDADiskDescriptionDeviceModelKey })
            .unwrap_or_default()
            .to_ascii_lowercase();
        let protocol =
            description_string(&description, unsafe { kDADiskDescriptionDeviceProtocolKey })
                .unwrap_or_default()
                .to_ascii_lowercase();
        !model.contains("disk image") && !protocol.contains("disk image")
    }

    fn disk_description(mount_path: &Path) -> Option<CFRetained<CFDictionary>> {
        let bytes = mount_path.as_os_str().as_bytes();
        let url = unsafe {
            CFURL::from_file_system_representation(
                kCFAllocatorDefault,
                bytes.as_ptr(),
                bytes.len().try_into().ok()?,
                true,
            )?
        };
        let session = unsafe { DASession::new(None)? };
        let disk = unsafe { DADisk::from_volume_path(None, &session, &url)? };
        unsafe { disk.description() }
    }

    fn description_value<'a>(description: &'a CFDictionary, key: &CFString) -> Option<&'a CFType> {
        let mut value = ptr::null();
        if !unsafe { description.value_if_present((key as *const CFString).cast(), &mut value) }
            || value.is_null()
        {
            return None;
        }
        Some(unsafe { &*value.cast::<CFType>() })
    }

    fn description_string(description: &CFDictionary, key: &CFString) -> Option<String> {
        description_value(description, key)
            .and_then(|value| value.downcast_ref::<CFString>())
            .map(ToString::to_string)
    }

    fn description_uuid(description: &CFDictionary) -> Option<String> {
        // This immutable key is exported by Disk Arbitration for the life of
        // the process.
        let uuid = description_value(description, unsafe { kDADiskDescriptionVolumeUUIDKey })?
            .downcast_ref::<CFUUID>()?;
        CFUUID::new_string(None, Some(uuid)).map(|uuid| uuid.to_string())
    }

    pub fn start_change_watcher(
        sender: Sender<()>,
        stopped: Arc<AtomicBool>,
    ) -> Result<Option<String>, ExplorerError> {
        thread::Builder::new()
            .name("explora-disk-arbitration".to_owned())
            .spawn(move || {
                let Some(session) = (unsafe { DASession::new(None) }) else {
                    return;
                };
                let queue = DispatchQueue::new("com.explora.volume-watch", None);
                let context = Box::into_raw(Box::new(sender));
                unsafe {
                    DARegisterDiskAppearedCallback(
                        &session,
                        None,
                        Some(disk_changed),
                        context.cast(),
                    );
                    DARegisterDiskDisappearedCallback(
                        &session,
                        None,
                        Some(disk_changed),
                        context.cast(),
                    );
                    DARegisterDiskDescriptionChangedCallback(
                        &session,
                        None,
                        None,
                        Some(disk_description_changed),
                        context.cast(),
                    );
                    session.set_dispatch_queue(Some(&queue));
                }
                while !stopped.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(250));
                }
                unsafe {
                    session.set_dispatch_queue(None);
                }
                // Disk Arbitration may already have queued a callback when the
                // session is unscheduled. Keep the tiny sender context alive
                // until process exit rather than risk a callback dereferencing
                // freed memory during shutdown.
            })
            .map_err(|error| {
                ExplorerError::Unexpected(format!(
                    "Explora could not start Disk Arbitration: {error}"
                ))
            })?;
        Ok(None)
    }

    unsafe extern "C-unwind" fn disk_changed(_disk: NonNull<DADisk>, context: *mut c_void) {
        notify(context);
    }

    unsafe extern "C-unwind" fn disk_description_changed(
        _disk: NonNull<DADisk>,
        _keys: NonNull<CFArray>,
        context: *mut c_void,
    ) {
        notify(context);
    }

    fn notify(context: *mut c_void) {
        if context.is_null() {
            return;
        }
        let sender = unsafe { &*context.cast::<Sender<()>>() };
        let _ = sender.send(());
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
mod platform {
    use std::{
        ffi::OsStr,
        path::Path,
        sync::{atomic::AtomicBool, mpsc::Sender, Arc},
    };

    use crate::filesystem::ExplorerError;

    pub fn volume_identity(mount_path: &Path, file_system: &OsStr, name: &OsStr) -> String {
        super::fallback_volume_identity(mount_path, file_system, name)
    }

    pub fn start_change_watcher(
        _sender: Sender<()>,
        _stopped: Arc<AtomicBool>,
    ) -> Result<Option<String>, ExplorerError> {
        Ok(Some(
            "Live volume notifications are unavailable; Explora is checking periodically."
                .to_owned(),
        ))
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::{
        ffi::OsStr,
        path::Path,
        sync::{atomic::AtomicBool, mpsc::Sender, Arc},
        thread,
    };

    use zbus::blocking::{fdo::ObjectManagerProxy, Connection};

    use crate::filesystem::ExplorerError;

    const SERVICE: &str = "org.freedesktop.UDisks2";
    const ROOT: &str = "/org/freedesktop/UDisks2";

    pub fn volume_identity(mount_path: &Path, file_system: &OsStr, name: &OsStr) -> String {
        super::fallback_volume_identity(mount_path, file_system, name)
    }

    pub fn start_change_watcher(
        sender: Sender<()>,
        _stopped: Arc<AtomicBool>,
    ) -> Result<Option<String>, ExplorerError> {
        if Connection::system().is_err() {
            return Ok(Some(
                "UDisks2 is unavailable; Explora is checking mounted volumes periodically."
                    .to_owned(),
            ));
        }
        spawn_signal_watcher(sender.clone(), true)?;
        spawn_signal_watcher(sender, false)?;
        Ok(None)
    }

    fn spawn_signal_watcher(sender: Sender<()>, added: bool) -> Result<(), ExplorerError> {
        thread::Builder::new()
            .name(if added {
                "explora-udisks-added".to_owned()
            } else {
                "explora-udisks-removed".to_owned()
            })
            .spawn(move || {
                let Ok(connection) = Connection::system() else {
                    return;
                };
                let Ok(proxy) = ObjectManagerProxy::builder(&connection)
                    .destination(SERVICE)
                    .and_then(|builder| builder.path(ROOT))
                    .and_then(|builder| builder.build())
                else {
                    return;
                };
                if added {
                    if let Ok(signals) = proxy.receive_interfaces_added() {
                        for _ in signals {
                            let _ = sender.send(());
                        }
                    }
                } else if let Ok(signals) = proxy.receive_interfaces_removed() {
                    for _ in signals {
                        let _ = sender.send(());
                    }
                }
            })
            .map_err(|error| {
                ExplorerError::Unexpected(format!(
                    "Explora could not start UDisks2 notifications: {error}"
                ))
            })?;
        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::{
        ffi::OsStr,
        mem::zeroed,
        path::Path,
        ptr::{null, null_mut},
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc::Sender,
            Arc, OnceLock,
        },
        thread,
        time::Duration,
    };

    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, PeekMessageW,
            RegisterClassW, TranslateMessage, MSG, PM_REMOVE, WM_DEVICECHANGE, WNDCLASSW,
        },
    };

    use crate::filesystem::ExplorerError;

    static CHANGE_SENDER: OnceLock<Sender<()>> = OnceLock::new();

    pub fn volume_identity(mount_path: &Path, file_system: &OsStr, name: &OsStr) -> String {
        super::fallback_volume_identity(mount_path, file_system, name)
    }

    pub fn start_change_watcher(
        sender: Sender<()>,
        stopped: Arc<AtomicBool>,
    ) -> Result<Option<String>, ExplorerError> {
        let _ = CHANGE_SENDER.set(sender);
        thread::Builder::new()
            .name("explora-device-change".to_owned())
            .spawn(move || unsafe { run_message_window(stopped) })
            .map_err(|error| {
                ExplorerError::Unexpected(format!(
                    "Explora could not start Windows device notifications: {error}"
                ))
            })?;
        Ok(None)
    }

    unsafe fn run_message_window(stopped: Arc<AtomicBool>) {
        let instance = unsafe { GetModuleHandleW(null()) };
        if instance.is_null() {
            return;
        }
        let class_name = "ExploraVolumeWatcher\0".encode_utf16().collect::<Vec<_>>();
        let class = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: null_mut(),
            hCursor: null_mut(),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class_name.as_ptr(),
        };
        if unsafe { RegisterClassW(&class) } == 0 {
            return;
        }
        let window = unsafe {
            CreateWindowExW(
                0,
                class_name.as_ptr(),
                class_name.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                instance,
                null(),
            )
        };
        if window.is_null() {
            return;
        }
        let mut message: MSG = unsafe { zeroed() };
        while !stopped.load(Ordering::Relaxed) {
            while unsafe { PeekMessageW(&mut message, window, 0, 0, PM_REMOVE) } != 0 {
                unsafe {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
        unsafe { DestroyWindow(window) };
    }

    unsafe extern "system" fn window_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        const DBT_DEVNODES_CHANGED: usize = 0x0007;
        const DBT_DEVICEARRIVAL: usize = 0x8000;
        const DBT_DEVICEREMOVECOMPLETE: usize = 0x8004;
        if message == WM_DEVICECHANGE
            && matches!(
                wparam,
                DBT_DEVNODES_CHANGED | DBT_DEVICEARRIVAL | DBT_DEVICEREMOVECOMPLETE
            )
        {
            if let Some(sender) = CHANGE_SENDER.get() {
                let _ = sender.send(());
            }
        }
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }
}

impl Drop for VolumeManager {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}

fn volume_name(name: &OsStr, mount_path: &Path) -> String {
    let name = name.to_string_lossy().trim().to_owned();
    if !name.is_empty() {
        return name;
    }
    mount_path
        .file_name()
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "System".to_owned())
}

fn fallback_volume_identity(mount_path: &Path, file_system: &OsStr, name: &OsStr) -> String {
    format!("{file_system:?}|{name:?}|{mount_path:?}")
}

fn format_capacity(available: u64, total: u64) -> String {
    if total == 0 {
        return "Mounted volume".to_owned();
    }
    format!(
        "{} available of {}",
        format_bytes(available),
        format_bytes(total)
    )
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if value >= 10.0 || unit == 0 || value.fract().abs() < 0.05 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn volume_is_browsable(mount_path: &Path, file_system: &OsStr, _name: &OsStr) -> bool {
    if !mount_path.is_absolute() || !mount_path.is_dir() {
        return false;
    }
    let fs = file_system.to_string_lossy().to_ascii_lowercase();
    if matches!(
        fs.as_str(),
        "autofs"
            | "cgroup"
            | "cgroup2"
            | "devfs"
            | "devpts"
            | "fusectl"
            | "nfs"
            | "nfs4"
            | "proc"
            | "smbfs"
            | "sysfs"
            | "tmpfs"
    ) {
        return false;
    }

    #[cfg(target_os = "macos")]
    {
        let path = mount_path.to_string_lossy();
        if mount_path != Path::new("/") && !path.starts_with("/Volumes/") {
            return false;
        }
        if !platform::is_physical_volume(mount_path) {
            return false;
        }
    }

    #[cfg(target_os = "linux")]
    {
        let device = _name.to_string_lossy();
        if device.starts_with("/dev/loop") || device.starts_with("/dev/zram") {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_capacity_for_sidebar_details() {
        assert_eq!(
            format_capacity(750_000_000_000, 1_000_000_000_000),
            "750 GB available of 1 TB"
        );
        assert_eq!(format_capacity(0, 0), "Mounted volume");
    }

    #[test]
    fn rejects_network_and_pseudo_filesystems() {
        assert!(!volume_is_browsable(
            Path::new("/tmp"),
            OsStr::new("tmpfs"),
            OsStr::new("tmpfs")
        ));
        assert!(!volume_is_browsable(
            Path::new("/mnt/share"),
            OsStr::new("nfs"),
            OsStr::new("server")
        ));
    }
}
