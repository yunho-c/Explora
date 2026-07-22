# Synced folders

- Status: Design proposal
- Last updated: 2026-07-22
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

The accepted product charter currently lists cloud-provider-specific
integrations and file synchronization as out of scope. Before shipping this
feature, update that language to distinguish provider-neutral discovery of
OS-managed locations from provider API integration and synchronization.

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

### Location model

Filesystem transport and presentation source are separate concerns. A synced
folder may use the existing local filesystem implementation, a platform storage
adapter, or a future GIO backend. Provider identity must not select filesystem
behavior.

A possible frontend shape is:

```ts
type LocationBackend = "local" | "ssh" | "gio";
type LocationSource = "favorite" | "volume" | "syncedFolder" | "ssh";

type SyncedFolderProvider =
  | "icloud"
  | "onedrive"
  | "googleDrive"
  | "other";

interface SyncedFolderMetadata {
  provider: SyncedFolderProvider;
  status: "available" | "offline" | "paused" | "error" | "unknown";
}
```

This is illustrative, not a mandate to replace the current location DTO in one
large migration. The important invariant is that provider names and string ID
prefixes do not become backend dispatch. The current `ssh:` routing should be
replaced with an authoritative backend association before adding enough
backends for implicit fallback-to-local behavior to become unsafe.

### Identity and privacy

Stable IDs should derive from opaque platform identifiers when available. If a
fallback must include a path or provider identifier, hash it in Rust with a
namespaced application identifier before exposing it. Display names, account
names, and paths are presentation data and cannot be round-tripped as authority.

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

No preview, metadata extractor, search, thumbnail job, or recursive operation
may silently launch one hydration task per entry.

### IPC and capabilities

Prefer a small typed command/event surface:

- `watch_synced_folders(request_id, channel)`
- `cancel_synced_folder_watch(request_id)`
- A later generic content-availability query or event stream.
- A later task-based `request_content` operation, shared with remote and other
  delayed-content backends where practical.

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

Discovery should enumerate accessible children of the standard user-visible
cloud-storage location and inspect operating-system metadata such as the File
Provider domain extended attribute. Folder-name matching may improve a display
label but must not establish identity or capabilities. iCloud Drive requires a
separate adapter and live validation because its user-visible root and ubiquity
APIs do not have identical semantics to third-party File Provider domains.

Foundation exposes ubiquitous-item state for iCloud content, including whether
an item is ubiquitous and its current download status. A macOS adapter should
use native URL resource values where applicable. Do not assume those iCloud
keys describe every third-party File Provider placeholder.

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

Use the Windows Cloud Files API to inspect placeholder state. It distinguishes a
placeholder, sync root, in-sync item, partial item, and content that is only
partly on disk. Query state from directory enumeration metadata when possible so
listing does not open file content.

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

The first Linux slice may discover only synced folders that are real local paths
and allow users to add such a folder explicitly. GIO/GVfs support should be a
separate vertical slice with contract tests. The UI must describe the limitation
honestly rather than showing an unusable mount.

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
- Preserve tabs as offline tombstones when a provider disappears.
- Validate in a packaged Tauri application with real providers.

### Phase 3: Windows discovery vertical slice

- Enumerate registered sync roots through the Windows storage-provider API.
- Add Cloud Files placeholder-state metadata without hydration.
- Add native lifecycle observation or bounded refresh.
- Validate OneDrive and Google Drive configurations in a packaged Windows build.

### Phase 4: Linux vertical slices

- Support explicitly added and reliably detected local sync folders.
- Evaluate GIO dependencies, runtime availability, licensing, and packaging.
- Add a GIO backend before displaying non-file GVfs roots.
- Validate GNOME Online Accounts and common non-GNOME fallback behavior.

### Phase 5: explicit hydration

- Define the shared task, progress, cancellation, and error contract.
- Implement platform adapters without coupling them to provider brands.
- Route hydrated content into existing bounded preview and streaming pipelines.
- Test offline, slow, cancelled, oversized, changed, and removed files.

## Progress checklist

### Cross-platform

- [ ] Accept a synced-folders ADR.
- [ ] Update the product boundary in `AGENTS.md`.
- [ ] Define location source separately from backend transport.
- [ ] Define synced-folder provider, status, availability, and capabilities.
- [ ] Replace string-prefix backend routing with authoritative dispatch.
- [ ] Implement `SyncedFolderManager` and complete revisioned snapshots.
- [ ] Bind opaque references to synced-folder location identities.
- [ ] Revoke references before publishing root removal.
- [ ] Add Cloud Storage sidebar UI and visibility preferences.
- [ ] Add deterministic demo roots and frontend tests.
- [ ] Add structured offline, permission, stale, and unavailable errors.
- [ ] Add privacy and log-redaction tests.
- [ ] Document the supported platform/provider matrix in `README.md`.

### macOS

- [ ] Verify discovery behavior on the minimum supported macOS version.
- [ ] Discover accessible third-party File Provider roots.
- [ ] Implement and validate iCloud Drive discovery separately.
- [ ] Derive stable opaque identities without exposing account data.
- [ ] Detect provider-root addition, removal, and client restart.
- [ ] Inspect iCloud availability without hydrating content.
- [ ] Determine safe third-party File Provider availability metadata.
- [ ] Test multiple OneDrive and Google Drive accounts.
- [ ] Test locally available, online-only, downloading, and failed items.
- [ ] Test permission denial and future sandbox implications.
- [ ] Run packaged macOS native smoke tests.

### Windows

- [ ] Add the required Windows Storage Provider API bindings.
- [ ] Enumerate current sync roots generically.
- [ ] Normalize provider display metadata without path-name heuristics.
- [ ] Read Cloud Files placeholder state from listing metadata.
- [ ] Detect sync-root registration, removal, and provider disconnects.
- [ ] Test OneDrive Personal and work/school roots.
- [ ] Test multiple OneDrive accounts.
- [ ] Test Google Drive streaming to a drive letter and folder.
- [ ] Test online-only, partial, pinned, in-sync, and error states.
- [ ] Verify no listing or metadata operation hydrates a file.
- [ ] Run packaged Windows native smoke tests.

### Linux

- [ ] Define the initial supported desktop and distribution matrix.
- [ ] Discover accessible local synced folders without provider databases.
- [ ] Provide an explicit add-folder fallback.
- [ ] Evaluate `GVolumeMonitor` and mount lifecycle integration.
- [ ] Decide whether a GIO backend is accepted for the first stable release.
- [ ] Implement GIO listing and reads before exposing non-file mounts.
- [ ] Test GNOME Online Accounts Google Drive where supported.
- [ ] Test environments without GVfs or a running GLib main loop.
- [ ] Verify packaging does not silently load unapproved GIO modules.
- [ ] Run packaged Linux native smoke tests.

### Hydration and preview

- [ ] Add an entry availability indicator with an accessible text label.
- [ ] Return `downloadRequired` instead of triggering implicit hydration.
- [ ] Add explicit Download to Preview intent.
- [ ] Model hydration as a bounded task with honest progress.
- [ ] Separate cancellation of waiting from cancellation of provider work.
- [ ] Revalidate identity and metadata after hydration.
- [ ] Feed downloaded bytes into existing preview limits.
- [ ] Prevent background metadata, search, and thumbnail hydration.
- [ ] Test slow, offline, oversized, malformed, changed, and removed files.
- [ ] Test cleanup and tab continuity after provider disconnects.

## Validation requirements

Unit and contract tests should cover identity stability, deduplication, root
removal, stale reference rejection, provider metadata validation, availability
mapping, and cancellation. Frontend tests should cover the sidebar, keyboard
navigation, offline tombstones, availability labels, and hydration decisions.

Native integration tests need controllable platform fixtures or test adapters
for discovery events and placeholder states. At least one real-provider smoke
scenario per supported platform is required because browser tests and synthetic
filesystem trees cannot prove File Provider, Cloud Files, or GVfs behavior.

Report discovery, listing, placeholder inspection, hydration, and packaged UI
evidence separately. A successful browser workflow is not native proof, and a
locally mirrored folder does not exercise online-only placeholder behavior.

## Open decisions

- Is explicit hydration part of the first synced-folder slice, or does the first
  slice show metadata-only results for non-local content?
- Should users be able to hide individual roots, entire providers, or both?
- Should an explicitly added local folder be marked as synced only by the user,
  or may Explora infer that status from platform metadata?
- How should multiple roots with identical OS-provided display names be
  disambiguated without exposing account email addresses?
- Is GIO an accepted backend dependency, or should Linux initially support only
  ordinary local paths?
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
- Apple ubiquitous-item download status:
  [Apple Developer Documentation](https://developer.apple.com/documentation/foundation/urlresourcekey/ubiquitousitemdownloadingstatuskey)
- Google Drive File Provider and legacy locations on macOS:
  [Google Drive Help](https://support.google.com/drive/answer/12178485?hl=en-GB)
- Google Drive streaming and mirroring behavior:
  [Google Drive Help](https://support.google.com/drive/answer/13401938?hl=en)
- OneDrive Files On-Demand on macOS:
  [Microsoft Support](https://support.microsoft.com/en-US/onedrive/save-disk-space-with-onedrive-files-on-demand-for-mac)
- Windows registered sync-root enumeration:
  [Microsoft Learn](https://learn.microsoft.com/en-us/uwp/api/windows.storage.provider.storageprovidersyncrootmanager.getcurrentsyncroots)
- Windows Cloud Files placeholder states:
  [Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/ne-cfapi-cf_placeholder_state)
- GIO volume monitor and user-visible mounts:
  [GNOME API Documentation](https://docs.gtk.org/gio/class.VolumeMonitor.html)
- GIO mount semantics:
  [GNOME API Documentation](https://docs.gtk.org/gio/iface.Mount.html)
