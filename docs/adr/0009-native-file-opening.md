# ADR 0009: Open files with native applications

- Status: Accepted
- Date: 2026-07-22

## Context

Explora already uses Space for a bounded, isolated Quick Preview. Opening a file
for full use is a distinct action: the operating system should choose the native
application, while Explora must keep path authorization, remote transfer limits,
and temporary-resource ownership behind Rust. The webview must not receive a
general path opener or be able to construct shell commands.

Local symlinks, application bundles, executable files, and operating-system
shortcuts also need normal desktop behavior. SSH files require a local snapshot
because native applications cannot consume an opaque SFTP entry reference.

## Decision

Double-click, Enter, and the explicit Open context action request native opening.
Space remains Quick Preview. Ordinary directories, including directory
symlinks, continue to navigate inside Explora. On macOS, a local `.app` bundle is
treated as natively openable rather than as a browsable directory. Every other
local regular file or file symlink is passed to the operating system's default
application through the Rust API provided by `tauri-plugin-opener`.

Directory listings expose only a typed `none`, `direct`, or `download`
capability. Open commands accept the opaque entry and location references that
issued the listing, resolve them again in Rust, and never accept a raw path or a
program name from the webview. The opener plugin's JavaScript commands are not
added to the main-window capability allowlist.

For an SSH file, Explora downloads a read-only snapshot over the active SFTP
session. The transfer has a request ID, typed progress, cancellation, and a
global limit of two concurrent native-open downloads. Additional requests wait
in a bounded queue. Files over 256 MiB, or files whose size is unknown, require
confirmation; files over 2 GiB are rejected. The transfer uses an exclusive
partial file, enforces the byte limit while streaming, syncs before finalization,
and rejects a snapshot when the remote size or modification time changes during
the download.

Snapshots live in an owner-only, application-owned cache directory. Final files
are read-only, retain an executable bit only when the remote metadata declared
one, and are marked as remote-origin content with macOS quarantine metadata or a
Windows Internet Zone identifier. Linux relies on the desktop's normal file
association and executable policy. Successful snapshots remain available to the
launched application for the rest of the Explora session and are deleted during
the next startup. Cleanup failures produce a non-blocking warning and are
retried. A native application's edits are never uploaded to the SSH host.

Remote directories and application bundles are not downloaded recursively for
opening. Explora delegates the interpretation of regular files, executables,
scripts, and shortcut formats to the operating system instead of parsing or
executing them itself.

## Security review

- The only privileged IPC accepts bounded request IDs and opaque references. It
  exposes no raw path, arbitrary application, URL, or shell-command primitive.
- References are re-resolved at execution time. Local symlinks are intentionally
  opened as selected; recursive operations are not introduced.
- Remote bytes are bounded before and during transfer, written with exclusive
  creation, and confined to a randomly named owned directory.
- The download remains cancellable while queued and during SFTP reads. Partial
  resources are removed on cancellation, transfer failure, finalization failure,
  or launch failure.
- The frontend renders file and host names as text and parses all progress and
  result messages as typed data.
- Platform origin metadata is a defense-in-depth signal to the native operating
  system. It does not replace the size limit, permissions, or user confirmation.

## Consequences

Native application behavior, including whether an executable or script launches
or opens in an editor, follows the user's operating-system associations. Remote
files are snapshots rather than live documents, so users must explicitly copy
changes back through a future file-operation workflow. Snapshot cleanup is
delayed until startup because deleting immediately can race applications that
retain or reopen the path.

The implementation has target-specific origin marking and therefore requires
packaged smoke coverage on macOS, Windows, and Linux. Rust tests cover opaque
resolution, symlink capability, bounded real-SFTP downloading, cancellation
contracts, path confinement, and a mocked native opener; frontend tests cover
keyboard separation, confirmation, progress, cancellation, and IPC parsing.
