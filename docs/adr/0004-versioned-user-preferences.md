# ADR 0004: Persist typed user preferences in the Rust application boundary

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
directory. Version 1 stores one global layout section: sidebar collapsed state,
view mode, and sort descriptor. IPC exposes a typed snapshot and typed partial
layout updates. Rust validates closed enums, serializes concurrent writes under a
mutex, performs file I/O away from the UI thread, and atomically replaces the
document. The file is owner-only on Unix.

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
locations. Per-location or per-folder display settings would require a later
product decision and stable backend-owned location identities rather than display
paths. A future unified settings screen may migrate theme ownership, but must
include an explicit compatibility path for existing `mode-watcher` values.
