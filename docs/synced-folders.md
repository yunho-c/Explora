# Synced folders

- Status: Implementation in progress
- Last updated: 2026-07-23
- Tracking branch: `feat/synced-folders`

## Summary

Explora can treat folders managed by iCloud Drive, OneDrive, Google Drive, and
similar desktop sync clients as first-class locations without becoming a cloud
client or sync engine. The operating system or installed provider remains
responsible for authentication, synchronization, conflict resolution, caching,
and account lifecycle. Explora discovers the provider's user-visible filesystem
root and browses it through a bounded backend.

This feature is called **synced folders** rather than cloud drives. "Drive" is
already associated with physical and mounted volumes, while a provider may
expose a folder, virtual volume, File Provider domain, Cloud Files sync root, or
GIO mount depending on the platform and configuration.

The first implementation should remain read-only, matching Explora's current
local and SSH/SFTP backends. File mutations, pinning, eviction, and explicit
provider control are later decisions.

## Product boundary

Synced folders are local or operating-system-mounted locations whose contents
are managed by software outside Explora. Supporting them means that Explora may:

- Discover user-visible sync roots already registered or mounted on the device.
- Show one location per provider account or sync root.
- List locally known metadata without downloading file contents.
- Report availability such as local, online-only, partial, syncing, offline, or
  unknown when the platform exposes it safely.
- Read a file through a bounded operation after any required download is made
  explicit to the user.
- Preserve tabs and navigation state when a provider becomes unavailable.

It does **not** mean that Explora will:

- Implement synchronization, conflict resolution, or provider databases.
- Sign users into cloud accounts or store OAuth tokens and provider credentials.
- Call iCloud, Microsoft Graph, Google Drive, or other cloud-service APIs.
- Claim that a provider has uploaded or synchronized a change unless the
  operating system reports that state authoritatively.
- Silently hydrate online-only files while gathering metadata or preparing a
  preview.
- Depend on folder names, account email addresses, or display paths as
  authoritative identities.

The product charter distinguishes provider-neutral discovery of OS-managed
locations from provider API integration and synchronization. The former is an
accepted Explora feature; the latter remain out of scope.

## User experience

Discovered roots appear in a **Cloud Storage** section of the sidebar, separate
from Favorites, physical Locations, and SSH targets. The section should support
multiple accounts from the same provider and should not collapse two distinct
roots merely because they share a display name.

Each location should show:

- A provider-neutral location name supplied by the operating system where
  possible.
- A provider icon only when the provider can be identified reliably; otherwise
  use the generic cloud-folder icon.
- Connected, offline, paused, or error state when known.
- Non-sensitive supporting text, such as "OneDrive" or "Google Drive". Account
  email addresses should not be exposed by default or written to logs.

Once opened, a synced folder uses the same tabs, breadcrumbs, selection, list and
grid views, sorting, keyboard navigation, and Quick Preview surface as other
locations. Backend differences should appear only when availability, latency,
capabilities, or safety differ.

An online-only file remains selectable. An operation that needs content should
present a clear action such as **Download to preview**, followed by honest,
cancellable progress where the platform supports it. If hydration cannot be
cancelled safely, the UI must say so and must not represent UI cancellation as
proof that the provider stopped downloading.

## Architecture

### Responsibilities

Add a `SyncedFolderManager` beside the existing `VolumeManager`. Physical volume
discovery and synced-folder discovery have different identity, lifecycle, and
availability semantics and should not be combined into one platform probe.

The manager should own:

- Platform-specific discovery adapters.
- Normalization, identity derivation, deduplication, and snapshot revisions.
- Provider-root lifecycle notifications plus a bounded polling fallback.
- Registration and revocation of browseable roots with the appropriate
  filesystem backend.
- A typed, cancellable IPC subscription for complete snapshots.

The frontend should own:

- Sidebar composition and visibility preferences.
- Tab, navigation, selection, and presentation state.
- User intent to hydrate content or retry an offline location.
- Rejection of stale discovery and availability events.

The backend remains authoritative for paths, provider handles, availability,
capabilities, hydration tasks, and errors.

### Discovery contract

Platform adapters should implement a narrow contract conceptually equivalent to:

```rust
trait SyncedFolderDiscovery {
    fn discover(&self) -> Result<Vec<DiscoveredSyncedFolder>, ExplorerError>;
}

struct DiscoveredSyncedFolder {
    stable_identity: PlatformSyncedFolderIdentity,
    display_name: String,
    provider: SyncedFolderProvider,
    access: SyncedFolderAccess,
    status: SyncedFolderStatus,
}
```

`SyncedFolderAccess` may contain a local `PathBuf`, an operating-system storage
handle, or a GIO location. It must never cross IPC. A platform adapter must not
pretend that a non-file URI is a local path.

Snapshots should be complete and monotonically revisioned, following the volume
discovery pattern. Removing a root revokes its opaque references before the new
snapshot is published. Open tabs retain an offline tombstone so a transient sync
client restart does not discard user context.

Backends keep a bounded tombstone history containing opaque location/reference
IDs only. Lifecycle failures have deliberately different meanings:

- `invalidReference` means the identity was never valid for that location (or
  was claimed under the wrong location).
- `unavailable` means a recently known provider root or mounted volume was
  removed. A still-configured manual root that cannot currently be reached uses
  `offline` instead.
- `staleReference` means the location is active again, but the caller retained
  an entry token from an earlier root lifetime and must refresh it.

The history is capped and may eventually forget old tombstones; it does not
retain filesystem paths, provider URIs, or account labels. The TypeScript data
source preserves recognized structured codes on `ExplorerFilesystemError` so UI
recovery does not need to parse human-readable messages. An `unavailable` or
`offline` listing marks the dynamic location offline immediately, even if the
next discovery snapshot has not arrived; the tab remains in place while preview
and hydration work is cancelled. A `staleReference` retries only at a newer
registered root and resets that tab's history rather than guessing a path.

Snapshot reconciliation also treats a changed opaque root token as a new root
lifetime, even when no intermediate offline snapshot was observed. Every tab for
that location resets to the new authoritative root before listing resumes. This
handles fast provider restarts without retaining invalid entry references.

### Location model

Filesystem transport and presentation source are separate concerns. A synced
folder may use the existing local filesystem implementation, a platform storage
adapter, or a future GIO backend. Provider identity must not select filesystem
behavior.

The current frontend contract is:

```ts
type LocationBackend = "local" | "gio" | "ssh";
type LocationKind = "local" | "volume" | "syncedFolder" | "ssh";

type SyncedFolderProvider = "iCloud" | "oneDrive" | "googleDrive" | "other";

interface SyncedFolderMetadata {
  provider: SyncedFolderProvider;
  status: "available" | "offline" | "paused" | "error" | "unknown";
  source: "system" | "manual";
}
```

The important invariant is that provider names and string ID prefixes do not
become backend dispatch. Rust resolves an ID against registered local roots,
active GIO roots, or active SSH sessions and rejects unknown or ambiguous
identities. Availability inspection is also selected by a Rust-owned access
policy—iCloud metadata, Windows Cloud Files, known local mirror, or
unknown—rather than provider brand.

### Identity and privacy

Stable IDs should derive from opaque platform identifiers when available. If a
fallback must include a path or provider identifier, hash it in Rust with a
namespaced application identifier before exposing it. Display names, account
names, and paths are presentation data and cannot be round-tripped as authority.

Local synced-folder summaries use the sanitized location name as their display
root. Descendant display paths are relative to that name, so physical provider
roots such as account-specific File Provider directories never cross IPC merely
to render a breadcrumb, tooltip, or accessibility description. Root reuse is
validated against the Rust path registry; it never compares or accepts the
presentation string as path authority.

Local and GIO synced-folder listings run in blocking workers because provider
namespace calls can be synchronous. Explora waits at most 30 seconds and allows
at most four concurrent workers across both transports. Cancellation wakes the
IPC command immediately and reaches `GCancellable` for GIO, while the worker
retains its concurrency permit until the provider call actually returns. This
bounds abandoned native work without falsely claiming that a provider-owned
call was cancelled. A cancellation flag is rechecked immediately after opening
the namespace and before any event is emitted, preventing a late worker from
publishing into a newer navigation lifetime.

Discovery and errors must not log:

- Account email addresses or tenant names.
- Full synced-folder or file paths.
- Provider database contents or opaque authentication data.
- File names or content except in deliberately local diagnostic tooling.

Two roots are duplicates only when their authoritative identity or canonical
filesystem object proves that they are the same root. Similar names are not
sufficient. Nested roots require an explicit policy so the same tree is not
presented twice or authorized under ambiguous location identities.

### Placeholder and availability model

Cloud-backed entries may exist in the namespace without their bytes being
available locally. Add optional availability metadata without changing ordinary
local entries into provider-specific objects:

```ts
type ContentAvailability =
  | "local"
  | "onlineOnly"
  | "partial"
  | "downloading"
  | "syncing"
  | "error"
  | "unknown";
```

Listing must request only metadata that does not hydrate content. Logical file
size and allocated local size are different values and must not be conflated.
Unknown availability should remain unknown rather than being shown as local.

Content access follows a task lifecycle:

1. Revalidate the entry reference, location identity, and current availability.
2. If bytes are not local, return a typed `downloadRequired` result rather than
   opening the file as a side effect.
3. Start hydration only after explicit user intent.
4. Publish structured progress when the platform provides trustworthy progress;
   otherwise use an indeterminate state.
5. Revalidate the file after hydration, then hand it to the bounded preview or
   streaming pipeline.
6. Keep cancellation and provider completion distinct. A cancelled Explora task
   may stop waiting even when an OS-owned download cannot be cancelled.

The content-request capability is valid only for a regular file. Revalidation
returns `notFound` if the entry was removed and drops the capability if the path
became a directory, symlink, or special entry. An in-flight request whose file
type or provider access policy changed fails with `staleReference`; it must not
report successful hydration merely because a non-file replacement is locally
present.

No preview, metadata extractor, search, thumbnail job, or recursive operation
may silently launch one hydration task per entry.

### IPC and capabilities

Prefer a small typed command/event surface:

- `watch_synced_folders(request_id, channel)`
- `cancel_synced_folder_watch(request_id)`
- `add_synced_folder()` opens a Rust-owned native directory picker where the
  snapshot reports that capability and returns only an opaque location ID; the
  selected path is not a command result.
- `remove_synced_folder(folder_id)` removes only a manually configured location.
- `request_content(request_id, entry_id, location_id, channel)` is available only
  when preview metadata carries a `downloadToPreview` capability. It publishes
  `started`, availability-based `progress`, and locally revalidated `complete`
  events.
- `cancel_content_request(request_id)` stops Explora's bounded wait. The current
  capability reports `providerWorkCancellable: false`, so the UI states that the
  operating-system download may continue.
- A later generic content-availability query may reuse this task vocabulary for
  remote and other delayed-content backends where practical.

Actions should be enabled from capabilities, not provider or platform names.
Examples include `canReadMetadata`, `canReadContent`, `requiresHydration`,
`canRequestHydration`, `canCancelHydration`, `canPin`, and `canEvict`. Pin and
evict actions are not part of the first slice.

## Platform strategy

### macOS

Modern OneDrive and Google Drive installations use Apple's File Provider
technology and normally expose user-visible roots beneath
`~/Library/CloudStorage`. Google Drive documents that location for File Provider
mode; legacy Google Drive installations may instead use `/Volumes/GoogleDrive`.
OneDrive also delegates Files On-Demand behavior and disk accounting to File
Provider.

Discovery enumerates accessible children of the standard user-visible
cloud-storage location. Folder-name matching may improve a display label but
must not establish identity or capabilities. The implementation does not read
private File Provider extended attributes: no documented provider-neutral client
API has yet been identified for third-party placeholder availability, so those
entries remain `unknown`. iCloud Drive uses a separate adapter because its
user-visible root and documented ubiquity APIs have different semantics.

Namespace accessibility and provider status are separate. macOS supplies no
provider-neutral root connection status for these locations, so system-discovered
iCloud and third-party roots report provider status `unknown` even while their
registered local namespace remains browsable. Explora does not turn directory
presence into a green provider-health claim.

Foundation exposes ubiquitous-item state for iCloud content, including whether
an item is ubiquitous, whether it is downloading, any download error, and its
current download status. The macOS adapter reads only those URL resource values;
it never calls `startDownloadingUbiquitousItem`. It maps a current local copy to
`local`, a not-downloaded item to `onlineOnly`, an active download to
`downloading`, a reported error to `error`, and a downloaded-but-stale copy to
`syncing` so preview stays gated until the copy is current. Missing, unexpected,
and non-ubiquitous values remain `unknown`. These iCloud-specific keys are never
used to infer third-party File Provider state.

Listing inspects availability only for regular files. Directories, symlinks, and
special entries remain metadata-only and are not followed or opened. Preview
revalidates the opaque entry reference and current availability before content
access; only a file reported as a current local copy enters the bounded preview
reader.

Explicit content-request authorization remains policy-specific. The documented
iCloud ubiquitous-item request is allowed when item metadata requires it even
though root provider status is unknown; the operating-system request and later
item revalidation are authoritative. Windows Cloud Files hydration remains
disabled when its separately queryable provider status is unknown. Neither
decision depends on a displayed provider name.

The initial packaged application should be tested both with locally available
and evicted files. Future Mac App Store sandboxing would change folder-access
requirements and requires a separate security review; a path accessible in a
non-sandboxed development build is not proof that it will be accessible in every
packaging model.

### Windows

Use `Windows.Storage.Provider.StorageProviderSyncRootManager.GetCurrentSyncRoots`
as the primary provider-neutral discovery mechanism. It returns currently
registered sync roots and may include legacy roots. This avoids guessing
OneDrive folder names or Google Drive letters, both of which can vary by account,
organization, policy, and user configuration.

The Windows adapter enumerates that registry directly. It hashes each opaque
registration ID into Explora's stable location identity and decodes only the
provider component before the first `!` as a display hint. The SID and account
components never enter UI state, logs, or persisted metadata. Duplicate paths
and identities are removed before roots are published, and the existing bounded
refresh detects registration and removal.

For a registered root, the adapter also queries
`CfGetSyncRootInfoByPath(CF_SYNC_ROOT_INFO_PROVIDER)` and consumes only the
provider status field. Known disconnected, connectivity-lost, terminated, and
error states are mapped conservatively; an unrecognized value or failed query
remains `unknown`. Provider status and namespace accessibility are separate: a
registered root remains browsable while its local namespace exists, even if its
provider is offline or in error. Cached local files can still be previewed, but
Explora does not offer or continue explicit placeholder hydration unless the
provider reports an available state.

Use the Windows Cloud Files API to inspect placeholder state. It distinguishes a
placeholder, sync root, in-sync item, partial item, and content that is only
partly on disk. Query state from directory enumeration metadata when possible so
listing does not open file content.

The availability adapter uses `FindFirstFileExW` to obtain file attributes and
the reparse tag, then passes only those values to
`CfGetPlaceholderStateFromAttributeTag`. It does not open file content or request
hydration. Partial and partially-on-disk placeholders map to `partial`; offline
or recall-on-access placeholders map to `onlineOnly`; non-placeholders and fully
local placeholders map to `local`; invalid or unreadable metadata remains
`unknown`. Preview revalidates this state and only reads content reported local.

`FOLDERID_SkyDrive` or documented OneDrive policy locations may be considered
only as bounded compatibility fallbacks. A fallback root must still be verified
as accessible and must not override a sync root returned by the platform API.

Windows tests must cover personal and work/school OneDrive roots, multiple
accounts, Google Drive streaming with a configurable drive letter or folder,
offline providers, and placeholders with each relevant Cloud Files state.

### Linux

There is no single provider-neutral local sync-root registry equivalent to the
Windows API. GNOME's `GVolumeMonitor` lists the user-interesting mounts a file
manager would normally show and emits mount lifecycle signals. GVfs may expose
Google Drive through a `google-drive://` GIO location when GNOME Online Accounts
and the corresponding backend are configured.

A GIO mount is not necessarily a POSIX filesystem path. Supporting it correctly
requires a GIO filesystem backend for listing, metadata, reads, cancellation,
and errors; resolving its URI through shell commands or assuming a FUSE mirror
is not an acceptable substitute.

The first Linux slice supports ordinary local paths through an explicit **Add
synced folder** action. The official Tauri dialog plugin is invoked only from an
async Rust command; its frontend commands are not granted, and the selected path
is not returned by the command. Explora rejects files and symlink roots,
canonicalizes the selected directory, derives a random opaque location identity,
and stores the OS path representation in a versioned owner-only configuration
file. This preserves non-UTF-8 Unix paths without making the display paths in
normal filesystem summaries authoritative.

Manually added roots use an explicit local-mirror availability policy, so known
local files may enter the existing bounded preview pipeline. If a saved root is
temporarily missing or no longer a real directory, it remains visible as offline
or errored and can still be removed. Removing it only forgets the Explora
location; it never touches files. There is no provider-neutral signal that
distinguishes an ordinary local sync-client folder from any other directory, so
automatic discovery of those roots remains unsupported without a future
documented OS source.

The accepted GIO slice enumerates `GVolumeMonitor` only on Linux and registers
non-native `google-drive://` roots already mounted by GNOME Online Accounts and
GVfs. It deliberately ignores SMB, SFTP, WebDAV, and other schemes because a
user-interesting mount is not by itself evidence that the location is a synced
folder. The monitor is installed from Tauri setup on the GTK/GLib main thread;
mount add, change, and remove signals refresh an internal snapshot, while the
existing bounded synced-folder refresh publishes revisioned sidebar state.

GIO URIs and account labels remain Rust-only. The backend issues cancellable,
no-follow directory enumeration on a blocking worker and maps GIO error domains
to structured Explora errors without returning provider messages. The shared
synced-listing limiter caps local and GIO provider opens at four workers with a
30-second command deadline. A timed-out GIO request is cancelled and cannot emit
its Started event later; its worker continues to hold capacity until the native
call actually exits. Quick Preview is an explicit read: GIO streams at most the
byte limit selected by the existing preview pipeline into an owned temporary
file, cancellation reaches `GCancellable`, and decoding then uses the same
five-second timeout, concurrency, format, pixel, and resource limits as local
preview. The temporary file is deleted when preparation finishes.

The initial Linux support matrix is capability-based. GNOME-compatible sessions
with GLib/GIO 2.56 or newer, a user D-Bus session, GNOME Online Accounts, and the
GVfs Google backend receive automatic Google Drive mounts. The packaged native
validation targets are the current Ubuntu LTS GNOME desktop and current Fedora
Workstation. KDE Plasma and Linux sessions without GVfs retain explicit local
folder selection; their native picker behavior remains a separate validation
item. No distribution-specific provider database or FUSE path is consulted.

The target-gated `gio` Rust dependency matches the gtk-rs 0.18 generation already
present in Tauri's Linux dependency graph. The Rust bindings are MIT-licensed and
link to the distribution's GLib/GIO runtime; Explora does not bundle a GVfs
provider backend. GIO may load extension modules installed by the desktop, but
Explora exposes only the allowlisted `google-drive` scheme as a synced folder.

## Implementation strategy

### Phase 0: approve the boundary

- Write an ADR accepting OS-managed synced folders as a product feature.
- Update `AGENTS.md` to distinguish this feature from provider API integrations
  and synchronization.
- Confirm whether the first release is discovery and read-only browsing only, or
  also includes explicit placeholder hydration.
- Decide how synced-folder visibility preferences should be persisted.

### Phase 1: shared model and deterministic UI

- Add provider-neutral location source, provider, status, availability, and
  capability types in Rust and TypeScript.
- Add strict IPC parsing and malformed-response tests.
- Add deterministic demo synced folders and placeholder entries.
- Add the Cloud Storage sidebar section, multiple-account labels, empty state,
  offline state, keyboard behavior, and visibility preferences.
- Add frontend tests without claiming native discovery coverage.

### Phase 2: macOS discovery vertical slice

- Implement native discovery and stable identities.
- Register real filesystem roots through the opaque local path registry.
- Watch for provider-root changes with a bounded polling fallback.
- Inspect documented iCloud availability metadata without requesting content.
- Preserve tabs as offline tombstones when a provider disappears.
- Validate in a packaged Tauri application with real providers.

### Phase 3: Windows discovery vertical slice

- Enumerate registered sync roots through the Windows storage-provider API.
- Add Cloud Files placeholder-state metadata without hydration.
- Add native lifecycle observation or bounded refresh.
- Validate OneDrive and Google Drive configurations in a packaged Windows build.

### Phase 4: Linux vertical slices

- Support explicitly added local sync folders through a Rust-owned picker.
- Add only reliably detected local sync folders when a provider-neutral source
  can prove their identity.
- Use `GVolumeMonitor` lifecycle signals on the GLib main loop and accept only
  explicitly supported GIO URI schemes.
- Add a GIO backend with cancellable listing and bounded reads before displaying
  non-file GVfs roots.
- Validate GNOME Online Accounts and common non-GNOME fallback behavior.

### Phase 5: explicit hydration

- Use at most four concurrent five-minute waits with typed lifecycle events and
  structured cancellation, timeout, offline, stale-reference, and platform
  errors.
- Use internal iCloud and Windows Cloud Files access policies rather than
  displayed provider brands to select native adapters.
- Treat progress as indeterminate unless availability changes are authoritative;
  do not synthesize byte percentages.
- Revalidate the opaque entry after hydration, then reopen the existing bounded
  preview pipeline so its byte, decoding, image, PDF, time, and resource limits
  still apply.
- Test offline, slow, cancelled, oversized, changed, and removed files.

## Progress checklist

### Cross-platform

- [x] Accept a synced-folders ADR.
- [x] Update the product boundary in `AGENTS.md`.
- [x] Define location source separately from backend transport.
- [x] Define synced-folder provider, status, availability, and capabilities.
- [x] Replace string-prefix backend routing with authoritative dispatch.
- [x] Implement `SyncedFolderManager` and complete revisioned snapshots.
- [x] Bind opaque references to synced-folder location identities.
- [x] Revoke references before publishing root removal.
- [x] Add Cloud Storage sidebar UI and visibility preferences.
- [x] Add deterministic demo roots and frontend tests.
- [x] Add structured offline, permission, stale, and unavailable errors.
- [x] Add privacy and log-redaction tests.
- [x] Document the supported platform/provider matrix in `README.md`.
- [x] Add a least-privilege macOS, Windows, and Linux CI matrix that compiles and
      tests target-gated adapters without pretending synthetic runners are real
      provider validation.

### macOS

- [ ] Verify discovery behavior on the minimum supported macOS version.
- [x] Discover accessible third-party File Provider roots.
- [x] Implement iCloud Drive discovery separately.
- [x] Derive stable opaque identities without exposing account data.
- [x] Detect provider-root addition, removal, and client restart with bounded polling.
- [x] Inspect documented iCloud availability without hydrating content.
- [x] Start iCloud download only from explicit Download to Preview intent.
- [x] Keep Explora wait cancellation distinct from the OS-owned iCloud request.
- [x] Keep third-party availability unknown pending a documented provider-neutral API.
- [x] Keep macOS root provider status unknown without disabling documented
      iCloud item requests.
- [x] Add an ignored, privacy-safe native smoke for discovery, opaque
      registration, and time-bounded provider-namespace opening.
- [x] Bound provider-owned namespace opens with prompt command cancellation, a
      30-second deadline, late-event suppression, and four retained worker
      permits.
- [ ] Test multiple OneDrive and Google Drive accounts.
- [ ] Test locally available, online-only, downloading, and failed items.
- [ ] Test permission denial and future sandbox implications.
- [ ] Run packaged macOS native smoke tests.

### Windows

- [x] Add the required Windows Storage Provider API bindings.
- [x] Enumerate current sync roots generically.
- [x] Normalize provider display metadata without path-name heuristics.
- [x] Read Cloud Files placeholder state from listing metadata.
- [x] Hydrate the complete placeholder only from explicit Download to Preview intent.
- [x] Run synchronous Cloud Files hydration off the async runtime and treat UI
      cancellation as stopping the wait only.
- [x] Detect sync-root registration and removal with bounded refresh.
- [x] Model provider disconnects that do not unregister the sync root.
- [x] Type-check sync-root discovery, placeholder metadata, and explicit
      hydration against the Windows MSVC target independently of unrelated
      native dependencies.
- [x] Add an ignored, privacy-safe native smoke for registered-root discovery,
      opaque registration, and time-bounded namespace opening.
- [ ] Test OneDrive Personal and work/school roots.
- [ ] Test multiple OneDrive accounts.
- [ ] Test Google Drive streaming to a drive letter and folder.
- [ ] Test online-only, partial, pinned, in-sync, and error states.
- [ ] Verify no listing or metadata operation hydrates a file.
- [ ] Run packaged Windows native smoke tests.

### Linux

- [x] Define the initial supported desktop and distribution matrix.
- [x] Confirm ordinary local sync-client folders have no provider-neutral
      discovery source; retain explicit selection instead of path heuristics.
- [x] Provide an explicit add-folder fallback through a Rust-owned native picker.
- [x] Persist manual roots with opaque IDs and owner-only, non-UTF-8-safe storage.
- [x] Keep unavailable manual roots visible, offline, and removable.
- [x] Keep picker results and dialog capabilities behind Rust; expose paths only
      as non-authoritative display data in normal filesystem summaries.
- [x] Evaluate `GVolumeMonitor` and mount lifecycle integration.
- [x] Accept a narrow, read-only GIO backend for the first stable release.
- [x] Implement cancellable GIO listing and bounded preview reads before exposing
      `google-drive://` mounts.
- [x] Bound GIO namespace opens with the shared 30-second deadline and four-worker
      provider cap, retaining capacity until native work exits.
- [x] Add an ignored, privacy-safe native smoke for Google Drive GIO discovery,
      opaque registration, and time-bounded namespace opening.
- [ ] Test GNOME Online Accounts Google Drive where supported.
- [ ] Test environments without GVfs or a running GLib main loop.
- [ ] Verify packaging does not silently load unapproved GIO modules.
- [ ] Test the native folder picker on representative GNOME and KDE sessions.
- [ ] Run packaged Linux native smoke tests.

Native Linux provider, picker, and packaging verification is deferred as of
2026-07-23 because no representative Linux host is currently available. The CI
matrix still provides target compile and deterministic test coverage once run,
but these native checklist items remain open and must not be inferred from CI.

### Hydration and preview

- [x] Add an entry availability indicator with an accessible text label.
- [x] Return `downloadRequired` instead of triggering implicit hydration.
- [x] Add explicit Download to Preview intent.
- [x] Model hydration as a bounded task with honest progress.
- [x] Separate cancellation of waiting from cancellation of provider work.
- [x] Revalidate identity and metadata after hydration.
- [x] Feed downloaded bytes into existing preview limits.
- [x] Prevent background metadata, search, and thumbnail hydration.
- [x] Add deterministic coverage for timeout decisions, offline and cancelled
      requests, oversized and malformed previews, and changed or removed files.
- [ ] Exercise those failure states against real platform providers.
- [x] Test deterministic cleanup, tab continuity, and root replacement after a
      provider disconnect.
- [ ] Test cleanup and tab continuity with real provider disconnects.

## Validation requirements

Unit and contract tests should cover identity stability, deduplication, root
removal, stale reference rejection, provider metadata validation, availability
mapping, and cancellation. Frontend tests should cover the sidebar, keyboard
navigation, offline tombstones, availability labels, and hydration decisions.

Native integration tests need controllable platform fixtures or test adapters
for discovery events and placeholder states. At least one real-provider smoke
scenario per supported platform is required because browser tests and synthetic
filesystem trees cannot prove File Provider, Cloud Files, or GVfs behavior.

The GitHub Actions matrix is configured to run `format:check`, `lint`, `check`,
and `test` using the locked repository command surface across macOS, Windows,
and Ubuntu. Its token is read-only, external actions are pinned to full commit
SHAs, and Bun is pinned to the version used to validate the lockfile. This is
intended to catch target-gated compile and deterministic test failures. The
ignored provider smokes remain manual because hosted runners have no
user-authenticated provider roots; the first remote workflow run is still
required evidence that the runner images and system prerequisites are correct.

On macOS on 2026-07-22, an isolated `x86_64-pc-windows-msvc` compile harness
type-checked the actual sync-root discovery, Cloud Files placeholder inspection,
and hydration modules against their pinned Windows bindings. That check exposed
and verified a missing `OsStrExt` import in the provider-status path. This is
useful target-compile evidence, but it does not execute WinRT or Cloud Files and
is not Windows runtime proof. The full application cross-check still belongs on
the native CI runner because unrelated SSH crypto dependencies require the
Windows SDK.

The macOS native smoke can be run without logging provider paths or account
labels:

```sh
cargo test --manifest-path src-tauri/Cargo.toml \
  native_macos_roots_register_and_open_without_provider_authority_crossing_ipc \
  -- --ignored --nocapture
```

Equivalent target-native discovery and namespace-opening smokes are available
on Windows and Linux. They emit aggregate root counts only:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml `
  native_windows_roots_register_and_open_without_provider_authority_crossing_ipc `
  -- --ignored --nocapture
```

```sh
cargo test --manifest-path src-tauri/Cargo.toml \
  native_linux_google_drive_mounts_register_and_open_without_uris_crossing_ipc \
  -- --ignored --nocapture
```

These smokes require a real configured provider on their host and deliberately
stop after the provider namespace opens. They are native adapter evidence, not
packaged UI proof and not placeholder hydration coverage.

On macOS 15.6 on 2026-07-22, the release `.app` and DMG packaged successfully.
The native smoke discovered and registered four installed OS-managed roots; two
opened through the local backend within the five-second diagnostic deadline and
two provider namespace opens stalled or failed. Production IPC now bounds those
calls as described above, but this remains useful provider-behavior evidence—not
a completed packaged-app UI smoke.

Report discovery, listing, placeholder inspection, hydration, and packaged UI
evidence separately. A successful browser workflow is not native proof, and a
locally mirrored folder does not exercise online-only placeholder behavior.

## Decisions for the first slice

- Discovery and metadata-only browsing shipped before the explicit, preview-only
  content-request slice; no other operation hydrates content.
- Users may hide individual discovered roots; provider-wide visibility is not
  stored.
- Duplicate provider roots receive sanitized ordinal labels such as
  `OneDrive 1` and `OneDrive 2`; account identifiers stay private.
- Explicit Linux roots receive stable generic labels and are treated as local
  mirrors; no provider is inferred from the selected folder name.

## Open decisions

- Which packaging models are supported on macOS, and will a sandboxed build need
  security-scoped user selection?
- Which availability states can each platform report authoritatively without
  opening content?

## References

- Existing physical-volume decision:
  [ADR 0008](adr/0008-cross-platform-volume-discovery.md)
- Existing opaque local path decision:
  [ADR 0001](adr/0001-opaque-local-path-references.md)
- Existing bounded preview decision:
  [ADR 0003](adr/0003-bounded-local-preview-pipeline.md)
- Apple File Provider manager and domain APIs:
  [Apple Developer Documentation](https://developer.apple.com/documentation/fileprovider/nsfileprovidermanager)
- Apple ubiquitous-item download status key and values:
  [URL resource key](https://developer.apple.com/documentation/foundation/urlresourcekey/ubiquitousitemdownloadingstatuskey),
  [download status](https://developer.apple.com/documentation/foundation/urlubiquitousitemdownloadingstatus)
- Apple explicit ubiquitous-item download request:
  [Apple Developer Documentation](<https://developer.apple.com/documentation/foundation/filemanager/startdownloadingubiquitousitem(at:)>)
- Google Drive File Provider and legacy locations on macOS:
  [Google Drive Help](https://support.google.com/drive/answer/12178485?hl=en-GB)
- Google Drive streaming and mirroring behavior:
  [Google Drive Help](https://support.google.com/drive/answer/13401938?hl=en)
- OneDrive Files On-Demand on macOS:
  [Microsoft Support](https://support.microsoft.com/en-US/onedrive/save-disk-space-with-onedrive-files-on-demand-for-mac)
- Windows registered sync-root enumeration:
  [Microsoft Learn](https://learn.microsoft.com/en-us/uwp/api/windows.storage.provider.storageprovidersyncrootmanager.getcurrentsyncroots)
- Windows sync-root registration identity:
  [Microsoft Learn](https://learn.microsoft.com/en-us/uwp/api/windows.storage.provider.storageprovidersyncrootinfo.id)
- Windows sync-root information query:
  [Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/nf-cfapi-cfgetsyncrootinfobypath)
- Windows sync-root provider information:
  [Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/ns-cfapi-cf_sync_root_provider_info)
- Windows sync-provider status values:
  [Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/ne-cfapi-cf_sync_provider_status)
- Windows Cloud Files placeholder states:
  [Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/ne-cfapi-cf_placeholder_state)
- Windows metadata-only placeholder-state helper:
  [Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/nf-cfapi-cfgetplaceholderstatefromattributetag)
- Windows explicit placeholder hydration:
  [Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/nf-cfapi-cfhydrateplaceholder)
- GIO volume monitor and user-visible mounts:
  [GNOME API Documentation](https://docs.gtk.org/gio/class.VolumeMonitor.html)
- GIO mount semantics:
  [GNOME API Documentation](https://docs.gtk.org/gio/iface.Mount.html)
- GIO native-path and virtual-file semantics:
  [GNOME API Documentation](https://docs.gtk.org/gio/iface.File.html)
- GVfs Google Drive scheme and GOA requirement:
  [GNOME GVfs Documentation](https://wiki.gnome.org/Projects%282f%29gvfs%282f%29schemes.html)
- Rust-owned native folder selection:
  [Tauri Dialog Plugin](https://v2.tauri.app/plugin/dialog/)
