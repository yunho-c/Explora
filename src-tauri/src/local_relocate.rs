use std::{io, path::Path};

/// Atomically relocates an entry while refusing to replace an existing name.
///
/// `std::fs::rename` replaces destinations on supported platforms, so mutation
/// callers must use this primitive after their advisory conflict checks. The
/// platform calls below make the no-overwrite decision in the filesystem.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn relocate_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use rustix::fs::{renameat_with, RenameFlags, CWD};

    renameat_with(CWD, source, CWD, destination, RenameFlags::NOREPLACE).map_err(io::Error::from)
}

#[cfg(target_os = "windows")]
pub fn relocate_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileW;

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();

    // SAFETY: both arguments are owned, null-terminated UTF-16 buffers that
    // remain alive for the duration of the call. MoveFileW does not retain
    // either pointer and, unlike MoveFileExW with replacement flags, refuses an
    // existing destination.
    if unsafe { MoveFileW(source.as_ptr(), destination.as_ptr()) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub fn relocate_no_replace(_source: &Path, _destination: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "exclusive filesystem relocation is unavailable on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn relocates_without_replacing_an_existing_destination() {
        let temp = TempDir::new().expect("temporary directory");
        let source = temp.path().join("source.txt");
        let destination = temp.path().join("destination.txt");
        fs::write(&source, b"source").expect("source fixture");
        fs::write(&destination, b"destination").expect("destination fixture");

        let error = relocate_no_replace(&source, &destination)
            .expect_err("existing destination must be preserved");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&source).expect("source preserved"), b"source");
        assert_eq!(
            fs::read(&destination).expect("destination preserved"),
            b"destination"
        );

        fs::remove_file(&destination).expect("remove destination fixture");
        relocate_no_replace(&source, &destination).expect("exclusive relocation");
        assert!(!source.exists());
        assert_eq!(fs::read(&destination).expect("relocated source"), b"source");
    }
}
