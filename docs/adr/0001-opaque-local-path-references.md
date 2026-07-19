# ADR 0001: Authorize local navigation with opaque path references

- Status: Accepted
- Date: 2026-07-18

## Context

Explora must browse the full local filesystem without giving its webview a generic
read-path command. Display paths cannot be authoritative because they are
untrusted IPC data and cannot faithfully represent every path supported by the
host operating system. Local navigation must also remain compatible with a future
SSH backend that uses a different path representation.

## Decision

The Rust process owns every local `PathBuf`. It maintains an application-session
registry from random UUID tokens to paths and a reverse map that keeps tokens
stable during the session. Tauri IPC sends only these tokens, location identity,
and lossy display strings.

The backend seeds the registry with Home and available standard folders. A
successful directory listing may register that directory's parent, ancestors,
and immediate children. This intentionally permits traversal across the full
filesystem from an issued reference while preventing the webview from submitting
an arbitrary raw path. The operating system remains the authority for access;
Explora never elevates privileges or bypasses permission errors.

The first command surface is read-only: list locations, stream one directory,
and cancel a listing. Directory work runs on Tauri's blocking executor, emits
bounded batches, and checks a cancellation flag between entries. Symlinks are
reported as symlinks and are not traversed recursively; a symlink to a directory
is followed only when the user explicitly opens it.

## Security review

- Display names and paths are presentation data and are never resolved back into
  filesystem paths.
- UUID tokens contain no path material and expire when the process exits.
- A compromised webview can revisit and traverse references already issued to
  it, which matches the authority of a full-filesystem file explorer, but cannot
  manufacture a raw-path request.
- No filesystem plugin, shell command, write operation, privilege elevation, or
  remote origin is introduced by this decision.
- Rust preserves `PathBuf` and `OsString` values internally so lossy display
  conversion does not break navigation on systems that permit non-UTF-8 names.

## Consequences

The registry grows as the user discovers paths and is intentionally scoped to the
application session. Persisted tabs will eventually need backend-resolved
bookmarks rather than persisted tokens. A bounded eviction policy may be added if
long-running usage demonstrates meaningful registry growth, but it must not
invalidate active tab history without an explicit recovery path.
