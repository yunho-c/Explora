# ADR 0002: Add SSH targets through a read-only SFTP backend

- Status: Accepted
- Date: 2026-07-18

> This ADR records the initial read-only SSH slice. ADR 0009 extends connected
> SFTP locations with capability-gated rename, same-location move, and permanent
> deletion while preserving the trust and opaque-reference boundaries below.

## Context

Explora treats remote computers as first-class file locations rather than as a
separate SSH client. The first remote slice needs useful OpenSSH compatibility,
safe host trust, ephemeral authentication, cancellable connection work, and the
same navigation contract as local folders. It must not expose a shell or turn
displayed remote paths into authoritative IPC input.

## Decision

Explora uses `russh` for SSH transport and authentication, `russh-sftp` for file
transport, and `russh-config` behind a small resolver for common OpenSSH options.
The resolver expands bounded `Include` directives, discovers concrete positive
`Host` aliases, and resolves `HostName`, `User`, `Port`, `IdentityFile`,
`IdentitiesOnly`, and `UserKnownHostsFile`. Wildcard and negated host patterns
are configuration rules rather than selectable targets and are not listed.

Manual targets store only a display name, host, port, username, initial path,
optional identity-file path, and `IdentitiesOnly` preference in a versioned JSON
document under the Tauri application configuration directory. The document is
written atomically and with owner-only permissions on Unix. Passwords,
passphrases, private-key contents, and keyboard-interactive answers are never
persisted.

Authentication tries the server's `none` method, then the SSH agent unless
`IdentitiesOnly` is active, then configured or standard identity files, password,
and keyboard-interactive authentication when the server offers them. Encrypted
keys and interactive methods produce typed, single-use UI prompts. Answers are
zeroed after use where the involved Rust buffers are under Explora's control.

Host verification uses the configured or standard `~/.ssh/known_hosts` file.
Known matching keys connect without prompting. Unknown keys require an explicit
SHA256 fingerprint confirmation before being appended. A changed key is a
blocking error and cannot be accepted through the routine first-use prompt.

Every saved remote target owns an opaque registry from random tokens to SFTP
paths. The registry survives routine disconnects so a manual reconnect can reopen
the current folder and retain Back/Forward history; editing or deleting the
target invalidates it. Directory listings use those tokens, emit the same bounded
event batches as local listings, and never parse shell output. Connection
attempts and listings have explicit request IDs and cancellation paths, including
while an SFTP response is delayed.

Client sessions use TCP no-delay, a 15-second idle keepalive interval, a maximum
of three unanswered keepalives, and a 30-second SFTP request timeout. An
unexpected transport close produces a typed disconnect event, removes the
session from the available-location view, and marks open frontend locations
offline without clearing their last directory contents or history. Reconnection
is explicit and user initiated; Explora does not automatically repeat operations
whose outcome might be uncertain. A manual refresh reloads the active directory
in place without adding navigation history.

This slice is read-only. It lists metadata and navigates directories but does not
upload, download, rename, delete, or execute remote commands.

## Unsupported configuration

`ProxyJump` is deferred until the transport can model each hop with the same host
verification and authentication guarantees. `ProxyCommand` is not executed.
Selecting an alias that depends on either directive returns a structured,
actionable unsupported error instead of silently ignoring the directive.

Other unsupported directives remain inert unless they affect the resolved
connection. The resolver must grow deliberately with compatibility tests rather
than executing arbitrary OpenSSH configuration behavior.

## Security review

- The webview receives target metadata, typed prompts, and opaque entry tokens,
  never a general SSH handle, raw private key, or shell primitive.
- Prompt IDs are scoped to the matching connection request and consumed once.
- Unknown and changed host keys have different outcomes; changed keys always
  block the connection.
- Credentials are not logged or stored. Prompt text originates from bounded SSH
  protocol fields and is rendered as text by Svelte.
- SFTP is required. Servers without a usable SFTP subsystem fail with an
  unsupported-server error instead of falling back to shell commands.
- Includes have a depth limit and cycle detection. Proxy commands are never
  launched.
- A disposable in-process SSH/SFTP server exercises real socket handshakes,
  first-use and changed host keys, identity files, encrypted keys, a Unix SSH
  agent, password and keyboard-interactive authentication, secret redaction,
  SFTP absence, permissions, symlinks, latency cancellation, connection loss,
  and opaque-reference reuse after reconnect. The agent scenario is Unix-only;
  Windows keeps its Pageant and named-pipe client paths behind platform-specific
  code.

## Consequences

OpenSSH configurations that depend on jump hosts cannot connect yet, though
their concrete aliases remain visible with a clear error. Connection state is
session-scoped, and users authenticate again after restarting Explora. Reconnect
is manual and does not yet include an automatic retry policy. Future write
operations, transfers, automatic retries, persistent secrets, or jump-host
support require separate capability, progress, conflict, and threat-model work.
