# ADR 0014: Copy and paste through verified transfers

- Status: Accepted
- Date: 2026-09-29

## Decision

Copy is a distinct action in the existing Rust operation coordinator. Entry
`copy` and directory `acceptCopy` capabilities are independent of move authority.
Requests carry bounded, opaque source references and one destination reference.
No command, dependency, clipboard permission, or shell authority is added.

Local and SFTP files, directory trees, and supported symbolic links use the
existing bounded transfer pipeline: plan, create an owned partial target, stream,
verify, and finalize without replacement. Copy never enters source removal or
source-reference invalidation, including after cancellation or remote uncertainty.
Same-location copies always use transfers rather than relocation. Local copies
retain the existing local verification-after-finalization and owned-cleanup
contract; remote targets are verified before finalization.

Conflicts use a shared `transferConflict` prompt with Keep Both, Skip, and Cancel.
Replacement and directory merging remain unavailable. Copying into the source
parent is allowed and asks how to handle its existing name. Successful items
return `copied` with a new destination entry; skipped items return `copySkipped`.
Batch completion, failure, and cancellation retain ordered per-source outcomes.

The application-local clipboard stores one selection and an explicit copy/cut
mode in memory. Copy replaces a previous cut selection and does not fade entries.
Paste retains copied sources for repeated use; cut continues to remove only
successfully moved sources. No filesystem paths are written to or read from the
OS clipboard. Interchange with other file managers is a separate native adapter
feature, not implemented by interpreting external clipboard text as authority.

Copy and Paste are available in toolbar and file-view context menus, including
Paste in empty folders. Cmd+C/Cmd+V on macOS and Ctrl+C/Ctrl+V on Windows/Linux
use the same operation flow. Text inputs, previews, dialogs, and terminal sessions
retain their own key handling. Successful copies select the resulting entries
only when their destination is still displayed, preserving unrelated navigation.
Listing batches that predate a committed copy retain its acknowledged entries
and selection instead of hiding them until a later refresh.

## Focused security and concurrency review

- Client capability flags and display strings never authorize a write. Rust
  resolves source/destination tokens, checks current identities and permissions,
  and rejects forged, stale, overlapping, or oversized source collections.
- Local directory destinations are compared with the canonical source path,
  including aliases and case differences. Remote source subtrees are rejected
  within the same session or the same configured host, port, and user. Symlink
  destination ancestors are rejected; recursive copying preserves links instead
  of following them. Remote links copied to Windows remain explicitly unsupported
  when SFTP cannot establish whether a file or directory link must be created.
- Same-session SFTP endpoints share an owned mutation guard, keeping serialization
  through verification/finalization without reacquiring the same mutex. Separate
  sessions retain their own guards under the coordinator's transfer guard.
- Acknowledged remote destination finalization updates the parent's registered
  fingerprint, permitting subsequent items and pastes. Failure to load metadata
  after committed finalization reports partial completion, not an ordinary copy
  failure that invites an unsafe blind retry.
- Cancellation cleans owned partial resources before commitment. After successful
  commitment, the acknowledged outcome is authoritative. Lost remote finalization
  acknowledgements retain existing uncertain-outcome/offline behavior and are
  never automatically retried. A copy never deletes its source to recover.
- The frontend validates copy outcomes, capabilities, destination identities, and
  batch source ordering before applying them. Copy outcomes cannot be reconciled
  as moves or invalidate a source tab, preview, or history.
- Existing path-based validation/mutation races remain. SFTP v3 cannot supply
  handle-relative creation or reliable identity across distinct server aliases.
  Different host names, mount aliases, and server-side path changes may be
  undetectable; bounded manifests and source revalidation remain mandatory.
  This does not claim sandbox isolation against concurrent filesystem mutation.

## Validation

Regression coverage exercises local and real loopback SSH/SFTP copies in every
backend direction, same-session copying, tree and empty-directory preservation,
symlinks, repeated same-folder conflicts, forged/stale references, descendant
rejection, cancellation, source changes, write failures, source-token retention,
typed IPC, clipboard modes, selection, keyboard ownership, and browser workflows.

Windows uses the separately committed stable file-identity prerequisite from
ADR 0012. Unix-only symlink scenarios and packaged macOS/Linux behavior require
their platform runs. Browser tests exercise the demo adapter and are not native
filesystem or packaged-application proof.

### Implementation checkpoint: Windows ARM64

- `bun install --frozen-lockfile` completed without lockfile changes.
- `bun run test:web --maxWorkers=2`: 144 frontend unit/component tests passed.
- `bun run test:e2e --workers=1 --timeout=90000`: 19 Chromium browser-shell
  workflows passed; the two copy workflows also passed after the final
  stale-listing reconciliation change.
- `bun run check:web`, `bun run lint:web`, `bun run build:web`, and
  `bun run check:rust` passed. Changed frontend/documentation formatting and
  repository Rust formatting passed.
- All six Windows-applicable `copy_action` Rust tests passed, including real
  loopback SSH/SFTP in every backend direction and cancellation between items.
  Existing transfer fault tests now exercise both copy and move intents.
- The broader Rust library run had 153 passes, one existing failure in
  `native_open::tests::snapshot_names_cannot_escape_the_owned_directory`, and two
  ignored tests. The previously hanging
  `terminal::local::tests::conpty_fixture_resizes_reports_output_and_exit` was
  explicitly excluded. The subsequently added batch-cancellation case passed
  in the focused copy suite.
- Strict all-target Clippy still reports existing Windows-only unused test code
  and `permissions_set_readonly_false` in transfer cleanup. Production-library
  Clippy passes when that existing transfer lint is allowed.
- Release packaging, packaged-native UI interaction, screen-reader validation,
  and macOS/Linux execution were not performed. OS clipboard interchange remains
  outside this implementation.
