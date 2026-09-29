# ADR 0013: Create folders through the operation lifecycle

- Status: Accepted
- Date: 2026-09-29

## Decision

New Folder is supported in local and connected SFTP directories through the
existing Rust-owned filesystem-operation coordinator. It adds no Tauri command,
permission, dependency, or shell authority.

Directories advertise `createFolder` independently of move acceptance. A
`createFolder` action carries an opaque parent directory and one leaf name;
its source collection must be empty. Rust bounds all identifiers and the name,
rejects traversal and platform-invalid names, and resolves the parent from its
own registry. Display paths and frontend capability flags are not authority.

The coordinator emits queued, running, and one terminal event for one item.
Local creation runs on a blocking worker under the existing mutation guard;
SFTP creation runs under the session mutation guard. A single exclusive mkdir
creates an empty directory. Existing files, directories, and links are conflicts,
never replacements. There is no recursive create-parents behavior.

The naming dialog retains the draft and displays actionable errors. Toolbar,
file-view context menus (including empty folders), and Cmd+Shift+N on macOS or
Ctrl+Shift+N on Windows/Linux expose the action. On success, the current parent
refreshes and selects the new entry. Navigating away never redirects completion
to a different directory. Terminal input and other dialogs retain keyboard
priority. Focus moves into the naming field and returns to the file view.

## Cancellation and uncertain outcomes

Cancellation is checked before mkdir. Once a mutation is dispatched, its actual
result remains authoritative: an acknowledged creation reports success even if
cancellation arrived meanwhile. Closing the pending dialog requests cancellation
and waits for that result. It never deletes a successfully created folder.

Lost SFTP acknowledgements produce `outcomeUncertain`, take the connection
offline, and require an explicit reconnect and refresh. No automatic retry or
rollback occurs. If mkdir succeeds but metadata cannot be obtained, report
`partialCompletion` and ask the user to refresh before trying again. This avoids
misreporting a committed mutation as an ordinary failure or cancellation.

## Focused security review

- Parent tokens are location scoped and revalidated at execution. Stale local
  identities and changed remote fingerprints are rejected using existing rules.
- Parent symlinks and symlink ancestors within the authorized local root (or
  remote path) are rejected. Editable display strings cannot redirect creation.
- Names reject empty strings, dot entries, separators, NUL, and excessive byte
  length. Windows also rejects reserved device names, forbidden characters, and
  trailing spaces/dots.
- OS/SFTP mkdir is exclusive even if another creator races the preflight check.
  Existing files, directories, and symlinks remain untouched.
- Permissions are enforced by the filesystem/server at execution time. New user
  folders inherit normal filesystem creation permissions and the user's umask.
- No arbitrary paths, shell commands, credentials, or new host integrations cross
  IPC. The frontend validates returned entries and capabilities and renders
  names and errors as text.
- Like existing path-based mutations, validation and mkdir are separate calls.
  External ancestor replacement during that interval is a residual race; SFTP v3
  also lacks directory-handle-relative mkdir and stable inode identity. This
  change does not claim stronger isolation than the existing location boundary.

## Validation

Regression coverage includes local and real disposable SFTP creation, nested
folders, conflicts, invalid names, forged/stale references, symlink parents,
permissions, cancellation before creation and after remote dispatch, disconnect
uncertainty, typed IPC results, selection, focus, and browser-shell workflows.
Unix symlink checks are platform gated. Packaged native interaction and macOS/Linux
execution remain separate release-validation requirements.

### Implementation checkpoint: Windows ARM64

- `bun run test:web --maxWorkers=2`: 126 tests passed.
- `bun run test:e2e --workers=1 --timeout=90000`: 17 Chromium browser-shell
  workflows passed, including local and remote New Folder. The initial cold
  browser load exceeded the default 30-second test limit.
- `bun run check:web`, `bun run lint:web`, and `bun run build:web` passed.
- Formatting checks pass for changed frontend/documentation files and Rust.
  The repository-wide Prettier check reports existing formatting differences
  in 67 untouched files in this Windows checkout.
- All seven Windows-applicable New Folder Rust tests passed, including real
  loopback SSH/SFTP. Unix-only symlink coverage was not executed here.
- Native validation temporarily applied the separate stable Windows file-identity
  prerequisite already in development. It was removed after validation and is
  not part of this feature. Committed main's `windows_by_handle` usage otherwise
  prevents a stable Windows build.
- The broader Rust library run had 148 passes, one existing failure in
  `native_open::tests::snapshot_names_cannot_escape_the_owned_directory`, and two
  ignored tests. The existing
  `terminal::local::tests::conpty_fixture_resizes_reports_output_and_exit` hung
  during the initial run and was explicitly excluded from the completed run.
- Strict all-target Clippy reports existing Windows-only unused test code and
  `permissions_set_readonly_false` in transfer cleanup. Production-library Clippy
  passes when that existing transfer lint is allowed.
- Packaged-native UI smoke tests, release packaging, and macOS/Linux execution
  remain unverified. Browser-shell coverage is not packaged-native proof.
