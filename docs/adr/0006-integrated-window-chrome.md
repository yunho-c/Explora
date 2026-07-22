# ADR 0006: Integrate the application shell with guarded native window chrome

- Status: Accepted
- Date: 2026-07-20

## Context

Explora's tabs are a natural window-level surface, but the default decorated
window reserves a separate titlebar above them. Tauri provides native macOS
titlebar overlays, but it does not provide an equivalent integrated titlebar
with native Windows Snap Layout behavior and Linux desktop styling.

Owning all native integrations would require separate AppKit, Win32, GTK, and
Wayland lifecycle implementations. Window chrome is also a recovery boundary:
an initialization failure must never leave the only application window hidden
or without usable controls.

## Decision

Explora pins `tauri-plugin-decoration` 2.1.4 behind two narrow Rust commands and
a frontend lifecycle controller. The plugin keeps native AppKit traffic lights
on macOS, uses HTML controls with Win32 `HTMAXBUTTON` hit testing on Windows 11,
and derives HTML controls from the GTK theme on supported Wayland sessions.

The main window starts hidden with native decorations intact. After Svelte
mounts, the controller requests the overlay titlebar and reveals the window only
after the plugin marks the current document active. Activation rejection, a
five-second timeout, or a reveal failure restores and shows the native titlebar.
If restoration fails, Explora still attempts to show the window and reports the
degraded state without exposing native error details.

The 32-pixel tab strip is the application titlebar. Plugin-provided clearance
variables protect platform controls, and only non-interactive gaps are draggable.
Native fallback keeps the same tab strip below the operating-system titlebar and
disables application drag regions.

Unsupported Linux sessions, including X11, use native decorations. Custom Linux
controls are limited to the plugin's tested GNOME/Mutter and KDE/KWin Wayland
environments. Browser-only development does not activate window chrome.

## Security and dependency review

- The crate is pinned exactly and remains behind an Explora-owned controller so
  it can be replaced without coupling explorer state to plugin APIs.
- The optional `macos-transparency` feature is disabled because it uses private
  AppKit APIs and is unnecessary for integrated tabs.
- The plugin requires `withGlobalTauri`. Explora compensates with an explicit
  main-window capability containing only the window operations used by the
  controls and activation lifecycle.
- The content security policy permits the plugin's local stylesheet protocol but
  adds no remote script, network, frame, or content origin.
- Activation and fallback errors cross IPC as structured, non-sensitive values.
  There is no telemetry and no file, path, or SSH data enters this subsystem.

## Consequences

The integrated titlebar must be smoke-tested in packaged applications on each
supported operating system; browser Playwright tests do not prove native window
behavior. Windows 10 does not expose the Windows 11 Snap Layout menu. Unsupported
Linux compositors get a safe but less space-efficient native titlebar.

Because the dependency is new and has limited adoption evidence, upgrades require
a source review and the full window-chrome smoke matrix. A maintained fork is a
future option only if a platform defect cannot be addressed through documented
configuration or application styling.
