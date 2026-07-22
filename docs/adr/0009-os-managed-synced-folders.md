# ADR 0009: Discover operating-system-managed synced folders

- Status: Accepted
- Date: 2026-07-22

## Context

Explora should make folders managed by iCloud Drive, OneDrive, Google Drive, and
similar desktop clients easy to reach without becoming a synchronization client.
These locations may be ordinary local folders, virtual filesystems, File Provider
domains, Cloud Files sync roots, or GIO mounts. Some entries have a visible name
and metadata before their content is available locally.

The existing location model mixes presentation category with backend transport:
`local`, `volume`, and `ssh` are represented by one `kind` field, while Rust
commands infer the backend from an `ssh:` identifier prefix. That convention does
not scale safely to operating-system-managed locations and makes an unknown
location look local by default.

Explora's product boundary excludes provider APIs, application-owned credentials,
and file synchronization. Discovering roots that the operating system already
manages does not require any of those capabilities, but content hydration and
provider lifecycle still need explicit, honest UI states.

## Decision

Explora treats these locations as **synced folders**. The installed provider and
operating system continue to own authentication, synchronization, conflicts,
caching, and account lifecycle. Explora discovers user-visible roots, registers
their authoritative access handles in Rust, and exposes only opaque location and
entry references to the webview.

Location summaries carry backend transport separately from presentation kind.
The initial transports are `local` and `ssh`; presentation kinds include local
favorites, physical volumes, synced folders, and SSH locations. Synced-folder
metadata contains only a normalized provider category and an OS-reported status.
Provider names never select backend behavior.

A `SyncedFolderManager` owns platform discovery, stable identities, complete
monotonically revisioned snapshots, bounded refresh, and registration/revocation
with the local filesystem boundary. It remains separate from `VolumeManager`
because physical media and provider roots have different identity, lifecycle,
availability, and user-action semantics.

The first slice is read-only discovery and browsing. Listing may inspect locally
available namespace metadata but must not open file content. If content is not
available locally, preview and future operations return a typed state instead of
silently triggering hydration. Explicit hydration, pinning, eviction, provider
APIs, and application-owned synchronization require later vertical slices.

Platform adapters use the strongest provider-neutral facility available:

- macOS discovers accessible user-visible File Provider roots and iCloud Drive,
  preserving path authority in Rust and validating behavior in a packaged app.
- Windows uses the Storage Provider sync-root registry and Cloud Files metadata.
- Linux may use ordinary local roots immediately; non-file GIO/GVfs locations
  require a real GIO backend before they are shown as browseable.

On Linux, the initial fallback lets the user select an ordinary local directory
through a Rust-owned native folder picker. The picker returns the selected path
only to Rust, and its command result contains only an opaque location identity.
Explora stores the path in a versioned owner-only configuration file, including
raw OS path data needed to preserve non-UTF-8 names. Display paths in normal
read-only filesystem summaries remain presentation-only and are never accepted
back as authority. Removing the location changes only Explora's configuration;
it never deletes or modifies the selected directory.

## Security and privacy invariants

- Display paths, folder names, account labels, and provider names are never
  authoritative identifiers.
- Stable public IDs are namespaced opaque values derived in Rust; raw provider
  identities and account email addresses do not cross IPC or enter logs.
- Removing a root revokes every opaque path reference for that location before
  the removal snapshot is published.
- A non-file URI is never coerced into a local path or passed through a shell.
- Native folder selection and saved-path authority remain behind Rust; the
  webview receives no generic dialog or filesystem permission.
- Metadata, preview, search, and thumbnail work may not trigger unbounded or
  implicit hydration.
- Unsupported platforms and unavailable providers report honest capability and
  status rather than falling back to provider-name checks.

## Consequences

- The sidebar can present one coherent Cloud Storage section while browsing uses
  the same navigation and selection model as other locations.
- Multiple accounts and roots from one provider remain distinct even when their
  sanitized display names match.
- Backend dispatch becomes authoritative and rejects unknown location IDs rather
  than treating them as local.
- The location and preference schemas gain versioned fields and strict IPC
  validation.
- Native discovery and placeholder behavior require per-platform packaged tests;
  deterministic browser data remains UI evidence only.
- Explora does not claim to synchronize files and does not need cloud-service
  credentials or network API permissions for this feature.

Detailed implementation phases and progress checklists live in
[`docs/synced-folders.md`](../synced-folders.md).
