# ADR 0010: Capability-driven filesystem actions

- Status: Accepted
- Date: 2026-07-22

## Context

Explora's local and SFTP backends were introduced as read-only boundaries. Adding
rename, move, trash, and permanent deletion expands the privileged IPC surface
and makes opaque-reference lifetime, conflicts, cancellation, remote uncertainty,
and partial completion part of the product contract.

Blocking mutation commands would not extend safely to transfers or recursive
work. Backend-name checks in the frontend would also couple presentation to
implementation and could expose actions that a read-only mount, disconnected
session, entry type, or platform integration cannot perform.

The complete design and phased strategy are described in
[`docs/filesystem-action.md`](../filesystem-action.md). This ADR records the
security boundary and operation model accepted for implementation.

## Decision

Rust owns filesystem-operation state behind a `FileOperationCoordinator`.
Frontend requests contain opaque, location-scoped entry and directory references,
an action, and bounded options. They never contain authoritative paths or backend
implementation details.

Every entry advertises typed capabilities for rename, move, trash, and permanent
delete. Directory destinations advertise move acceptance and atomic-replacement
capabilities independently. The frontend uses these values to present actions,
while Rust revalidates references, capabilities, identity, and filesystem state
immediately before execution.

Operations use opaque IDs and monotonically sequenced events. The lifecycle
supports queued, running, awaiting-confirmation, awaiting-conflict, completed,
cancelled, and failed states. Terminal states are immutable. Cancellation is
best-effort; an atomic or remotely acknowledged mutation is never described as
cancelled merely because its result arrived after a cancellation request.

Local and SFTP implementations converge on a shared backend mutation contract.
Same-filesystem rename and move use an explicit no-replace relocation
primitive. Cross-filesystem and cross-backend moves are coordinated copy,
finalize, verify, and delete operations. The source is removed only after the
destination succeeds and is verified.

Opaque path registries are mutation-aware. Same-backend directory relocation
rebases registered descendants while preserving their tokens. Trash and permanent
deletion invalidate affected subtrees only after success. Cross-backend moves
create new destination identities and invalidate source identities only after the
source has been removed.

Local deletion prefers a narrow native-trash adapter. If trash is unavailable,
permanent deletion requires an explicit Rust-authoritative confirmation. Remote
deletion is permanent unless a later backend implements and tests genuine trash
semantics; it always requires confirmation showing the authoritative host and
target presentation.

Operations act on symlink entries and never follow their targets implicitly.
Recursive work is coordinator-owned, bounded, cancellable between entries, and
implemented through backend APIs rather than shell commands.

## First vertical slice

The initial implementation enables single-entry local rename:

- Local entries advertise rename capability; all unimplemented capabilities stay
  false.
- The frontend provides an accessible inline editor and submits a typed operation
  request through the common coordinator.
- Rust validates the name, location, root boundary, current filesystem identity,
  and destination conflict before mutation.
- Rename never overwrites. Case-only rename on a case-insensitive filesystem uses
  an owned intermediate name with rollback on finalization failure.
- Successful directory rename rebases registered descendants, preserves the
  selected opaque identity, and returns an updated entry summary.
- The operation emits queued, running, and one structured terminal event.

The coordinator serializes mutations in this slice. A later move/transfer phase
may replace the global execution guard with subtree-aware guards without changing
the operation or IPC contract.

## Second vertical slice

The next implementation enables single-entry local trash and confirmed permanent
deletion:

- Local entries advertise native-trash capability only on supported desktop
  targets and advertise permanent deletion separately. The UI never relabels one
  action as the other.
- `trash` 5.2.6 is locked as the platform adapter dependency. It is MIT-licensed,
  actively maintained, uses native macOS and Windows facilities, implements the
  FreeDesktop Trash specification on Linux, and deletes a symlink entry without
  following its target.
- The dependency remains behind an injectable `PlatformTrash` trait. It receives
  only a Rust-resolved path and no generic path or trash command is exposed to the
  webview. Its errors are mapped to stable, path-redacted Explora errors.
- The crate documents serialized mount-table access on Linux. Explora's concurrent
  Linux volume discovery reads `/proc/mounts` through `sysinfo` rather than calling
  the conflicting non-thread-safe libc iterator.
- Permanent deletion enters `awaitingConfirmation` with a random, single-use
  prompt ID. Rust supplies the target and location presentation; a mismatched,
  repeated, cancelled, or timed-out response cannot authorize deletion.
- Directory deletion builds a bounded post-order plan with `symlink_metadata` and
  never follows links. Planning is cancellable. Once removal begins, the
  operation is deliberately irreversible and reports its actual terminal result
  instead of falsely reporting late cancellation.
- Successful removal returns the invalidated opaque IDs so selection, previews,
  tabs, breadcrumbs, and histories can be reconciled without treating display
  paths as authoritative.

The adapter is covered through an injected fake in contract tests so automated
tests do not pollute a developer's real Trash. Packaged native validation is
required separately on macOS, Linux, and Windows.

## Third vertical slice

The third implementation enables single-entry moves within one local location:

- Directory references advertise `acceptMove` and `atomicReplace` separately
  from entry capabilities. The frontend uses them for presentation while Rust
  resolves and revalidates every opaque reference.
- Local relocation uses an exclusive, no-replace operating-system primitive:
  `renameat2`/`renameatx_np` through `rustix` on Linux and macOS, and `MoveFileW`
  on Windows. This closes the time-of-check/time-of-use overwrite race in both
  move and the earlier rename implementation.
- Move rejects roots, stale identities, cross-location references, unavailable
  filesystems, symlink destinations, and a directory's own subtree. It never
  follows a destination symlink or silently falls back to copy and delete.
- Conflicts are Rust-authoritative and permit only Keep Both, Skip, or Cancel.
  Keep Both chooses a bounded platform-safe name and repeats the exclusive
  relocation, so a concurrent creator is preserved rather than overwritten.
- Prompt waits do not hold the filesystem execution guard. The chosen operation
  reacquires the guard and revalidates its references before mutation, allowing
  unrelated operations to continue while a user decides.
- Successful directory moves rebase registered descendants and return their
  opaque IDs. The frontend refreshes affected source, destination, tabs, and
  previews without round-tripping display paths.
- The destination chooser navigates only typed directory references, disables
  incompatible locations, and keeps the final command unavailable until the
  destination capability and relationship checks pass.

At this stage, cross-location and cross-volume moves were reserved for Phase 6.
They require new identities, an owned partial target, verification and
finalization, and source removal only after those steps succeed.

## Fourth vertical slice

The fourth implementation enables single-entry SFTP mutation within one active
remote location:

- Connected, non-root SFTP entries advertise rename, move, and permanent-delete
  capabilities. Remote Trash remains false because no recoverable server-side
  trash contract exists.
- The backend revalidates each opaque reference against a bounded metadata
  fingerprint immediately before mutation. Directory relocation rebases every
  registered descendant while preserving its opaque identity; deletion
  invalidates the removed subtree.
- Rename and same-location move use the SFTP relocation request and never invoke
  a remote shell. Existing destinations remain conflicts. Keep Both generates a
  bounded backend-valid candidate and repeats the no-replace attempt.
- Recursive deletion constructs a bounded post-order plan with SFTP `lstat` and
  directory enumeration. It deletes symlink entries without traversing their
  targets and reports item-based progress.
- Permanent deletion always suspends for a random, single-use prompt whose host
  and target text comes from the active Rust session.
- A disconnect or timeout after dispatch is not retried. Before any confirmed
  deletion it returns `outcomeUncertain`; after earlier removals it returns
  `partialCompletion`, marks the session offline when connectivity is lost, and
  directs the user to reconnect and refresh.
- The disposable SFTP server is mutable and covers relocation, conflicts,
  permissions, symlinks, recursive partial completion, latency, timeout, and
  disconnect uncertainty over the real protocol.

At this stage, cross-location and cross-backend moves were reserved for Phase 6
because they cannot use the relocation path.

## Fifth vertical slice

The fifth implementation enables verified moves across local volumes, connected
SFTP locations, and backend boundaries:

- The coordinator copies each source to an exclusively created, least-privilege
  partial artifact owned by the operation. Finalization uses no-replace
  relocation and never overwrites a destination created concurrently.
- Regular files stream in bounded chunks and are compared byte for byte before
  source removal. Directory manifests are deterministic, limited to 100,000
  entries and 256 levels, preserve empty directories, and never follow symlinks.
- Source identity and content are revalidated after copying and immediately
  before cleanup. A changed source invalidates the copy rather than applying the
  original intent to new content.
- Cancellation and pre-finalization failures clean only operation-owned partial
  artifacts and preserve the source. A verified destination is preserved if
  source removal fails, with `partialCompletion` identifying the remaining
  source.
- Remote finalization and deletion are never replayed after a lost
  acknowledgement. The operation reports `outcomeUncertain`, takes the affected
  session offline where appropriate, and requires an explicit refresh.
- Cross-backend symlinks are recreated as links without traversing their targets.
  Unsupported Windows remote-link cases return a typed error instead of guessing
  a target type.

## Sixth vertical slice

The sixth implementation adds bounded batches and direct interaction surfaces:

- Move, Trash, and permanent deletion accept at most 1,000 unique entries from
  one source location. Requests containing both a directory and its descendant
  are rejected before mutation.
- Entries execute sequentially under one operation ID. Ordered per-entry results
  distinguish completed, failed, cancelled, and unstarted work, and cancellation
  is checked between entries.
- Permanent deletion uses one Rust-authoritative confirmation for the selected
  batch. Move conflicts remain per entry and expose only decisions supported by
  the active backend.
- Cut/paste stores typed opaque entry references only in frontend memory.
  Browser drag data carries only an internal marker; authoritative references
  remain in application state and are revalidated by Rust at execution time.
- Capability intersections gate every multi-entry action and drop destination.
  Explicit Move and cut/paste commands remain complete keyboard alternatives.

## Security review

- Mutation commands accept no local path, remote path, shell text, or generic
  filesystem option from the webview.
- Operation collections, names, IDs, and enum values are bounded and validated in
  Rust. Unknown fields are rejected.
- Opaque tokens remain scoped to the location that issued them, and location
  roots cannot be renamed through the entry action.
- Local identity revalidation detects replacement of a selected path before
  rename. Missing and replaced sources are reported as changed rather than
  applying the user's intent to a different item.
- Destination existence produces a conflict and never an implicit overwrite.
  Exclusive relocation preserves that invariant even when another process
  creates the destination after preflight validation.
- The case-only intermediate path is random, owned by the operation, and restored
  to the original path if finalization fails.
- Transfer partials have an explicit owned lifecycle, bounded creation and
  cleanup, no-replace finalization, byte or tree verification, and source
  revalidation before deletion.
- Remote acknowledgement loss is never retried automatically. Uncertain outcomes
  remain explicit, preserve any known-good side of the transfer, and require
  user-led reconnect and refresh.
- Batch bounds, same-location source requirements, overlap rejection, ordered
  outcomes, and capability intersections prevent frontend selection state from
  widening the authority of an operation.
- Browser cut and drag payloads never contain authoritative local or remote
  paths. Rust accepts only bounded, location-scoped opaque references and
  revalidates them immediately before mutation.

## Consequences

- Quick local actions and long-running transfers use the same lifecycle rather
  than separate mutation APIs.
- The frontend receives explicit feature availability but cannot use capabilities
  to bypass Rust authorization or execution-time validation.
- Registered tabs and histories can remain valid through same-backend directory
  relocation.
- Remote Trash remains visibly unavailable rather than silently degrading.
  Transfer-based moves are capability-gated and report unsupported destination
  semantics explicitly.
- Backend contract coverage is required before enabling a capability. Packaged
  native Trash and local-move validation remains required on macOS, Linux, and
  Windows.
