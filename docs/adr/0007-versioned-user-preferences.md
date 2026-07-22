# ADR 0007: Persist typed user preferences in the Rust application boundary

- Status: Accepted
- Date: 2026-07-20

## Context

Explora already exposes global controls for sidebar width, list or grid view, and
sorting, but those choices reset on every launch. Theme selection is independently
persisted by `mode-watcher`. Navigation references, tabs, selections, connections,
and file operations are session or domain state rather than user preferences.

A general browser-storage object would split ownership across webviews and make
validation, migration, and recovery difficult. The preference model also needs a
clear path toward later settings such as file-display behavior and customizable
keyboard shortcuts without committing to those interfaces before their action
registries exist.

## Decision

Rust owns a versioned `preferences.json` document in Tauri's application config
directory. The global layout section stores sidebar collapsed state, view mode,
sort descriptor, the standard location roles shown under Favorites, and a sparse
list of SSH target IDs hidden from the sidebar. IPC exposes a typed snapshot and
typed partial layout updates. Rust validates closed enums and bounded target IDs,
serializes concurrent writes under a mutex, performs file I/O away from the UI
thread, and atomically replaces the document. The file is owner-only on Unix.

Version 2 adds the favorite-role list. Version 1 documents migrate in memory with
all available standard favorites enabled, preserving their existing layout
choices. Favorite roles are deduplicated and stored in canonical sidebar order.
Version 3 adds hidden SSH target IDs; earlier documents migrate with every target
visible. IDs are bounded, validated, deduplicated, and sorted before persistence.

Missing preference files use defaults. Unreadable, malformed, or unsupported
documents also recover to defaults and return a structured warning rather than
blocking application startup. A successful update replaces the invalid document
with a valid current-version document.

The frontend applies preferences before revealing the initially hidden Tauri
window. Existing controls update the UI optimistically and serialize their
partial writes so a slow earlier write cannot overwrite a newer choice. Browser
development and component tests use an in-memory implementation of the same
typed preference contract.

Theme remains under `mode-watcher` for now. Tabs, navigation history, opaque path
references, selection, transient UI, operation state, connection state, and SSH
secrets are not preferences and are not added to this document.

## Future preference sections

New settings are added through explicit document-version migrations and typed
IPC fields, grouped by stable product concepts such as appearance, file display,
navigation, confirmations, and shortcuts. Unknown arbitrary JSON is not accepted.

Shortcut customization requires a stable command/action registry first. When it
is introduced, platform defaults remain in code and the preference document
stores only sparse user overrides keyed by semantic action ID. A binding model
must support normalized platform-aware modifiers, multiple bindings, an explicit
unbound state, conflict validation, and reset-to-default behavior. No placeholder
shortcut map is persisted before those semantics are implemented and tested.

## Consequences

Current layout choices survive packaged-app restarts and remain consistent across
locations. Users can hide or restore standard-folder shortcuts without removing
the underlying location or affecting an open tab. Hiding an SSH target likewise
does not edit, delete, or disconnect it; target lifecycle actions remain separate
from sidebar visibility. Arbitrary directory favorites, per-location, or
per-folder settings require a later product decision and stable backend-owned
location identities rather than display paths. A future unified settings screen
may migrate theme ownership, but must include an explicit compatibility path for
existing `mode-watcher` values.
