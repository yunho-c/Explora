# ADR 0012: Stable Windows file identity

- Status: Accepted
- Date: 2026-09-29

## Context

Windows development failed on stable Rust because local reference revalidation
and transfer ownership used the unstable `MetadataExt::volume_serial_number`
and `file_index` methods. Dropping identity checks would allow operations or
cleanup to act on a replacement at a previously registered path.

## Decision

A shared Rust platform adapter reads local metadata together with its volume and
file identity. On Windows it opens a metadata-only handle with
`FILE_FLAG_BACKUP_SEMANTICS` and `FILE_FLAG_OPEN_REPARSE_POINT`, reads metadata
from that handle, and obtains the volume serial and 64-bit file index through
`GetFileInformationByHandle`. Newly created transfer files use their existing
handle. Unix retains the device and inode from the same metadata result.

The adapter uses the existing `windows-sys` dependency. No new dependency,
nightly Rust requirement, IPC command, or frontend path authority is introduced.

## Security review

- Metadata and identity refer to the same open entry, even if its path is
  replaced between the two reads. The handle is released by Rust ownership.
- Reparse points are opened as entries rather than followed; broken links can
  still be identified. Directory inspection does not require reading contents.
- The handle requests no read/write data access and uses identification-only
  security quality of service. Existing OS permissions remain authoritative.
- Windows inspection failures propagate as I/O errors during revalidation.
  Registration retains the existing optional-identity contract; cleanup still
  requires a known identity matching the currently owned path before removal.
- This does not eliminate the existing race between revalidation and a later
  path-based mutation. It preserves the existing execution-time checks and
  no-replace relocation contract from ADR 0010.

## Validation

Regression tests exercise open-handle and hard-link equivalence, rename
stability, same-content replacement, directory identity, and missing entries.
The existing local operation and transfer tests cover stale references and
preservation of replacements during cleanup. A symlink test also covers broken
targets; on Windows it must be run explicitly with `--ignored` on a host with
Developer Mode or symlink privilege.

These tests do not establish release packaging or native behavior on macOS and
Linux; those platforms retain their separate validation requirements.
