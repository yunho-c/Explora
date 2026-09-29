use std::{fs, io, path::Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    pub(crate) volume: u64,
    pub(crate) file: u64,
}

/// Reads metadata and identity for the same entry without following its symlink.
pub(crate) fn symlink_metadata(path: &Path) -> io::Result<(fs::Metadata, Option<FileIdentity>)> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, SECURITY_IDENTIFICATION,
        };

        // Metadata-only access also supports unreadable files and directories.
        // OPEN_REPARSE_POINT identifies the link itself, including broken links.
        // Keep the handle open for both reads so a path replacement cannot mix
        // one entry's metadata with another entry's identity.
        let file = fs::OpenOptions::new()
            .access_mode(0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(path)?;
        Ok((file.metadata()?, file_identity(&file)?))
    }
    #[cfg(not(windows))]
    {
        let metadata = fs::symlink_metadata(path)?;
        let identity = metadata_identity(&metadata);
        Ok((metadata, identity))
    }
}

/// Uses the existing handle when recording ownership of a newly created file.
pub(crate) fn file_identity(file: &fs::File) -> io::Result<Option<FileIdentity>> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };

        let mut information = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: File owns a valid handle throughout the call, and information
        // is a writable, correctly aligned output buffer that Windows does not retain.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Some(FileIdentity {
            volume: u64::from(information.dwVolumeSerialNumber),
            file: (u64::from(information.nFileIndexHigh) << 32)
                | u64::from(information.nFileIndexLow),
        }))
    }
    #[cfg(not(windows))]
    {
        Ok(metadata_identity(&file.metadata()?))
    }
}

#[cfg(unix)]
fn metadata_identity(metadata: &fs::Metadata) -> Option<FileIdentity> {
    use std::os::unix::fs::MetadataExt;
    Some(FileIdentity {
        volume: metadata.dev(),
        file: metadata.ino(),
    })
}

#[cfg(not(any(unix, windows)))]
fn metadata_identity(_metadata: &fs::Metadata) -> Option<FileIdentity> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_survives_rename_and_matches_an_open_handle_and_hard_link() {
        let temp = tempfile::tempdir().expect("fixture");
        let original = temp.path().join("original");
        fs::write(&original, b"contents").expect("file");
        let file = fs::File::open(&original).expect("handle");
        let expected = file_identity(&file).expect("handle identity");
        assert!(expected.is_some());
        let alias = temp.path().join("alias");
        fs::hard_link(&original, &alias).expect("hard link");
        let renamed = temp.path().join("renamed");
        fs::rename(&original, &renamed).expect("rename");
        assert_eq!(
            symlink_metadata(&renamed).expect("renamed identity").1,
            expected
        );
        assert_eq!(
            symlink_metadata(&alias).expect("alias identity").1,
            expected
        );
    }

    #[test]
    fn replacement_has_a_different_identity_even_with_the_same_contents() {
        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("original");
        fs::write(&path, b"same").expect("original");
        let (_, expected) = symlink_metadata(&path).expect("original identity");
        fs::rename(&path, temp.path().join("retained")).expect("retain original");
        fs::write(&path, b"same").expect("replacement");
        let (_, actual) = symlink_metadata(&path).expect("replacement identity");
        assert!(expected.is_some());
        assert!(actual.is_some());
        assert_ne!(actual, expected);
    }

    #[test]
    fn reads_directory_identity_and_reports_missing_entries() {
        let temp = tempfile::tempdir().expect("fixture");
        let (metadata, identity) = symlink_metadata(temp.path()).expect("directory metadata");
        assert!(metadata.is_dir());
        assert!(identity.is_some());
        assert_eq!(
            symlink_metadata(&temp.path().join("missing"))
                .expect_err("missing")
                .kind(),
            io::ErrorKind::NotFound,
        );
    }

    #[cfg(any(unix, windows))]
    #[cfg_attr(
        windows,
        ignore = "requires Windows Developer Mode or symlink privilege"
    )]
    #[test]
    fn identifies_symlinks_without_following_even_a_missing_target() {
        let temp = tempfile::tempdir().expect("fixture");
        let target = temp.path().join("target");
        let link = temp.path().join("link");
        fs::write(&target, b"target").expect("target");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&target, &link).expect("symlink");
        let (metadata, identity) = symlink_metadata(&link).expect("link metadata");
        assert!(metadata.file_type().is_symlink());
        assert!(identity.is_some());
        assert_ne!(
            identity,
            symlink_metadata(&target).expect("target metadata").1
        );
        fs::remove_file(&target).expect("remove target");
        assert_eq!(
            identity,
            symlink_metadata(&link).expect("broken link metadata").1
        );
    }
}
