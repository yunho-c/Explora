# ADR 0011: Add location-scoped integrated terminals

- Status: Accepted
- Date: 2026-07-22

## Context

Explora already treats local and SSH-backed directories as related locations and
keeps privileged filesystem and network behavior behind typed Rust boundaries. A
bottom terminal pane can make common file-oriented work substantially faster, but
it also introduces deliberate process-execution authority, untrusted terminal
control sequences, long-lived byte streams, and platform-specific lifecycle
behavior.

The original stable-release boundary excluded terminal emulation and remote
shells. ADR 0002 also made the first SSH/SFTP slice explicitly shell-free. Adding
an integrated terminal therefore requires a product decision and a narrower
security contract than either a generic process-launch command or a standalone
SSH client.

## Decision

Explora will include a collapsible, resizable terminal pane scoped to the
application window. Users may create multiple terminal sessions. A new local
session starts in the authorized local directory active at creation time. A new
remote session reuses a connected SSH location's verified transport and opens an
explicit SSH PTY and shell channel. Because SSH has no portable request for
starting an interactive shell in an arbitrary directory, the baseline remote
session starts in the account's server-selected default directory and presents
that limitation honestly.

xterm.js owns terminal emulation and rendering in the Svelte frontend.
`portable-pty` owns local pseudoterminal creation across macOS, Linux, and
Windows. Rust owns session identity, process or SSH-channel lifetime, authorized
launch context, I/O ordering, backpressure, resize validation, and cleanup.

The WebView receives no generic process-spawn or execute-command primitive.
Terminal IPC is limited to:

- creating a terminal for an opaque, authorized location/directory reference;
- forwarding bounded input to an existing opaque session ID;
- resizing that session within validated limits;
- acknowledging ordered output for backpressure; and
- closing the session and observing structured lifecycle events.

The application chooses the local shell through a Rust-owned platform adapter.
It spawns the program with an argument-vector API and sets the local working
directory through the process API, never through an interpolated shell command.
Remote shell input begins only after an explicit user action creates the session.
Explora does not add Tauri's shell or filesystem plugin for this feature; the
existing application-owned command surface remains the only WebView authority.

Terminal sessions belong to the application window, not to explorer tabs.
Routine navigation and closing a file tab do not change or terminate a running
shell. Each terminal tab retains a clear local or remote identity and launch
context. SSH disconnects end affected remote sessions; Explora never silently
reconnects a shell or replays uncertain input.

The detailed lifecycle, IPC, UI, validation, and phased delivery design lives in
[`docs/terminal.md`](../terminal.md).

## Security review

- Output bytes, terminal titles, escape sequences, and remote banners are
  untrusted. They are rendered only through xterm.js and never inserted as HTML.
- xterm.js code, styles, fonts, and addons are bundled with the application. The
  terminal adds no remote origin or network listener and does not relax the CSP.
- Output-driven link opening, clipboard writes, notifications, file access, and
  other host integrations are disabled by default. Multiline paste requires an
  explicit confirmation.
- No terminal transcript, command history, input, output, or current working
  directory is logged or persisted by Explora.
- Session count, dimensions, input chunks, output in flight, scrollback, and
  shutdown time are bounded. Backpressure pauses reads rather than dropping or
  reordering terminal output.
- Local working directories come only from opaque references already authorized
  by the Rust filesystem registry. Display paths are never resolved back into
  process paths.
- Remote sessions reuse the existing host-key and ephemeral-credential policy.
  They do not expose raw SSH handles to the WebView and do not weaken SFTP's role
  as the filesystem transport.
- Closing or quitting ends owned local process groups and SSH channels with a
  bounded graceful period followed by platform-appropriate termination.

## Alternatives considered

Plain process pipes were rejected because they do not provide terminal job
control, dimensions, interactive application semantics, or portable Windows
ConPTY behavior. Tauri's shell plugin was rejected because it exposes the wrong
generic process-launch abstraction and does not replace a PTY lifecycle.

A localhost WebSocket transport was rejected because it would add a listener,
origin/authentication work, and another attack surface solely to move bytes
inside one desktop process. The typed Tauri channel remains preferred unless the
implementation spike proves it cannot meet measured throughput requirements.

Launching the system `ssh` command for remote terminals was rejected because it
would duplicate authentication and host-trust state, complicate secret prompts,
and weaken structured disconnect handling. The existing verified Rust SSH
connection should own the remote PTY channel.

## Consequences

The terminal becomes part of the first stable release and must be validated in
packaged applications on macOS, Linux, and Windows. Browser-only tests can prove
layout and frontend state but cannot prove PTY, ConPTY, process-tree, signal, or
SSH-channel behavior.

The application intentionally gains shell authority, increasing its threat
surface and dependency weight. That authority remains interactive and
session-scoped; it is not a reusable automation API. Features such as arbitrary
launch profiles, shell integration scripts, remote working-directory injection,
automatic link activation, command detection, terminal restoration, splits, and
persistent transcripts require separate review before adoption.

This ADR supersedes only the no-shell scope of ADR 0002. ADR 0002's SFTP
filesystem contract, host verification, authentication, secret handling, opaque
remote paths, and prohibition on parsing shell output for file operations remain
in force.
