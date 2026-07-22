# Integrated terminal design

- Product decision: Accepted by [ADR 0009](adr/0009-integrated-terminal.md)
- Implementation status: Not started
- Target: Initial stable release

## Purpose

Explora's terminal is a fast, location-aware companion to file browsing. It
appears as a collapsible, resizable bottom pane similar to an editor terminal,
but it remains part of the file explorer rather than becoming a standalone
terminal application or general-purpose SSH client.

The feature must feel native enough for everyday interactive use while
preserving Explora's core boundaries:

- users explicitly create every shell session;
- local sessions can start in the active authorized directory;
- remote sessions belong to an already connected and verified SSH location;
- navigation never silently changes a running shell;
- terminal output never becomes filesystem authority; and
- process, PTY, SSH, and cleanup work stays in Rust.

This document defines the intended architecture and delivery strategy. It does
not describe functionality that exists in the current codebase yet.

## Product behavior

### Pane and sessions

The terminal pane sits below the main file view and above the status bar. It is
collapsed by default until the user creates or reveals a session. A vertical
resizer preserves file-view space and enforces sensible minimum sizes for both
surfaces.

The pane is window-scoped and contains terminal tabs. It is deliberately not
owned by an explorer tab:

- navigating to another directory does not run `cd` in an existing shell;
- closing an explorer tab does not kill its terminal;
- switching explorer tabs does not hide unrelated running work; and
- every terminal tab shows the location and launch context that created it.

Creating a terminal from a local directory starts the platform-selected shell
with that directory as its process working directory. Creating one from an SSH
location opens an interactive PTY/shell channel on the verified connection. SSH
does not define a portable way to start an interactive shell in an arbitrary
SFTP directory, so the initial remote implementation starts in the server's
default account directory. The UI must not imply otherwise. A later remote-CWD
feature would require a safe, explicitly supported shell-profile design rather
than interpolating paths into a command string.

Each session presents these states:

- `starting`: authority and transport are being established;
- `running`: input, resize, and output are active;
- `exited`: the process or remote channel ended normally or with a status;
- `failed`: startup or transport failed before normal interaction; and
- `closing`: graceful shutdown is in progress.

Exited sessions retain their visible scrollback until the user closes or restarts
them. Restart always creates a new opaque session; it never reuses uncertain
process or SSH-channel state.

### Commands and keyboard behavior

The baseline commands are:

- New Terminal
- Show/Hide Terminal
- Focus Terminal
- Next/Previous Terminal
- Rename Terminal (presentation only)
- Restart Terminal
- Close Terminal
- Close All Terminals

The default Show/Hide Terminal shortcut is `Ctrl` + backtick on every desktop
platform, matching the established integrated-terminal convention. Commands also
appear in menus or the command surface so they remain discoverable and keyboard
complete.

When xterm has focus, terminal input takes precedence over explorer navigation.
`Ctrl+C` sends an interrupt unless terminal text is selected, in which case the
platform copy convention applies. `Escape`, arrows, Space, and printable keys go
to the terminal rather than triggering file selection or Quick Preview. Global
window commands must use an explicit allowlist through xterm.js's custom key
handler instead of relying on event bubbling accidents.

Pasting multiple lines or text containing line breaks requires confirmation with
a concise preview and destination identity. Single-line paste uses the normal
platform command. Output can never trigger paste, clipboard access, or another
host action.

### Visual and accessibility behavior

The pane uses existing Explora tokens and stock resizable primitives. It should
look like part of the application shell: restrained borders, compact tabs, clear
focus, and no separate dashboard-like chrome.

Terminal colors derive from semantic application tokens, but ANSI colors remain
distinct and WCAG-conscious. Font family, size, line height, cursor style, and
scrollback are terminal preferences rather than one-off component constants.
Text scaling and high-contrast modes must remain usable, and resizing text must
re-fit the PTY.

xterm.js screen-reader support must be exercised and exposed through an
accessible preference or platform-sensitive default. Session state and exit
status use a concise `aria-live` region outside the terminal canvas. Pane tabs,
close buttons, the resizer, and all commands need visible focus and accessible
names. Terminal keystrokes must not be duplicated into an invisible accessibility
surface.

## Architecture

### Ownership boundary

```text
Svelte terminal feature
  TerminalPane + TerminalState + XtermAdapter
             │ typed TerminalDataSource
             ▼
Tauri IPC: create / input / resize / acknowledge / close + event channel
             │ opaque session and directory IDs only
             ▼
Rust TerminalCoordinator
       ┌─────┴──────────┐
       ▼                ▼
LocalPtyTransport   SshTerminalTransport
portable-pty        verified russh connection
       │                │
local process       SSH PTY + shell channel
```

The frontend owns pane presentation, focus, terminal tabs, xterm instances, and
user intent. Rust owns authoritative session state, launch authorization,
processes and remote channels, ordered byte transport, backpressure, exit status,
and cleanup.

Terminal state must not be added directly to the existing `ExplorerState` class.
The application shell composes `ExplorerState` and a separate `TerminalState`.
The only coupling is a read-only launch context obtained when the user creates a
session.

### Proposed code organization

```text
src/
├── app/
│   └── terminal-state.svelte.ts
├── features/terminal/
│   ├── TerminalPane.svelte
│   ├── TerminalTabs.svelte
│   ├── TerminalSurface.svelte
│   └── xterm-adapter.ts
└── lib/
    ├── contracts/terminal.ts
    └── data/
        ├── terminal-data-source.ts
        ├── tauri-terminal-data-source.ts
        └── demo-terminal-data-source.ts

src-tauri/src/terminal/
├── mod.rs
├── coordinator.rs
├── local.rs
├── remote.rs
├── transport.rs
└── types.rs
```

Names may evolve, but the coordinator, transport abstraction, IPC validation,
and frontend adapter boundaries should remain explicit.

### Domain model

`TerminalSessionId` is an unguessable, session-scoped opaque value. It contains
no PID, host, path, or backend information. Rust associates it with:

- the application window that owns it;
- local or SSH transport identity;
- the authorized location and launch context;
- lifecycle state and exit information;
- validated terminal dimensions;
- monotonically increasing output sequence numbers;
- bounded output awaiting frontend acknowledgement; and
- handles required for input, resize, termination, and worker cleanup.

The frontend receives a `TerminalSessionSummary` suitable for display:

```ts
type TerminalSessionState =
  | "starting"
  | "running"
  | "exited"
  | "failed"
  | "closing";

interface TerminalSessionSummary {
  id: string;
  state: TerminalSessionState;
  kind: "local" | "ssh";
  locationId: string;
  title: string;
  contextLabel: string;
}
```

`contextLabel` is presentation-only. It must never be sent back as a working
directory or remote path.

### Terminal transport contract

Local and remote terminals share a lifecycle contract rather than pretending
their implementations are identical. A spawned transport provides:

- a blocking or asynchronous ordered output source;
- a bounded input sink;
- resize support;
- a wait/exit signal;
- graceful close; and
- forced termination where the backend supports it.

The local transport uses `portable-pty`. It obtains a master/slave pair, starts a
Rust-selected default shell on the slave, drops the parent slave handle, and
places the blocking master reader and child wait on dedicated blocking workers.
The coordinator retains the master resize handle, input writer, child/process
group control, and cancellation state.

The SSH transport asks the existing verified connection owner for a new session
channel, requests a PTY with the current terminal dimensions and a conservative
terminal type, then requests the account's default shell. It does not start an
SFTP replacement, parse shell output, execute a synthesized `cd`, or expose the
raw SSH handle to the frontend.

An SSH disconnect publishes one terminal exit event for each affected session
and closes their input paths. Reconnection of file browsing does not revive old
shells. The user must explicitly create a new session.

### Shell selection and environment

The baseline exposes no arbitrary executable, argument, or environment fields to
the WebView. A Rust platform adapter selects the default interactive shell using
documented operating-system conventions and a safe fallback. It launches with an
argument vector and a separately assigned working directory.

The child receives the normal user environment required for an interactive shell
plus deliberate terminal variables such as `TERM`, `COLORTERM`, and a product
identifier. Explora-specific secrets, SSH prompt responses, private-key material,
and internal control variables must never enter the child environment. The
environment policy needs adversarial tests and platform-specific review.

Launch profiles, custom shell arguments, environment overrides, login-shell
behavior, WSL distributions, and container entry points are later capabilities.
If added, profiles are typed Rust-owned configuration—not arbitrary command
strings passed across IPC.

## IPC and stream protocol

### Commands

The narrow command surface is:

```text
create_terminal(request_id, location_id, directory_id?, size, on_event)
write_terminal(session_id, input_sequence, bytes)
resize_terminal(session_id, size)
acknowledge_terminal_output(session_id, output_sequence)
close_terminal(session_id, reason)
```

Creation validates request IDs, opaque references, location capability, initial
dimensions, per-window session limits, and backend availability before allocating
a process or SSH channel. `directory_id` is required for local sessions and is
resolved only inside the authorized local registry. The remote baseline ignores
display paths and uses the account's server-selected default directory.

Input is accepted only for a running session owned by the calling window. The
frontend batches xterm `onData` and `onBinary` input for a short interval or until
a bounded chunk is full. Rust validates chunk size and sequence ordering before
writing and flushing it. Explora does not parse commands, but it also does not
provide an API for another feature to inject input without the same explicit
terminal-session authority.

Resize accepts columns, rows, and optional pixel dimensions within centralized
policy bounds. The frontend uses the xterm fit addon and a `ResizeObserver`, then
debounces duplicate or rapidly changing sizes. Rust remains the final validator.

Close is idempotent. Closing a running local terminal closes its PTY input and
allows a bounded graceful interval before terminating the owned process group.
Closing a remote terminal sends EOF and closes its SSH channel. Application exit
uses the same coordinator path for every session and waits only for a bounded
shutdown interval.

### Events

Rust publishes a typed event stream for each creation request:

```ts
type TerminalEvent =
  | { event: "started"; session: TerminalSessionSummary }
  | { event: "output"; sequence: number; bytes: Uint8Array }
  | { event: "exited"; exitCode: number | null; reason: TerminalExitReason }
  | { event: "failed"; error: TerminalError };
```

The actual Tauri representation must be benchmarked with realistic output before
the wire shape is frozen. Prefer binary payload support where available. If a
byte vector would be JSON-expanded, use the smallest application-owned Tauri IPC
representation that preserves CSP and authorization; do not introduce a local
WebSocket server merely for convenience.

Output is raw bytes because UTF-8 characters and terminal escape sequences may
cross read boundaries. The frontend writes `Uint8Array` chunks directly to
xterm.js. Neither side performs lossy, chunk-local string decoding.

### Ordering and backpressure

PTY reads are blocking and terminal programs can produce output much faster than
the WebView can render it. The protocol therefore uses explicit bounded flow
control:

1. A blocking local reader or asynchronous SSH reader produces bounded chunks.
2. The coordinator assigns strictly increasing sequence numbers.
3. Only a bounded number of bytes may be in flight to the frontend.
4. The xterm adapter acknowledges the highest contiguous sequence after
   `Terminal.write` completion callbacks have consumed it.
5. When the in-flight window is full, Rust pauses delivery and ultimately the
   transport read. The PTY or SSH flow-control window then slows the child
   naturally.

Output is never dropped, reordered, or accumulated without a bound. An event
channel closure starts session shutdown. Duplicate or stale acknowledgements are
ignored safely; acknowledgements beyond the last emitted sequence are rejected.

Initial policy targets should live in one Rust policy type and be tuned through
measurement. Reasonable starting points are a small single-digit session limit,
kilobyte-scale output chunks, a low-megabyte in-flight ceiling per session, a
bounded input chunk, and a few thousand scrollback lines. Tests should assert the
bounds rather than scatter numeric literals.

## Frontend design

### xterm adapter

`XtermAdapter` isolates third-party APIs from Svelte state. It owns the `Terminal`
instance, fit addon, DOM mount, theme updates, input subscriptions, write
callbacks, resize observation, focus, and disposal. Components never reach into
xterm's internal buffer or parser.

The baseline loads only the core terminal and fit addon. Do not initially enable
automatic web links, WebGL rendering, clipboard sequences, search, Unicode
providers, image protocols, shell integration, or custom parser hooks. Each addon
adds compatibility, security, memory, or native-WebView behavior that should be
validated independently.

The adapter must dispose all subscriptions, observers, write callbacks, addons,
and the terminal instance when its session tab closes. A disposed surface may not
acknowledge or render later events from a stale session.

### State and data sources

`TerminalState` owns presentation summaries, active session selection, pane
visibility and size, pending confirmations, and per-session adapters. It delegates
all authoritative lifecycle work to a `TerminalDataSource`.

`TauriTerminalDataSource` validates every IPC response and event before it reaches
state. `DemoTerminalDataSource` supplies deterministic browser behavior for UI
development and Playwright tests but must be visibly non-native in developer
documentation. It should emulate ordering, exit, failure, and delayed output; it
must not pretend to be PTY proof.

The application creates `TerminalState` next to `ExplorerState` and passes a
read-only current launch-context callback. This avoids a circular dependency and
keeps terminal lifecycle out of explorer navigation tests.

### Layout integration

The main content column becomes a vertical pane group:

```text
tab strip
toolbar
┌─────────────────────────────┐
│ file view                   │
├─────────────────────────────┤ resizer
│ terminal tabs + xterm       │
└─────────────────────────────┘
status bar
```

The terminal pane is not rendered when no session exists and the pane is hidden.
Showing an existing session restores its previous bounded height. Hiding the pane
does not stop processes. The status bar remains outside the resizer so item count
and active file location stay stable.

At constrained heights, the pane enforces a minimum file-view height and offers a
maximize/restore command rather than shrinking the file view to unusability. Pane
height persistence belongs in the versioned preferences document after the first
vertical slice proves the interaction.

## Security and privacy model

An interactive terminal intentionally allows the user to execute arbitrary
commands. The security goal is not to restrict what the user's chosen shell can
do; it is to ensure only an explicit terminal session has that authority and that
untrusted output cannot drive Explora or escape the renderer.

Required controls include:

- no generic `execute(command)` or arbitrary process-spawn IPC;
- no Tauri shell/filesystem plugin, localhost listener, remote script, CDN asset,
  or terminal-driven CSP expansion;
- no path string accepted as a working directory;
- no shell interpolation when starting local or remote sessions;
- no automatic command injection on navigation;
- no output-driven link activation, clipboard writes, notifications, file open,
  downloads, or new windows;
- no terminal input, output, transcript, history, title, or full working path in
  application logs;
- explicit multiline-paste confirmation with the target host visible;
- bounded sessions, dimensions, chunks, queues, scrollback, and shutdown;
- hostile control-sequence and title rendering tests;
- credentials and SSH prompt responses excluded from child environments; and
- deterministic process-group/channel cleanup on close and application exit.

Terminal-generated titles may update a tab label only after length and control
character sanitization. The original local/remote identity remains visible and
cannot be replaced by terminal output.

If links are added later, detection and activation must be separate. Activation
requires a modifier and explicit user gesture, permits only reviewed schemes, and
routes through a Rust/platform adapter with confirmation where appropriate. OSC
52 clipboard access remains disabled unless a later ADR supplies a compelling
need and threat model.

## Failure and lifecycle behavior

Failure states are structured and recoverable:

- authorization failure leaves the pane and existing sessions intact;
- shell-not-found offers a platform-appropriate explanation, not a raw spawn
  error;
- PTY or ConPTY creation failure marks only the new session failed;
- output channel loss triggers bounded backend cleanup;
- a local child exit preserves scrollback and reports status;
- SSH disconnect marks each affected shell exited without replay or reconnect;
- resize failure reports degradation without discarding output;
- forced termination is visibly distinguished from normal exit; and
- application shutdown never waits indefinitely for a child or remote channel.

Closing a running session is a destructive lifecycle action. A shell sitting at a
prompt can close immediately after user intent; a session with foreground work
should receive a confirmation unless reliable platform process state proves it is
idle. Until idle detection is trustworthy across platforms, prefer a concise
confirmation for explicit close and a single grouped confirmation on application
quit. Forced shutdown after the deadline must be reported during testing even if
the UI can no longer present it.

## Cross-platform requirements

### macOS and Linux

- Verify login/default-shell selection and fallback independently.
- Preserve job control, signals, process groups, window size, UTF-8 input, and
  non-UTF-8 output bytes.
- Closing the PTY must not leave descendants running unintentionally.
- Validate packaged WebView font metrics, IME input, Option/Alt handling, and
  copy/interrupt conventions.

### Windows

- Exercise the `portable-pty` native Windows backend and supported ConPTY
  baseline on packaged Windows builds.
- Treat PowerShell, Windows PowerShell, Command Prompt, and WSL as explicit
  profiles rather than assuming Unix shell semantics.
- Validate resize, Ctrl handling, UTF-16/UTF-8 boundaries, process-tree cleanup,
  antivirus/packaging behavior, and Windows Terminal compatibility expectations.
- Report unsupported operating-system versions or degraded PTY behavior as a
  capability, not as silent fallback.

No platform is complete based only on compilation or mocked tests.

## Implementation strategy

### Phase 0: protocol and dependency spike

Goal: retire the highest-risk assumptions before UI polish.

- Add xterm.js core and fit addon with Bun, and `portable-pty` with Cargo; review
  licenses, maintenance, transitive weight, and packaged artifacts.
- Build a disposable Rust PTY harness that starts a controlled fixture program,
  echoes arbitrary bytes, resizes, exits, and proves process-tree cleanup.
- Benchmark Tauri channel payload shapes and xterm write acknowledgement under
  sustained output in the packaged macOS app.
- Verify the default xterm renderer in WKWebView before considering WebGL.
- Freeze the typed event protocol only after the benchmark.

Exit criteria: real packaged macOS input/output/resize/exit works, byte boundaries
are lossless, the output queue stays bounded, and closing the window leaves no
owned child process.

### Phase 1: finished local single-session slice

Goal: ship one trustworthy local terminal before building multiplexing.

- Implement terminal domain types, policy, coordinator, local transport, and
  structured errors.
- Add session-scoped IPC with opaque directory authorization and output
  acknowledgements.
- Add `TerminalDataSource`, runtime validation, `TerminalState`, `XtermAdapter`,
  pane layout, focus handling, multiline-paste confirmation, and close behavior.
- Add deterministic demo behavior for component and browser tests.
- Start the shell in the active local directory without exposing its path.
- Add unit, component, IPC, and native smoke coverage.

Exit criteria: the local session is keyboard complete, accessible, bounded,
cancellable, and validated in a packaged macOS application. It is not yet called
cross-platform complete.

### Phase 2: local cross-platform hardening

Goal: make the local terminal a supported Explora capability everywhere.

- Validate and fix Linux PTY, process-group, compositor/WebView, font, IME, and
  package behavior.
- Validate and fix Windows ConPTY, default profile, encoding, Ctrl handling,
  resizing, process-tree cleanup, and packaging behavior.
- Add platform fixture processes and CI tests that do not depend on personal
  shell configuration.
- Exercise output floods, full-screen terminal applications, Unicode, malformed
  bytes, rapid resize, sleep/wake, and application shutdown.

Exit criteria: packaged local-terminal smoke matrices pass on macOS, Linux, and
Windows with documented evidence and no silent degraded target.

### Phase 3: multiple sessions and polished pane behavior

Goal: reach the intended daily-driver interaction.

- Add terminal tabs, new/next/previous/restart/close-all commands, sanitized
  titles, and clear location badges.
- Persist pane visibility, bounded height, font preferences, and scrollback policy
  through the versioned preferences boundary.
- Add grouped shutdown confirmation and robust stale-event disposal.
- Measure CPU and memory with the maximum supported session count, hidden panes,
  and sustained background output.

Exit criteria: sessions remain independent, window-scoped lifecycle is clear, and
resource limits hold under concurrency.

### Phase 4: SSH terminal transport

Goal: add remote shells without weakening SFTP or trust boundaries.

- Refactor the SSH connection owner only as needed to vend typed terminal
  channels while keeping raw transport handles private.
- Request a PTY and default shell after existing host verification and
  authentication succeed.
- Start in the server-selected account directory and label the session honestly.
- Map channel exit status, signal, EOF, disconnect, and cancellation into the
  shared lifecycle contract.
- Extend the disposable SSH server with a controlled PTY/shell fixture and cover
  host trust, every supported authentication path, output, resize, exit,
  disconnect, and secret redaction.
- Validate high-latency backpressure and confirm that reconnect never revives or
  replays an interrupted terminal.

Exit criteria: remote terminals behave consistently with local presentation,
retain explicit host identity, and pass packaged native smoke tests without using
shell output for filesystem work.

### Deferred enhancements

The following are not part of the baseline and should not shape the first slice:

- split terminals or a full multiplexer;
- persistent sessions or transcript restoration;
- command history owned by Explora;
- shell integration scripts or command decorations;
- automatic current-directory tracking or navigation synchronization;
- arbitrary executable/argument/environment profiles;
- WSL, container, or subsystem launch profiles;
- automatic links, image protocols, or output-driven clipboard access;
- remote directory injection; and
- terminal sharing, telemetry, or cloud synchronization.

## Validation plan

### Rust

- Unit-test lifecycle transitions, validation, sequence numbers, acknowledgements,
  bounded queues, idempotent close, timeout escalation, and error mapping.
- Use a fake transport for deterministic state-machine tests.
- Use a controlled fixture process for real PTY contract tests; never depend on a
  developer's dotfiles or prompt.
- Assert opaque-reference ownership and reject stale, cross-location, cross-window,
  oversized, and malformed payloads.
- Confirm process groups and SSH channels are gone after every close path.

### Frontend

- Unit-test IPC validation, binary chunk preservation, stale-event rejection,
  batching, acknowledgement, and disposal.
- Component-test pane toggle, resizer, tabs, focus priority, exit/error states,
  theme changes, screen-reader status, and multiline-paste confirmation.
- Keep xterm behind the adapter so most tests do not require canvas internals.
- Browser Playwright tests use the demo source for layout and interaction only and
  must be labeled as non-native evidence.

### Native integration

- Launch a real shell in an authorized directory and verify `pwd` or its platform
  equivalent through a controlled test fixture, not output scraping in product
  code.
- Exercise interactive input, alternate screen applications, Unicode, IME,
  selection/copy, interrupt, resize, exit codes, and forced cleanup.
- Flood output while hiding/showing and resizing the pane; confirm bounded memory,
  ordered bytes, responsive file browsing, and no deadlock.
- Run remote scenarios against the disposable SSH server with authentication,
  trust, latency, disconnect, and reconnect boundaries.
- Package and smoke-test macOS, Linux, and Windows. Browser E2E and a successful
  build do not substitute for native terminal proof.

### Repository gates

Each implementation slice runs the narrowest focused tests first, followed by the
relevant stable command surface:

```sh
bun run format:check
bun run lint
bun run check
bun run test
bun run test:e2e
bun run build
```

Report the operating system, backend, shell/profile, and packaged versus browser
surface for every manual result. Any unrun platform or transport remains an
explicit release risk.

## Completion criteria

Terminal support is complete for the initial stable release only when:

- local and remote sessions use the typed, opaque, session-scoped authority model;
- I/O is lossless, ordered, bounded, and responsive under sustained output;
- resize, focus, copy/interrupt, paste, exit, close, and application shutdown are
  predictable and keyboard complete;
- untrusted output cannot trigger links, clipboard access, HTML, file operations,
  or another host integration;
- local process groups and remote channels are cleaned up on every lifecycle path;
- accessibility behavior has automated coverage and native manual evidence;
- packaged macOS, Linux, and Windows applications pass the real terminal smoke
  matrix; and
- the README and user-facing help are updated to describe only the behavior that
  actually shipped.
