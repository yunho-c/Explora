# ADR 0008: Cross-platform physical volume discovery

- Status: Accepted
- Date: 2026-07-21

## Context

Explora's sidebar reserved a Locations section for mounted volumes, but the
native backend only registered a fixed set of user directories. Removable media
can disappear while listings and opaque path references remain active, so adding
discovery also changes the lifetime and authorization rules of local paths.

## Decision

Explora discovers mounted physical filesystems in Rust and exposes them through
the existing location model with `kind: "volume"` and `role: "volume"`. The boot
filesystem, fixed secondary disks, removable media, and optical media are in
scope. Network filesystems, pseudo-filesystems, RAM filesystems, and known loop
devices are excluded. Mount, unmount, and eject operations remain out of scope.

A shared volume manager obtains normalized snapshots from `sysinfo`. Platform
notifications accelerate refreshes: Disk Arbitration on macOS,
`WM_DEVICECHANGE` on Windows, and UDisks2 ObjectManager signals on Linux. A
bounded periodic refresh remains active because notifications can be coalesced
or missed and because UDisks2 is not universal. Linux reports a non-fatal warning
when it must rely on periodic discovery.

The frontend subscribes through a typed, cancellable IPC channel. Snapshots are
complete, monotonically revisioned, and merged without replacing local favorites
or SSH locations. A removed volume disappears from the sidebar, but referenced
tabs retain an offline tombstone and their current presentation. Reconnecting a
volume with the same derived identity resets affected tabs to its newly
authorized root.

Opaque local path tokens are bound to the location that issued them. Removing or
replacing a volume revokes every token for that location before the new snapshot
is published. The backend rejects a valid token presented with another location
identity as well as every revoked token.

## Consequences

- Volume enumeration and capacity reads stay off the webview thread.
- Native platform dependencies are target-specific and do not expand the Tauri
  command allowlist beyond the typed watch and cancellation commands.
- Volume IDs are deterministic opaque UUIDs. macOS prefers Disk Arbitration's
  persistent volume UUID; other platforms currently derive identity from
  filesystem, label, and mount metadata. Raw device identifiers are not sent to
  the frontend or persisted. Fallback identities remain stable across metadata
  and capacity changes, but remounting at a different path may produce a new ID.
- Polling provides eventual consistency when native notification services are
  unavailable, with at most a short delay.
- Safe eject requires a later ADR because it adds destructive-operation,
  authorization, progress, and platform-error semantics.
