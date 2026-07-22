# ADR 0009: Capability-driven filesystem actions

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
delete. Directory destinations will advertise move-acceptance and replacement
capabilities when those phases are implemented. The frontend uses these values to
present actions, while Rust revalidates references, capabilities, identity, and
filesystem state immediately before execution.

Operations use opaque IDs and monotonically sequenced events. The lifecycle
supports queued, running, awaiting-confirmation, awaiting-conflict, completed,
cancelled, and failed states. Terminal states are immutable. Cancellation is
best-effort; an atomic or remotely acknowledged mutation is never described as
cancelled merely because its result arrived after a cancellation request.

Local and SFTP implementations will converge on a shared backend mutation
contract. Same-filesystem rename and move use an explicit no-replace relocation
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
- The case-only intermediate path is random, owned by the operation, and restored
  to the original path if finalization fails.
- Transfer and SFTP phases still require focused review of their platform
  dependencies, partial-resource lifecycle, and uncertain-outcome behavior before
  their capabilities are enabled.

## Consequences

- Quick local actions use the same lifecycle that will support long-running
  transfers, avoiding a second mutation API later.
- The frontend receives explicit feature availability but cannot use capabilities
  to bypass Rust authorization or execution-time validation.
- Registered tabs and histories can remain valid through same-backend directory
  relocation.
- Remote and move actions remain visibly unavailable rather than silently
  degrading.
- Each subsequent phase must add backend contract coverage and native validation
  before enabling its capability.
