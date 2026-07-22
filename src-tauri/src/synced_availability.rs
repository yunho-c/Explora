use std::{path::Path, sync::Arc};

use crate::filesystem::ContentAvailability;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// Each target constructs only its own platform policy in non-test builds.
#[allow(dead_code)]
pub(crate) enum SyncedAvailabilityPolicy {
    ICloud,
    WindowsCloudFiles,
    LocalMirror,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ICloudDownloadStatus {
    Current,
    Downloaded,
    NotDownloaded,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ICloudAvailabilityMetadata {
    is_ubiquitous: Option<bool>,
    is_downloading: Option<bool>,
    download_status: Option<ICloudDownloadStatus>,
    has_download_error: bool,
}

trait ICloudMetadataSource: Send + Sync {
    /// Reads only URL resource metadata. Implementations must never open the
    /// item or request ubiquitous-item content.
    fn read_metadata(
        &self,
        path: &Path,
        is_directory: bool,
    ) -> Result<ICloudAvailabilityMetadata, ()>;
}

struct SystemICloudMetadataSource;

#[cfg(any(target_os = "windows", test))]
const WINDOWS_STATE_PLACEHOLDER: u32 = 0x0000_0001;
#[cfg(any(target_os = "windows", test))]
const WINDOWS_STATE_PARTIAL: u32 = 0x0000_0010;
#[cfg(any(target_os = "windows", test))]
const WINDOWS_STATE_PARTIALLY_ON_DISK: u32 = 0x0000_0020;
#[cfg(any(target_os = "windows", test))]
const WINDOWS_STATE_INVALID: u32 = u32::MAX;
#[cfg(any(target_os = "windows", test))]
const WINDOWS_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
#[cfg(any(target_os = "windows", test))]
const WINDOWS_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
#[cfg(any(target_os = "windows", test))]
const WINDOWS_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WindowsPlaceholderMetadata {
    state: u32,
    attributes: u32,
}

#[cfg(any(target_os = "windows", test))]
trait WindowsPlaceholderMetadataSource: Send + Sync {
    /// Reads directory-enumeration metadata only. Implementations must not
    /// open the item or request Cloud Files hydration.
    fn read_metadata(&self, path: &Path) -> Result<WindowsPlaceholderMetadata, ()>;
}

#[cfg(any(target_os = "windows", test))]
struct SystemWindowsPlaceholderMetadataSource;

#[derive(Clone)]
pub(crate) struct SyncedAvailabilityInspector {
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    source: Arc<dyn ICloudMetadataSource>,
    #[cfg(any(target_os = "windows", test))]
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    windows_source: Arc<dyn WindowsPlaceholderMetadataSource>,
}

impl Default for SyncedAvailabilityInspector {
    fn default() -> Self {
        Self {
            source: Arc::new(SystemICloudMetadataSource),
            #[cfg(any(target_os = "windows", test))]
            windows_source: Arc::new(SystemWindowsPlaceholderMetadataSource),
        }
    }
}

impl SyncedAvailabilityInspector {
    pub(crate) fn inspect(
        &self,
        policy: SyncedAvailabilityPolicy,
        path: &Path,
        is_directory: bool,
    ) -> ContentAvailability {
        match policy {
            SyncedAvailabilityPolicy::ICloud => self
                .source
                .read_metadata(path, is_directory)
                .map(map_icloud_metadata)
                .unwrap_or(ContentAvailability::Unknown),
            SyncedAvailabilityPolicy::WindowsCloudFiles => self.inspect_windows_metadata(path),
            SyncedAvailabilityPolicy::LocalMirror => ContentAvailability::Local,
            SyncedAvailabilityPolicy::Unknown => ContentAvailability::Unknown,
        }
    }

    fn inspect_windows_metadata(&self, path: &Path) -> ContentAvailability {
        #[cfg(any(target_os = "windows", test))]
        {
            inspect_windows_metadata(self.windows_source.as_ref(), path)
        }

        #[cfg(not(any(target_os = "windows", test)))]
        {
            let _ = path;
            ContentAvailability::Unknown
        }
    }
}

#[cfg(any(target_os = "windows", test))]
fn inspect_windows_metadata(
    source: &dyn WindowsPlaceholderMetadataSource,
    path: &Path,
) -> ContentAvailability {
    source
        .read_metadata(path)
        .map(map_windows_placeholder_metadata)
        .unwrap_or(ContentAvailability::Unknown)
}

#[cfg(any(target_os = "windows", test))]
fn map_windows_placeholder_metadata(metadata: WindowsPlaceholderMetadata) -> ContentAvailability {
    if metadata.state == WINDOWS_STATE_INVALID {
        return ContentAvailability::Unknown;
    }
    if metadata.state & (WINDOWS_STATE_PARTIAL | WINDOWS_STATE_PARTIALLY_ON_DISK) != 0 {
        return ContentAvailability::Partial;
    }
    if metadata.state & WINDOWS_STATE_PLACEHOLDER == 0 {
        return ContentAvailability::Local;
    }
    if metadata.attributes
        & (WINDOWS_ATTRIBUTE_OFFLINE
            | WINDOWS_ATTRIBUTE_RECALL_ON_OPEN
            | WINDOWS_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
        != 0
    {
        return ContentAvailability::OnlineOnly;
    }

    ContentAvailability::Local
}

fn map_icloud_metadata(metadata: ICloudAvailabilityMetadata) -> ContentAvailability {
    if metadata.is_ubiquitous != Some(true) {
        return ContentAvailability::Unknown;
    }
    if metadata.has_download_error {
        return ContentAvailability::Error;
    }
    if metadata.is_downloading == Some(true) {
        return ContentAvailability::Downloading;
    }

    match metadata.download_status {
        Some(ICloudDownloadStatus::Current) => ContentAvailability::Local,
        // Apple defines Downloaded as a local but stale copy whose newest
        // version will be downloaded. Treat it as syncing so content access
        // remains gated until the copy is current.
        Some(ICloudDownloadStatus::Downloaded) => ContentAvailability::Syncing,
        Some(ICloudDownloadStatus::NotDownloaded) => ContentAvailability::OnlineOnly,
        Some(ICloudDownloadStatus::Other) | None => ContentAvailability::Unknown,
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2::{rc::Retained, runtime::AnyObject};
    use objc2_foundation::{
        NSError, NSNumber, NSString, NSURLIsUbiquitousItemKey, NSURLResourceKey,
        NSURLUbiquitousItemDownloadingErrorKey, NSURLUbiquitousItemDownloadingStatusCurrent,
        NSURLUbiquitousItemDownloadingStatusDownloaded, NSURLUbiquitousItemDownloadingStatusKey,
        NSURLUbiquitousItemDownloadingStatusNotDownloaded, NSURLUbiquitousItemIsDownloadingKey,
        NSURL,
    };

    use super::{
        ICloudAvailabilityMetadata, ICloudDownloadStatus, ICloudMetadataSource,
        SystemICloudMetadataSource,
    };
    use std::path::Path;

    impl ICloudMetadataSource for SystemICloudMetadataSource {
        fn read_metadata(
            &self,
            path: &Path,
            is_directory: bool,
        ) -> Result<ICloudAvailabilityMetadata, ()> {
            let url = NSURL::from_path(path, is_directory, None).ok_or(())?;
            // SAFETY: These are immutable Foundation constants provided by the
            // macOS process and documented to exist for the lifetime of the
            // Foundation framework.
            let (
                is_ubiquitous_key,
                is_downloading_key,
                download_status_key,
                download_error_key,
                status_current,
                status_downloaded,
                status_not_downloaded,
            ) = unsafe {
                (
                    NSURLIsUbiquitousItemKey,
                    NSURLUbiquitousItemIsDownloadingKey,
                    NSURLUbiquitousItemDownloadingStatusKey,
                    NSURLUbiquitousItemDownloadingErrorKey,
                    NSURLUbiquitousItemDownloadingStatusCurrent,
                    NSURLUbiquitousItemDownloadingStatusDownloaded,
                    NSURLUbiquitousItemDownloadingStatusNotDownloaded,
                )
            };
            let is_ubiquitous = read_bool(&url, is_ubiquitous_key)?;

            if is_ubiquitous != Some(true) {
                return Ok(ICloudAvailabilityMetadata {
                    is_ubiquitous,
                    is_downloading: None,
                    download_status: None,
                    has_download_error: false,
                });
            }

            let is_downloading = read_bool(&url, is_downloading_key).ok().flatten();
            let download_status =
                read_string(&url, download_status_key)
                    .ok()
                    .flatten()
                    .map(|status| {
                        if status.isEqualToString(status_current) {
                            ICloudDownloadStatus::Current
                        } else if status.isEqualToString(status_downloaded) {
                            ICloudDownloadStatus::Downloaded
                        } else if status.isEqualToString(status_not_downloaded) {
                            ICloudDownloadStatus::NotDownloaded
                        } else {
                            ICloudDownloadStatus::Other
                        }
                    });
            let has_download_error = read_value(&url, download_error_key)
                .ok()
                .flatten()
                .and_then(|value| value.downcast::<NSError>().ok())
                .is_some();

            Ok(ICloudAvailabilityMetadata {
                is_ubiquitous,
                is_downloading,
                download_status,
                has_download_error,
            })
        }
    }

    fn read_value(url: &NSURL, key: &NSURLResourceKey) -> Result<Option<Retained<AnyObject>>, ()> {
        let mut value = None;
        // SAFETY: `value` is an Objective-C object slot owned by this call, and
        // every requested key is a documented NSURL resource key. Runtime type
        // checks occur before any returned value is interpreted.
        unsafe { url.getResourceValue_forKey_error(&mut value, key) }.map_err(|_| ())?;
        Ok(value)
    }

    fn read_bool(url: &NSURL, key: &NSURLResourceKey) -> Result<Option<bool>, ()> {
        Ok(read_value(url, key)?
            .and_then(|value| value.downcast::<NSNumber>().ok())
            .map(|value| value.as_bool()))
    }

    fn read_string(url: &NSURL, key: &NSURLResourceKey) -> Result<Option<Retained<NSString>>, ()> {
        Ok(read_value(url, key)?.and_then(|value| value.downcast::<NSString>().ok()))
    }
}

#[cfg(not(target_os = "macos"))]
impl ICloudMetadataSource for SystemICloudMetadataSource {
    fn read_metadata(
        &self,
        _path: &Path,
        _is_directory: bool,
    ) -> Result<ICloudAvailabilityMetadata, ()> {
        Err(())
    }
}

#[cfg(target_os = "windows")]
impl WindowsPlaceholderMetadataSource for SystemWindowsPlaceholderMetadataSource {
    fn read_metadata(&self, path: &Path) -> Result<WindowsPlaceholderMetadata, ()> {
        use std::os::windows::ffi::OsStrExt;

        use windows::{
            core::PCWSTR,
            Win32::Storage::{
                CloudFilters::CfGetPlaceholderStateFromAttributeTag,
                FileSystem::{
                    FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW,
                    FIND_FIRST_EX_FLAGS, WIN32_FIND_DATAW,
                },
            },
        };

        struct FindHandle(windows::Win32::Foundation::HANDLE);

        impl Drop for FindHandle {
            fn drop(&mut self) {
                // SAFETY: The handle came from a successful FindFirstFileExW
                // call and is closed exactly once by this guard.
                let _ = unsafe { FindClose(self.0) };
            }
        }

        let mut wide_path = path.as_os_str().encode_wide().collect::<Vec<_>>();
        if wide_path.is_empty() || wide_path.contains(&0) {
            return Err(());
        }
        wide_path.push(0);
        let mut find_data = WIN32_FIND_DATAW::default();
        // SAFETY: `wide_path` is NUL-terminated and lives through the call;
        // `find_data` is a correctly sized writable output buffer. This is a
        // directory metadata query and does not open the file's content.
        let handle = unsafe {
            FindFirstFileExW(
                PCWSTR(wide_path.as_ptr()),
                FindExInfoBasic,
                (&mut find_data as *mut WIN32_FIND_DATAW).cast(),
                FindExSearchNameMatch,
                None,
                FIND_FIRST_EX_FLAGS(0),
            )
        }
        .map_err(|_| ())?;
        let _handle = FindHandle(handle);
        // SAFETY: Both values come directly from WIN32_FIND_DATAW as required
        // by the documented Cloud Files metadata-only helper.
        let state = unsafe {
            CfGetPlaceholderStateFromAttributeTag(find_data.dwFileAttributes, find_data.dwReserved0)
        };

        Ok(WindowsPlaceholderMetadata {
            state: state.0,
            attributes: find_data.dwFileAttributes,
        })
    }
}

#[cfg(all(not(target_os = "windows"), test))]
impl WindowsPlaceholderMetadataSource for SystemWindowsPlaceholderMetadataSource {
    fn read_metadata(&self, _path: &Path) -> Result<WindowsPlaceholderMetadata, ()> {
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[cfg(not(target_os = "windows"))]
    struct FixtureSource {
        calls: Arc<AtomicUsize>,
        metadata: ICloudAvailabilityMetadata,
    }

    struct WindowsFixtureSource {
        calls: Arc<AtomicUsize>,
        metadata: WindowsPlaceholderMetadata,
    }

    impl WindowsPlaceholderMetadataSource for WindowsFixtureSource {
        fn read_metadata(&self, _path: &Path) -> Result<WindowsPlaceholderMetadata, ()> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(self.metadata)
        }
    }

    #[cfg(not(target_os = "windows"))]
    impl ICloudMetadataSource for FixtureSource {
        fn read_metadata(
            &self,
            _path: &Path,
            _is_directory: bool,
        ) -> Result<ICloudAvailabilityMetadata, ()> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(self.metadata)
        }
    }

    fn metadata(status: ICloudDownloadStatus) -> ICloudAvailabilityMetadata {
        ICloudAvailabilityMetadata {
            is_ubiquitous: Some(true),
            is_downloading: Some(false),
            download_status: Some(status),
            has_download_error: false,
        }
    }

    #[test]
    fn maps_documented_icloud_download_states_conservatively() {
        assert_eq!(
            map_icloud_metadata(metadata(ICloudDownloadStatus::Current)),
            ContentAvailability::Local
        );
        assert_eq!(
            map_icloud_metadata(metadata(ICloudDownloadStatus::Downloaded)),
            ContentAvailability::Syncing
        );
        assert_eq!(
            map_icloud_metadata(metadata(ICloudDownloadStatus::NotDownloaded)),
            ContentAvailability::OnlineOnly
        );
        assert_eq!(
            map_icloud_metadata(metadata(ICloudDownloadStatus::Other)),
            ContentAvailability::Unknown
        );
    }

    #[test]
    fn download_activity_and_errors_take_precedence_over_status() {
        let mut fixture = metadata(ICloudDownloadStatus::Current);
        fixture.is_downloading = Some(true);
        assert_eq!(
            map_icloud_metadata(fixture),
            ContentAvailability::Downloading
        );

        fixture.has_download_error = true;
        assert_eq!(map_icloud_metadata(fixture), ContentAvailability::Error);
    }

    #[test]
    fn non_ubiquitous_items_remain_unknown() {
        let mut fixture = metadata(ICloudDownloadStatus::Current);
        fixture.is_ubiquitous = Some(false);
        assert_eq!(map_icloud_metadata(fixture), ContentAvailability::Unknown);
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn unknown_and_local_mirror_policies_do_not_query_icloud_metadata() {
        let calls = Arc::new(AtomicUsize::new(0));
        let inspector = SyncedAvailabilityInspector {
            source: Arc::new(FixtureSource {
                calls: calls.clone(),
                metadata: metadata(ICloudDownloadStatus::Current),
            }),
            windows_source: Arc::new(SystemWindowsPlaceholderMetadataSource),
        };

        assert_eq!(
            inspector.inspect(
                SyncedAvailabilityPolicy::Unknown,
                Path::new("ignored"),
                false
            ),
            ContentAvailability::Unknown
        );
        assert_eq!(
            inspector.inspect(
                SyncedAvailabilityPolicy::LocalMirror,
                Path::new("ignored"),
                false
            ),
            ContentAvailability::Local
        );
        assert_eq!(calls.load(Ordering::Relaxed), 0);

        assert_eq!(
            inspector.inspect(
                SyncedAvailabilityPolicy::ICloud,
                Path::new("ignored"),
                false
            ),
            ContentAvailability::Local
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn maps_windows_cloud_files_states_without_opening_content() {
        let cases = [
            (0, 0, ContentAvailability::Local),
            (WINDOWS_STATE_PLACEHOLDER, 0, ContentAvailability::Local),
            (
                WINDOWS_STATE_PLACEHOLDER,
                WINDOWS_ATTRIBUTE_OFFLINE,
                ContentAvailability::OnlineOnly,
            ),
            (
                WINDOWS_STATE_PLACEHOLDER,
                WINDOWS_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
                ContentAvailability::OnlineOnly,
            ),
            (
                WINDOWS_STATE_PLACEHOLDER | WINDOWS_STATE_PARTIAL,
                WINDOWS_ATTRIBUTE_OFFLINE,
                ContentAvailability::Partial,
            ),
            (
                WINDOWS_STATE_PLACEHOLDER | WINDOWS_STATE_PARTIAL | WINDOWS_STATE_PARTIALLY_ON_DISK,
                WINDOWS_ATTRIBUTE_RECALL_ON_OPEN,
                ContentAvailability::Partial,
            ),
            (WINDOWS_STATE_INVALID, 0, ContentAvailability::Unknown),
        ];

        for (state, attributes, expected) in cases {
            assert_eq!(
                map_windows_placeholder_metadata(WindowsPlaceholderMetadata { state, attributes }),
                expected
            );
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let source = WindowsFixtureSource {
            calls: calls.clone(),
            metadata: WindowsPlaceholderMetadata {
                state: WINDOWS_STATE_PLACEHOLDER,
                attributes: WINDOWS_ATTRIBUTE_OFFLINE,
            },
        };
        assert_eq!(
            inspect_windows_metadata(&source, Path::new("metadata-only")),
            ContentAvailability::OnlineOnly
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn ordinary_local_files_are_not_misclassified_as_icloud_content() {
        let temp = tempfile::NamedTempFile::new().expect("temporary file");
        assert_eq!(
            SyncedAvailabilityInspector::default().inspect(
                SyncedAvailabilityPolicy::ICloud,
                temp.path(),
                false
            ),
            ContentAvailability::Unknown
        );
    }
}
