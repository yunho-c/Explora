# Filesystem actions

- Status: Accepted; phased implementation in progress
- Scope: Rename, move, trash, and permanent deletion for local and SSH/SFTP
  locations

## Summary

Explora can support filesystem mutations without weakening its existing opaque
reference, privacy, and trust boundaries. The filesystem APIs themselves are not
the difficult part. The substantial work is defining safe behavior for conflicts,
mutable paths, cancellation, partial transfers, native trash, remote disconnects,
and honest progress across three desktop platforms and two backend families.

The recommended design introduces a Rust-owned filesystem-operation coordinator,
a shared capability-driven backend contract, mutation-aware opaque path
registries, and a small typed IPC surface. The frontend presents user intent and
operation state; it never constructs authoritative paths or performs filesystem
work.

Delivery should start with a production-ready, single-entry local rename and
expand through local trash, same-filesystem moves, SFTP mutations, cross-backend
moves, and finally multi-selection. Requests should still use collections from
the beginning so adding multi-selection does not require replacing the IPC
contract.

The approximate effort for one engineer familiar with the codebase is:

| Capability                                                    | Approximate effort |
| ------------------------------------------------------------- | ------------------ |
| Local single-entry rename                                     | 3–5 working days   |
| Local rename, same-filesystem move, and native trash          | 2–3 weeks          |
| Polished local and SFTP rename, move, and deletion            | 3–5 more weeks     |
| Cross-backend moves with progress, verification, and recovery | 3–5 more weeks     |
| Complete stable-release interpretation, including platform QA | 6–10 weeks total   |

These estimates include relevant automated tests but not unexpected upstream or
platform-packaging work.

## Goals and boundaries

This design covers regular files, directories, and symlinks on local and SSH/SFTP
locations. “Entry” refers to any of those item types. Rename, move, trash, and
permanent delete must be available only when the selected entry and destination
advertise the required capability.

The implementation must:

- Preserve opaque, location-scoped references across IPC.
- Revalidate mutable filesystem state immediately before mutation.
- Never overwrite a destination silently.
- Prefer reversible local trash and make permanent actions unmistakable.
- Never follow symlinks implicitly during move, copy, verification, or deletion.
- Keep the window responsive and make long-running work cancellable.
- Preserve the source of a cross-filesystem move until the destination has been
  finalized and verified.
- Report partial and uncertain outcomes honestly.
- Apply the same domain and lifecycle model to local and remote locations.

The first shipped slice supports exactly one selected entry. The domain and IPC
shapes accept entry collections so multi-selection can be enabled later. The
following are deferred:

- Directory merging.
- Undo beyond recovery through the operating system's trash facility.
- Resuming operations after the application restarts.
- Automatic retry of an operation with an uncertain outcome.
- Drag-and-drop and cut/paste as initial interaction requirements.
- Background operation persistence or a durable transfer journal.

## Current implementation

Phases 1 through 5 are implemented for single entries. Native Trash and local
move still require packaged validation on every supported platform:

- Local and remote paths are represented by opaque, location-scoped tokens.
- Directory listings already use typed incremental events and cancellation.
- A Rust-owned operation coordinator emits queued, running, confirmation, and
  typed terminal events with monotonic sequences and item-based progress.
- Local entries advertise rename, move, native trash, and permanent-delete
  capabilities; directories separately advertise whether they accept moves.
  Connected SFTP entries advertise rename, move, and permanent deletion but not
  Trash.
- Rename preserves opaque identity and rebases registered descendants.
- Successful trash and permanent deletion invalidate registered descendants;
  frontend selection, previews, tabs, and histories reconcile those references.
- Native trash is implemented through a narrow, injectable platform adapter.
  Permanent deletion requires a single-use, Rust-authoritative confirmation.
- Same-location local moves use an operating-system no-replace primitive, reject
  symlink and descendant destinations, preserve registered identities, and offer
  Keep Both, Skip, or Cancel when a destination exists.
- SFTP rename and same-location move use protocol relocation without shell
  commands, preserve opaque identities, never replace a pre-existing target,
  and share the same Keep Both, Skip, or Cancel conflict flow.
- Remote permanent deletion requires a Rust-authoritative host-and-target
  confirmation. Recursive plans are bounded, item-progressed, post-order, and
  never follow symlinks.
- A connection loss or timeout after a remote mutation is dispatched becomes an
  `outcomeUncertain` error and takes the session offline without retry. Failure
  after some recursive removals becomes `partialCompletion`.
- The accessible destination chooser consumes opaque directory references and
  capability data. Cross-location destinations remain visibly unavailable until
  transfer-based moves are implemented.
- The frontend validates IPC responses through a replaceable data-source
  boundary.
- List and grid views expose capability-gated context and platform-keyboard
  actions with accessible blocking dialogs for permanent deletion and move
  conflicts.
- Disposable SSH/SFTP tests cover real authentication, trust, listing,
  relocation conflicts, permission failures, symlink-safe recursive deletion,
  partial completion, cancellation, disconnect, timeout, and reconnect behavior.

Packaged native trash and same-filesystem move must still be exercised on macOS,
Linux, and Windows before those phases are considered complete across all
supported targets.

The remaining work is transfer-based moves with verified finalization,
multi-selection and collision planning, undo, and the corresponding
cross-platform/native validation.

## Architecture

The operation coordinator is the authoritative owner of operation state. It
resolves references, validates capabilities, acquires path-level operation
guards, selects the correct execution strategy, and emits typed events. Backends
perform narrowly scoped filesystem primitives and do not make presentation or
conflict-policy decisions.

```text
Frontend action UI
        │ typed intent and responses
        ▼
Tauri filesystem-action commands
        │ validated DTOs
        ▼
FileOperationCoordinator
        ├── capability and stale-state validation
        ├── operation lifecycle and cancellation
        ├── conflict and confirmation suspension
        ├── path overlap guards
        └── transfer strategy and verification
                 │
        ┌────────┴────────┐
        ▼                 ▼
Local backend         SFTP backend
        │                 │
PlatformTrash         Remote permanent delete
        │
macOS / Linux / Windows
```

### Capability model

Capabilities must drive both the frontend and coordinator. Backend names must
not be used to decide whether an action is enabled.

Each entry summary should include an `EntryCapabilities` value with:

- `rename`: the entry can be renamed within its current directory.
- `move`: the entry can be relocated to a compatible destination.
- `trash`: the entry can be moved to a recoverable native trash location.
- `deletePermanently`: the entry can be deleted without recovery.

Directory summaries should include destination capabilities with:

- `acceptMove`: entries can be moved into the directory.
- `atomicReplace`: the backend can atomically replace a regular-file target.

The coordinator rechecks capabilities and relevant metadata at execution time.
Frontend capability state is advisory and may be stale.

Roots, disconnected locations, read-only mounts, and entries without sufficient
permissions expose the corresponding capability as false. Remote locations do
not claim `trash` unless a later backend explicitly implements and tests real
server-side trash semantics.

### Backend contract

Local and SFTP backends should implement a shared filesystem contract containing
only the primitives needed by coordinators:

- Resolve and inspect an opaque entry or directory reference.
- Return current entry identity, type, size, timestamps, and capabilities.
- Relocate an entry within the backend, with an explicit no-replace or replace
  mode.
- Open a bounded streaming reader.
- Create an owned partial destination without overwriting another entry.
- Stream writes and finalize or abandon an owned partial destination.
- Reopen or inspect a finalized destination for verification.
- Permanently remove a file, an empty directory, or a symlink entry.
- Enumerate a directory for coordinator-owned recursive work without following
  symlinks.

Rename and same-backend move use the same relocation primitive. A rename changes
the final name while retaining the parent; a move supplies a different parent.
The coordinator, not the backend, decides whether relocation or transfer is the
correct strategy.

Local trash remains a separate `PlatformTrash` adapter because it is an operating
system integration rather than a generic filesystem primitive. The adapter
receives a Rust-resolved local path and returns a structured result. It must not
expose a generic path or trash command to the webview. The concrete dependency or
native implementation must receive a focused maintenance, license, packaging,
and security review before adoption.

### Operation requests

The frontend starts an operation through one typed command. The conceptual request
shape is:

```text
FileOperationRequest
  sources: EntryRef[]
  action:
    Rename { newName }
    Move { destination: DirectoryRef }
    Trash
    DeletePermanently
```

`start_file_operation` validates the DTO, allocates an opaque operation ID,
registers an event channel, and returns without waiting for completion. The first
delivery phases require `sources` to contain exactly one item.

Two additional commands complete the surface:

- `respond_file_operation(operationId, promptId, response)` answers an
  authoritative confirmation or conflict prompt. Prompt IDs are single-use and
  scoped to their operation.
- `cancel_file_operation(operationId)` requests best-effort cancellation. It is
  idempotent and never reports completion before cleanup and required finalization
  have finished.

The commands accept no authoritative path strings, shell fragments, or
backend-specific options.

### Lifecycle and events

Every operation moves through an explicit state machine:

```text
queued
  └── running
        ├── awaitingConfirmation ──┐
        ├── awaitingConflict ──────┤── running
        ├── completed
        ├── cancelled
        └── failed
```

Terminal states are immutable. An event sequence is monotonically numbered so
the frontend can reject duplicated or stale events. Events contain:

- Operation ID, sequence, action, and lifecycle state.
- Completed and total item counts.
- Completed and total bytes when the total is known.
- The current entry's display name, not a newly authoritative path.
- A confirmation or conflict prompt when user input is required.
- A structured terminal result with per-item outcomes.
- A structured error for failed, partial, or uncertain operations.

The progress UI uses an indeterminate state whenever byte totals cannot be known
honestly. A completed state means all required finalization, verification,
registry updates, and cleanup succeeded.

### Error model

The existing error categories remain and should be extended with:

- `conflict`: the destination already exists or became occupied.
- `sourceChanged`: the source no longer matches the state validated for the
  operation.
- `destinationUnavailable`: the target directory, volume, or connection became
  unavailable.
- `partialCompletion`: some requested items completed and others did not.
- `outcomeUncertain`: a remote connection ended after a mutation was sent but
  before its result could be established.

Errors remain structured across IPC and contain safe presentation text. They must
not include secrets, raw remote protocol messages, or private-key material. Full
remote paths should not be logged by default.

### Concurrency and cancellation

The coordinator should allow unrelated quick operations to proceed while
preventing overlapping destructive work. Before execution, it acquires operation
guards for the source subtree and destination name or subtree. Overlapping
operations queue rather than racing.

Transfer and recursive-operation concurrency must be bounded globally and per
backend. Cancellation is checked:

- Before metadata and capability validation.
- Before and after each backend request.
- Between directory entries and transfer chunks.
- Before finalization.
- Before source deletion in a move.

Cancellation cannot revoke an atomic rename or a remote request that the server
may already have applied. In that situation the coordinator refreshes observable
state and reports the established result or `outcomeUncertain`; it never silently
retries.

Operations survive tab changes and navigation but not application restart. On
shutdown, cancellable work is asked to stop and owned partial destinations are
cleaned up where that can be proven safe.

## Opaque reference lifecycle

Mutation changes the paths currently stored behind opaque references. Refreshing
the directory alone is insufficient because tabs, breadcrumbs, histories, and
preview state may still hold those references.

Both local and remote path registries need two mutation operations:

- `rebaseSubtree(locationId, oldPath, newPath)` updates every registered path at
  or below a relocated directory while preserving its opaque token.
- `invalidateSubtree(locationId, path)` removes every registered path at or below
  a permanently removed entry.

Same-backend rename and move perform the filesystem mutation and registry rebase
as one coordinator-owned critical section. If registry rebasing unexpectedly
fails after a successful mutation, the affected location's registry is revoked
and its views are reloaded rather than serving incorrect references.

Trash and permanent deletion invalidate the source subtree after the backend has
established success. Cross-backend move cannot preserve an identity across
locations: the destination receives new references on refresh, and source
references are invalidated only after destination verification and successful
source deletion.

A valid reference presented under another location remains invalid. Display
paths and names never become authoritative mutation inputs.

## Operation behavior

### Rename

Rename is the first vertical slice because it exercises validation, conflicts,
mutation-aware references, refresh, keyboard behavior, and structured errors
without requiring transfers or native trash.

The UI enters inline editing for one selected entry. Rust validates the proposed
name for the source backend, rejects empty or reserved values, and revalidates the
source immediately before relocation. Platform-specific filename constraints
come from the backend; shared code must not assume POSIX separators, case
sensitivity, Unicode normalization, or Windows name rules.

Rename never overwrites an existing entry. A name conflict keeps the editor open,
selects the editable name where practical, and presents an inline error. A
case-only rename on a case-insensitive filesystem uses a safe intermediate name
inside the coordinator when a direct atomic rename cannot implement it.

Renaming a symlink renames the link itself. Renaming a directory rebases all
registered descendants. Root locations cannot be renamed through this action.

### Same-backend move

The initial Move command opens an in-app destination chooser using the same local
and remote location model as the explorer. The chooser returns an opaque directory
reference and never a display path.

When the source and destination share a backend and filesystem that supports
atomic relocation, the coordinator uses the relocation primitive. It prevents a
directory from moving into itself or one of its descendants and revalidates both
source and destination immediately before execution.

If the destination name exists, the coordinator emits a conflict prompt whose
allowed decisions are determined by entry type and backend capabilities:

- `keepBoth`: choose a backend-valid, non-conflicting name and continue.
- `skip`: leave the source unchanged.
- `cancel`: cancel the operation.
- `replace`: offer only for regular-file targets when atomic or staged safe
  replacement is implemented and tested.

Directory merging and replacing a directory are not offered in the first stable
implementation.

### Cross-filesystem and cross-backend move

A move that cannot be implemented as atomic relocation becomes a coordinated
copy, verify, and delete operation:

1. Revalidate the source and destination and resolve any name conflict.
2. Create an explicitly owned partial destination with non-executable,
   least-privilege permissions where applicable.
3. Stream bytes or directory entries with bounded buffers and honest progress.
4. Preserve entry type and deliberately supported metadata. Do not follow
   symlinks.
5. Finalize the destination only after the entire copy succeeds.
6. Reopen or inspect the result and verify required size, type, and content
   integrity.
7. Permanently remove the source only after verification succeeds.
8. Invalidate source references and refresh both locations.

If copy, finalization, or verification fails, the source remains untouched. The
coordinator removes only partial resources it can prove it created. Any leftover
is reported with actionable, non-sensitive information.

Directory moves traverse client-side through backend primitives. SFTP moves do
not execute remote shell commands. A failed source deletion after verified copy
is a partial completion: both copies remain and the UI explains that the source
could not be removed.

### Local trash

Local deletion uses the operating system's native trash facility whenever the
entry capability permits it. Routine trash does not require confirmation because
it is reversible through the platform. The action must still report permission,
volume, and platform failures clearly.

If native trash is unavailable for the selected entry, Explora must not silently
fall back to permanent deletion. It presents a blocking confirmation that states
the action is permanent and displays the target. Only an affirmative response to
the operation's single-use prompt allows execution.

Successful trash invalidates source references and refreshes affected views. The
first implementation does not provide an in-app Restore command; recovery uses
the operating system's trash UI.

### Remote permanent deletion

Remote entries do not advertise trash. Delete is presented as Delete Permanently
and always requires confirmation. The Rust coordinator supplies the authoritative
host and target presentation used by the confirmation; the frontend does not
construct it from arbitrary strings.

Recursive directory deletion enumerates children without following symlinks,
removes children, and then removes the empty directory. Progress is item-based
unless byte totals can be computed without an expensive unbounded traversal.

A disconnect after an SFTP deletion request was sent may make the result
uncertain. Explora must not reconnect and repeat the deletion automatically. It
reports the uncertainty, preserves the tab and history, and offers reconnect and
refresh so the user can inspect the actual state.

## Frontend behavior

The frontend should keep file-operation presentation in a feature-scoped store
rather than adding the complete lifecycle to the central explorer state. The
store subscribes to operation events, rejects stale sequences, owns pending
prompts, and exposes active and recently completed operations.

Core entry points are:

- Context-menu Rename, Move…, Move to Trash, and Delete Permanently actions.
- Platform-appropriate keyboard shortcuts for rename and deletion.
- An explicit Move… command and destination chooser as the keyboard alternative
  to future drag-and-drop.

Capabilities control whether commands are shown and enabled. Permanent and
reversible actions use distinct labels and icons. Color is never the only
distinction.

Quick rename errors remain inline. Conflicts and permanent-deletion confirmations
are blocking dialogs because they require a decision. Progress and ordinary
failures are non-modal so navigation remains usable. Dialogs restore focus to the
originating entry or the nearest surviving entry.

After a successful rename, selection follows the preserved opaque identity. After
move, trash, or deletion, selection advances predictably to a neighboring entry.
Closing Quick Preview cancels or reconciles a preview whose entry is removed;
renaming an entry updates the displayed name without treating its content as a
different file.

The destination chooser must support keyboard navigation, local and connected
SSH locations, offline states, loading, empty folders, and cancellation. It must
not allow an invalid descendant destination or a location without `acceptMove`.

## Security review requirements

Filesystem mutations expand Explora's privileged boundary and require a focused
security review and accepted ADR before the first implementation is merged. The
review must verify:

- The webview cannot supply authoritative local or remote paths.
- Every DTO field, operation ID, prompt ID, name, collection size, and option is
  bounded and validated in Rust.
- Mutations cannot escape an authorized root through traversal, symlinks,
  remounts, or stale tokens.
- Permanent deletion cannot be reached through a mislabeled reversible action.
- Remote host and target confirmation data are Rust-authoritative.
- Partial targets have least-privilege permissions and an owned cleanup
  lifecycle.
- Logs and errors redact secrets and avoid full remote paths by default.
- No mutation path invokes a shell or parses remote shell output.
- Dependency choices for native trash and content verification are deliberate,
  pinned, licensed appropriately, and supported on all targets.

## Implementation strategy

Each phase should be a finished vertical slice with its own tests, documentation,
and native validation where applicable.

### Phase 1: Domain and coordinator foundation

- Record the accepted destructive-operation, conflict, trash, and cross-backend
  semantics in an ADR.
- Add capability, request, event, progress, prompt, result, and extended error
  types in Rust and TypeScript.
- Add the operation coordinator, monotonic event sequencing, cancellation,
  operation guards, and typed IPC commands.
- Add mutation-aware rebase and invalidation operations to local and remote path
  registries.
- Keep all frontend action controls disabled until an end-to-end action is ready.

### Phase 2: Local single-entry rename

- Implement local no-replace relocation and backend-specific name validation.
- Add inline list and grid rename with pointer and keyboard entry points.
- Preserve opaque identity and selection through file and directory rename.
- Cover conflicts, case-only renames, stale sources, symlinks, and permission
  errors.

### Phase 3: Local trash and permanent fallback

- Implement and review the `PlatformTrash` adapter on macOS, Linux, and Windows.
- Enable Move to Trash only when supported.
- Add the Rust-authoritative permanent fallback confirmation.
- Validate trash behavior in packaged applications on all three platforms.

### Phase 4: Same-filesystem move

- Add the accessible in-app destination chooser.
- Implement atomic relocation, descendant checks, conflict prompts, and keep-both
  naming.
- Refresh source and destination views and reconcile open tabs and previews.

Implemented for single-entry moves within one local location. Moving between
location roots, volumes, or backends is deliberately deferred to Phase 6 because
those operations require a new destination identity and verified transfer rather
than an in-place registry rebase.

### Phase 5: SFTP mutations

- Implement remote relocation, permanent file deletion, and bounded recursive
  directory deletion through SFTP only.
- Add remote confirmation, disconnect, timeout, and uncertain-outcome behavior.
- Extend disposable SFTP fixtures with conflict, permission, symlink, latency,
  and disconnect cases.

Implemented for single entries within one connected SFTP location. The backend
revalidates a bounded metadata fingerprint immediately before mutation, uses
opaque registry rebasing/invalidation, and serializes mutations per session.
Standard SFTP relocation is treated as no-replace; a destination is preflighted
and server failures are reconciled against source and destination metadata.
Recursive deletion is limited to 100,000 entries and 256 levels, emits honest
item counts, and stops automatic cancellation once irreversible removal begins.
Disconnects and request timeouts after dispatch are never retried. They mark the
location offline and report an uncertain or partial result for user-led reconnect
and refresh. Cross-location remote moves remain Phase 6 transfers.

### Phase 6: Transfer-based move

- Add partial-target creation, bounded streaming, progress, cancellation,
  finalization, integrity verification, and cleanup.
- Support cross-volume local moves and local-to-SSH, SSH-to-local, and SSH-to-SSH
  moves.
- Delete sources only after successful destination verification.
- Report partial completion when the verified copy remains but source deletion
  fails.

Implemented for single entries. The local-to-local path supports regular files,
directory trees, and symbolic links without following link targets. It snapshots
at most 100,000 entries and 256 levels, aggregates regular-file byte totals,
copies in bounded 256 KiB chunks, and checks cancellation between entries and
chunks. Owned partial files and trees use least-privilege creation, synchronize
file contents, finalize with a no-replace relocation, and clean themselves up on
copy, cancellation, finalization, or verification failure. Verification compares
the complete source and destination structure, link targets, metadata identity,
and every regular-file byte before source removal. Deletion removes only entries
from the verified snapshot, so a late-arriving child is preserved and produces a
partial result instead of being deleted without a copy. A failed source removal
preserves the verified destination and reports partial completion.

Regular-file streaming is implemented in every direction: local to SFTP, SFTP
to local, and SFTP to SFTP, in addition to local to local. Remote destinations
use exclusive, owner-only `.explora-partial-*` files. The coordinator streams and
reports bounded byte progress, flushes the partial, reopens both endpoints for a
byte-for-byte comparison, revalidates the source, and only then assigns the final
name without replacement. A cancellation or changed source abandons the hidden
partial and preserves the source. Remote finalization that loses its
acknowledgement is never replayed; Explora reports an uncertain outcome and
requires a refresh. After successful finalization, source deletion runs exactly
once. A rejected source deletion preserves the verified destination and reports
partial completion. SFTP-to-local transfers translate ordinary permission bits
without copying special mode bits.

Remote directory and symbolic-link transfers use the same bounded contract in
every local/SFTP direction. Remote manifests are deterministic, limited to
100,000 entries and 256 levels, preserve empty directories and link targets, and
never recurse through links. Hidden remote directory artifacts track exactly the
paths Explora created, so cancellation and failures clean those entries in
reverse order without scanning or following unexpected children. Complete trees
are structure-checked and every regular file is byte-verified before the root is
finalized. Source manifests are revalidated before cleanup, and remote source
removal deletes only verified entries; an unplanned late child prevents its
parent directory from being removed and produces a partial result.

The destination chooser enables every online location whose advertised
directory capability accepts moves, rather than checking backend names or
requiring the source location. On Windows, a remote symbolic link cannot be
recreated locally when SFTP provides no authoritative target-type metadata;
that direction returns an explicit unsupported error instead of following the
link or guessing whether to create a file or directory link.

### Phase 7: Multi-selection

- Permit multiple sources in the existing request shape.
- Add aggregate and per-item progress, batch conflicts, skip semantics, and
  structured partial results.
- Test mixed entry types and capability intersections.

### Phase 8: Additional interaction surfaces

- Implement drag-and-drop and cut/paste as clients of the same operation API.
- Keep explicit commands available as complete keyboard alternatives.

## Testing strategy

### Rust unit tests

Cover:

- Capability calculation and execution-time revalidation.
- Name validation across platform and backend rules.
- Lifecycle transitions, event ordering, prompts, cancellation, and terminal
  immutability.
- Overlapping operation guards and bounded concurrency.
- Conflict decisions and keep-both name generation.
- Registry subtree rebasing and invalidation.
- Structured conflict, partial, and uncertain errors.
- Symlink-safe recursive traversal and deletion.

### Backend contract tests

Run the same behavioral expectations against a temporary local tree and the
disposable SFTP server:

- Rename and move files, directories, and symlinks.
- Reject traversal and cross-location token misuse.
- Preserve sources on conflicts and failed transfers.
- Handle missing, changed, permission-denied, and read-only entries.
- Never follow symlinks during recursive work.
- Cancel between batches and transfer chunks.
- Produce equivalent structured results where backend capabilities match.

### Integration and fault tests

Cover typed IPC, stale-event rejection, tab navigation during operations, and
refresh reconciliation. Inject:

- Destination disappearance and volume removal.
- Full storage or write failure.
- Source mutation during copy.
- Disconnect before and after remote acknowledgement.
- Failed finalization, verification, partial cleanup, and source deletion.
- Application shutdown with owned partial resources.

### Frontend and accessibility tests

Cover:

- Inline rename success, error, cancellation, and focus restoration.
- Platform keyboard shortcuts and context-menu actions.
- Destination chooser navigation, invalid destinations, offline locations, and
  cancellation.
- Confirmation and conflict dialogs, including allowed-decision filtering.
- Determinate and indeterminate progress, cancellation, partial results, and
  non-modal errors.
- Screen-reader names, focus order, visible focus, selection reconciliation, and
  reduced motion.

### Native validation

Browser tests are useful for interaction logic but are not proof of filesystem or
platform integration. Packaged validation must exercise:

- Native trash and permanent fallback on macOS, Linux, and Windows.
- Same-volume and cross-volume moves.
- Real local permission and conflict failures.
- Real SFTP rename, move, recursive delete, disconnect, and reconnect.
- Progress and cancellation without freezing the window.

## Acceptance criteria

The full capability is complete only when:

- No destination is overwritten without an explicit supported decision.
- No recursive operation follows a symlink implicitly.
- Cross-backend moves retain the source until destination finalization and
  verification succeed.
- Local deletion is recoverable whenever native trash is available.
- Remote and fallback permanent deletion require explicit, correctly labeled
  confirmation.
- Cancellation, partial completion, and uncertain remote outcomes are reported
  truthfully.
- All operations have an ID, structured lifecycle, cancellation path, and typed
  result.
- The UI remains responsive and core workflows are keyboard complete.
- Local and SFTP backend contract tests pass.
- Packaged native scenarios pass on macOS, Linux, and Windows.
