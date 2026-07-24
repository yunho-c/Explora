use std::path::Path;

use crate::filesystem::ExplorerError;

/// Narrow platform boundary for recoverable deletion. Implementations receive
/// only Rust-resolved paths and are never exposed through IPC.
pub trait PlatformTrash: Send + Sync {
    fn is_available(&self) -> bool;
    fn move_to_trash(&self, path: &Path) -> Result<(), ExplorerError>;
}

#[derive(Default)]
pub struct SystemPlatformTrash;

impl PlatformTrash for SystemPlatformTrash {
    fn is_available(&self) -> bool {
        cfg!(any(
            target_os = "macos",
            target_os = "linux",
            target_os = "windows"
        ))
    }

    fn move_to_trash(&self, path: &Path) -> Result<(), ExplorerError> {
        if !self.is_available() {
            return Err(ExplorerError::Unsupported(
                "The operating system Trash is not available for this item.".to_owned(),
            ));
        }

        trash::delete(path).map_err(map_trash_error)
    }
}

fn map_trash_error(error: trash::Error) -> ExplorerError {
    match error {
        trash::Error::TargetedRoot => ExplorerError::InvalidReference,
        trash::Error::CouldNotAccess { .. } => ExplorerError::Io {
            message: "Explora could not access this item to move it to Trash.".to_owned(),
            kind: std::io::ErrorKind::PermissionDenied,
        },
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        trash::Error::FileSystem { source, .. } => ExplorerError::Io {
            message: "The operating system could not move this item to Trash.".to_owned(),
            kind: source.kind(),
        },
        _ => ExplorerError::Unexpected(
            "The operating system could not move this item to Trash.".to_owned(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_desktop_targets_advertise_native_trash() {
        assert_eq!(
            SystemPlatformTrash.is_available(),
            cfg!(any(
                target_os = "macos",
                target_os = "linux",
                target_os = "windows"
            ))
        );
    }

    #[test]
    fn trash_errors_do_not_expose_dependency_paths() {
        let mapped = map_trash_error(trash::Error::CouldNotAccess {
            target: "/sensitive/location/private.txt".to_owned(),
        });
        assert!(!mapped.to_string().contains("sensitive"));
        assert!(matches!(
            mapped,
            ExplorerError::Io {
                kind: std::io::ErrorKind::PermissionDenied,
                ..
            }
        ));
    }

    #[test]
    #[ignore = "moves an explicitly supplied fixture into the real operating-system Trash"]
    fn system_trash_moves_an_explicit_native_fixture() {
        let path = std::env::var_os("EXPLORA_NATIVE_TRASH_FIXTURE")
            .map(std::path::PathBuf::from)
            .expect("EXPLORA_NATIVE_TRASH_FIXTURE must name an owned smoke-test fixture");
        assert!(
            path.is_file(),
            "native trash fixture must be a regular file"
        );

        SystemPlatformTrash
            .move_to_trash(&path)
            .expect("native trash operation");

        assert!(!path.exists(), "native trash must remove the source path");
    }
}
