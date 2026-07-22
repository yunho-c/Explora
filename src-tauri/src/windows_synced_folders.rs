use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf};

use windows::{
    core::PCWSTR,
    Storage::Provider::StorageProviderSyncRootManager,
    Win32::{
        Foundation::RPC_E_CHANGED_MODE,
        Storage::CloudFilters::{
            CfGetSyncRootInfoByPath, CF_SYNC_ROOT_INFO_PROVIDER, CF_SYNC_ROOT_PROVIDER_INFO,
        },
        System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
    },
};

#[derive(Debug)]
pub(crate) struct WindowsSyncRoot {
    pub identity: Vec<u8>,
    pub path: PathBuf,
    pub registration_provider_id: String,
    pub provider_status: Option<u32>,
}

#[derive(Debug)]
pub(crate) struct WindowsSyncRootError {
    pub action: &'static str,
    pub code: u32,
}

struct RuntimeApartment(bool);

impl RuntimeApartment {
    fn initialize() -> Result<Self, WindowsSyncRootError> {
        // SAFETY: The guard balances every successful initialization on this
        // thread. RPC_E_CHANGED_MODE means the caller already initialized a
        // different apartment, which is also valid for these static APIs.
        match unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
            Ok(()) => Ok(Self(true)),
            Err(error) if error.code() == RPC_E_CHANGED_MODE => Ok(Self(false)),
            Err(error) => Err(platform_error("initialize Windows Runtime", error)),
        }
    }
}

impl Drop for RuntimeApartment {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: This balances the successful RoInitialize call made on
            // the same discovery thread by RuntimeApartment::initialize.
            unsafe { RoUninitialize() };
        }
    }
}

pub(crate) fn discover() -> Result<Vec<WindowsSyncRoot>, WindowsSyncRootError> {
    let _apartment = RuntimeApartment::initialize()?;
    let registered = StorageProviderSyncRootManager::GetCurrentSyncRoots()
        .map_err(|error| platform_error("enumerate registered sync roots", error))?;
    let count = registered
        .Size()
        .map_err(|error| platform_error("count registered sync roots", error))?;
    let mut roots = Vec::with_capacity(count as usize);

    for index in 0..count {
        let registered_root = registered
            .GetAt(index)
            .map_err(|error| platform_error("read a registered sync root", error))?;
        let registration_id = registered_root
            .Id()
            .map_err(|error| platform_error("read a sync-root identity", error))?;
        if registration_id.is_empty() {
            continue;
        }
        let storage_folder = registered_root
            .Path()
            .map_err(|error| platform_error("read a sync-root folder", error))?;
        let path_value = storage_folder
            .Path()
            .map_err(|error| platform_error("read a sync-root path", error))?;
        if path_value.is_empty() {
            continue;
        }

        let mut identity = b"windows-sync-root\0".to_vec();
        identity.extend(registration_id.iter().flat_map(|unit| unit.to_le_bytes()));
        let provider_end = registration_id
            .iter()
            .position(|unit| *unit == u16::from(b'!'))
            .unwrap_or(registration_id.len());
        let path = PathBuf::from(OsString::from_wide(&path_value));
        roots.push(WindowsSyncRoot {
            identity,
            provider_status: query_provider_status(&path),
            path,
            registration_provider_id: String::from_utf16_lossy(&registration_id[..provider_end]),
        });
    }

    Ok(roots)
}

fn query_provider_status(path: &std::path::Path) -> Option<u32> {
    let mut wide_path = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if wide_path.is_empty() || wide_path.contains(&0) {
        return None;
    }
    wide_path.push(0);
    let mut provider_info = CF_SYNC_ROOT_PROVIDER_INFO::default();

    // SAFETY: `wide_path` is null-terminated for the duration of the call and
    // `provider_info` is a correctly sized writable buffer for the requested
    // CF_SYNC_ROOT_INFO_PROVIDER class. Provider name/version are ignored.
    unsafe {
        CfGetSyncRootInfoByPath(
            PCWSTR(wide_path.as_ptr()),
            CF_SYNC_ROOT_INFO_PROVIDER,
            std::ptr::from_mut(&mut provider_info).cast(),
            u32::try_from(std::mem::size_of::<CF_SYNC_ROOT_PROVIDER_INFO>()).ok()?,
            None,
        )
    }
    .ok()?;
    Some(provider_info.ProviderStatus.0)
}

fn platform_error(action: &'static str, error: windows::core::Error) -> WindowsSyncRootError {
    WindowsSyncRootError {
        action,
        code: error.code().0 as u32,
    }
}
