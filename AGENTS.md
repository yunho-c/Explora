# AGENTS.md

## Scope and authority

This file applies to the entire repository. A nested `AGENTS.md` may add rules for
its subtree, but it must not weaken the product, safety, security, accessibility,
or validation requirements here.

Explora is at the beginning of its development. Treat this document as the
durable product and engineering charter, not as proof that a feature, command, or
directory already exists. The checked-in code, manifests, lockfiles, tests, and
ADRs describe the current implementation. Keep this file accurate when those
facts change.

## Product north star

Explora is a simple, fast, modern desktop file explorer inspired by the clarity
of macOS Finder and GNOME Nautilus. It is not a visual clone of either. It should
feel like one coherent product on macOS, Linux, and Windows while adapting menus,
shortcuts, window behavior, and operating-system integrations to each platform.

The first stable release is a focused daily driver. It should make these jobs
excellent:

- Browse local folders, mounted volumes, and saved SSH locations.
- Navigate with breadcrumbs, back/forward/up history, tabs, favorites, and the
  keyboard.
- View useful list and icon layouts, sort entries, show hidden files on demand,
  and search within a location.
- Create folders and copy, move, rename, drag, drop, trash, or delete files with
  clear progress and conflict handling.
- Preview common files quickly, without opening another application.
- Move naturally between local and remote locations without learning a separate
  remote-file-manager workflow.
- Recover cleanly from permission failures, disconnects, stale state, cancelled
  work, and partial transfers.

“Minimal” means a deliberately small, polished feature set and restrained UI. It
does not mean omitting safety, accessibility, error states, progress, tests, or
cross-platform behavior.

### Product principles

1. **Local and remote are locations, not separate products.** Share navigation,
   selection, previews, operations, and visual language. Reveal backend
   differences only when they affect capability, latency, safety, or trust.
2. **Fast feedback beats hidden work.** Show loading, transfer progress,
   cancellation, conflicts, and failures. Never freeze the window while doing
   filesystem or network I/O.
3. **Safe by default.** Prefer reversible operations, never overwrite silently,
   and make permanent actions unmistakable.
4. **Keyboard complete, pointer friendly.** Every core workflow must work with
   the keyboard; drag and drop and context menus complement rather than replace
   explicit commands.
5. **Privacy is the default.** File names, contents, paths, host names, and usage
   stay on the device unless the user explicitly connects to a remote service.
   Do not add telemetry or content-uploading features without an explicit product
   decision.
6. **Native where it matters, consistent where it helps.** Use platform
   conventions for shortcuts and system integrations while keeping a recognizable
   Explora layout and interaction model.

### Stable-release boundaries

The initial stable release includes polished navigation, tabs, favorites, core
file operations, search, previews, SSH/SFTP locations, transfer progress, and
platform packaging.

The following are out of scope unless a later decision explicitly adds them:

- Tags, comments, saved smart folders, and Finder-class metadata workflows.
- Cloud-provider-specific integrations or file synchronization.
- A terminal emulator, remote shell, text editor, or general-purpose SSH client.
- Third-party plugins or preview-provider APIs.
- Archive creation/extraction beyond what is deliberately added and tested.
- Full disk indexing, content indexing, or a background search daemon.
- Exact visual parity with Finder or Nautilus.

Avoid speculative abstractions for these deferred features.

## Technology baseline

Use the following baseline unless an accepted ADR changes it:

- Tauri 2 for the desktop application boundary and packaging.
- Stable Rust for filesystem, SSH, transfer, trust, and preview orchestration.
- Svelte 5 with TypeScript and Vite for the UI.
- Bun for JavaScript dependency management and repository-level scripts.
- The platform's native facilities, reached through small Rust adapters, when
  behavior genuinely differs by operating system.

Pin dependencies in manifests and the Bun lockfile. Do not put exact dependency
versions in this charter. Do not introduce a second JavaScript package manager or
commit npm, pnpm, or Yarn lockfiles.

This is a desktop application. Do not add server-side rendering, a web service,
or a cloud backend merely because the frontend stack can support one.

## Architecture

Keep privileged and backend-specific work behind a narrow Rust boundary. The
Svelte frontend presents state and user intent; it must not receive unrestricted
filesystem access, construct shell commands, or contain SSH implementation logic.

Organize the code by these responsibilities, even if the exact directory names
evolve:

- **Domain model:** locations, entries, paths, capabilities, operations, errors,
  conflicts, transfer progress, and preview results.
- **Backends:** local filesystem and SSH/SFTP implementations of the same core
  filesystem contract.
- **Coordinators:** navigation/listing, file operations, search, transfers,
  connection lifecycle, and previews.
- **Platform adapters:** trash, volumes, file watching, native menus, keychain
  integration if later approved, and other OS-specific behavior.
- **Tauri IPC:** a small set of typed, validated commands and events that translate
  between frontend DTOs and Rust domain types.
- **Frontend features:** reusable presentation components plus feature-scoped
  stores/actions for navigation, selection, tabs, operations, connections, and
  previews.

### Core contracts

The implementation must preserve these concepts. Names may vary, but their
boundaries must remain explicit:

- A **location reference** identifies a local root or a saved/active SSH host and
  path. It contains no password, private-key contents, or other secret.
- An **entry reference** combines its location/backend identity with an opaque
  path identity. Display strings are for presentation; do not round-trip them as
  authoritative paths. Preserve non-UTF-8 local path data where the OS permits it.
- A **filesystem backend** provides listing, metadata, streaming reads/writes,
  and supported mutations. Local and SFTP implementations obey shared behavioral
  contract tests.
- A **capability set** describes operations such as trash, permanent delete,
  rename, atomic replace, symlinks, watching, seeking, permissions, and remote
  search. The UI enables actions from capabilities, never from backend-name checks.
- A **file operation** has an ID, lifecycle state, structured progress, a
  cancellation path, and a structured result/error. Long work must not be modeled
  as a single blocking IPC response.
- A **preview result** is bounded and typed: safe render data, a controlled stream
  or temporary resource, or metadata/unsupported status. It is never arbitrary
  executable markup.
- Errors crossing IPC are structured and actionable. Preserve categories such as
  not found, permission denied, conflict, offline, host-key failure, unsupported,
  cancelled, and unexpected; do not reduce them to ad hoc strings.

Use task IDs or opaque handles across IPC instead of exposing Rust objects or
backend internals. Validate every command payload in Rust, including paths,
operation options, sizes, and stale identifiers.

### Concurrency and state

- Never perform filesystem, network, hashing, metadata extraction, or preview
  decoding on the UI thread.
- Make directory listing incremental or paged and virtualize large views. A folder
  with tens of thousands of entries must remain interactive.
- Cancel stale listings, searches, metadata requests, and previews when the user
  navigates away or changes selection.
- Bound concurrent metadata, preview, search, and transfer work. Do not start one
  task per entry without backpressure.
- Treat filesystem state as mutable and racy. Revalidate destructive operations
  at execution time and report changed/missing sources clearly.
- Keep tab navigation and selection state in the frontend; keep authoritative
  operation, connection, and trust state in Rust.

## Filesystem behavior and safety

All operations must have defined behavior for files, directories, symlinks,
permission errors, name conflicts, case sensitivity, unavailable destinations,
and cancellation.

- Use native trash for local items where the platform supports it. If trash is
  unavailable, explain that the action is permanent and require confirmation.
- Treat remote deletion as permanent by default and require explicit confirmation
  with the remote host and target visible.
- Never overwrite a destination silently. Present replace, keep both/rename,
  skip, and cancel only where each option is implementable and tested.
- Prefer atomic rename/replace within one filesystem when supported. For
  cross-backend moves, copy and verify first, then delete the source; a failed or
  cancelled copy must not destroy the source.
- Write downloads/uploads to an explicit partial target and finalize only after a
  successful transfer. Clean up owned partial files when safe, and identify any
  leftovers that require user action.
- Do not follow symlinks implicitly during recursive copy, delete, size, search,
  or preview. Prevent cycles and escaping an operation's intended root.
- Do not assume case sensitivity, path separators, Unicode normalization,
  timestamp precision, permission models, or filename validity are identical
  across backends.
- Keep operation progress honest. Unknown totals must use an indeterminate state;
  “complete” means all required finalization succeeded.

## SSH remotes

SSH locations are first-class, with SFTP as the file transport. If a server does
not provide a compatible subsystem, show a clear unsupported-server error; do not
fall back to parsing shell output.

### OpenSSH compatibility

- Discover and honor the user's OpenSSH configuration and host aliases through a
  dedicated resolver. The compatibility baseline includes common `Host`,
  `HostName`, `User`, `Port`, `IdentityFile`, `IdentitiesOnly`, and `ProxyJump`
  workflows when the chosen transport supports them.
- Support SSH agents and passphrase-protected keys. Prompt for passwords or
  passphrases only when required and keep them in memory for no longer than the
  active connection attempt/session policy.
- Use standard `known_hosts` semantics. Unknown hosts require an explicit
  fingerprint confirmation. A changed host key is a blocking security error, not
  a routine reconnect prompt.
- Never silently execute `ProxyCommand` or arbitrary directives from SSH config.
  Unsupported directives must be reported rather than ignored when they affect
  the selected connection.
- Saved favorites may store non-secret connection metadata such as host alias,
  username, port, and initial directory. Do not invent an application credential
  vault. Any future persistent-secret feature requires an ADR, OS keychain use,
  and a threat-model review.
- Redact passwords, passphrases, private-key material, sensitive query data, and
  file contents from logs and errors. Avoid logging full remote paths by default.

Connection establishment, authentication, host-key prompts, reconnects, and
disconnects must have explicit UI states. A disconnect must not discard tabs or
queued intent silently. Automatic retries must be bounded and must never repeat a
destructive operation whose outcome is uncertain.

Remote browsing should tolerate latency: stream listings, cache only appropriate
metadata, invalidate visibly, and offer manual refresh. Remote search may use a
cancellable, bounded client-side traversal; do not run remote shell commands just
to make search faster.

## Preview system

Quick Look-style preview is a core workflow. Space opens a focused preview for the
selection, arrow navigation changes the preview without closing it, and Escape
closes it. The same underlying preview pipeline may also feed an inspector pane.

The first stable release supports:

- Common raster images and carefully sanitized/isolated SVG.
- Plain text and source files with bounded decoding and a safe fallback encoding.
- PDF in an isolated, non-scriptable viewer.
- Common audio and video formats supported by the packaged platform/runtime.
- Basic metadata for all entries and a useful unsupported-format state.

Previewing untrusted files must not execute scripts, macros, embedded applications,
external URLs, or active document content. Apply explicit byte, pixel, duration,
memory, and time limits. Protect against decompression bombs and malformed files.

For remote files, prefer bounded streaming or range reads. If a local temporary
file is necessary, give it an owned lifecycle, non-executable permissions, a size
limit, cancellation, and reliable cleanup. Cache entries must be bounded and
invalidated by stable metadata; never confuse a stale preview with current file
contents.

## User experience and visual language

Aim for a quiet, content-first interface: clear hierarchy, restrained color,
compact but comfortable density, subtle depth, and motion that explains state.
Avoid gratuitous gradients, glass effects, dashboards made of cards, oversized
marketing typography, and decorative animation.

The primary shell should have a location/favorites sidebar, navigation toolbar and
path/breadcrumb surface, tab strip, main file view, and optional inspector/preview
surface. Preserve file-view space at ordinary laptop sizes. Use shared design
tokens for spacing, typography, radii, colors, focus, selection, and motion rather
than one-off component values.

- Distinguish hover, focus, selection, inactive selection, drop target, loading,
  disabled, and destructive states.
- Use the system font stack and platform-appropriate iconography. Icons require
  accessible names or adjacent text; never rely on color alone.
- Follow platform conventions for primary modifier keys, menus, context menus,
  file-name editing, double-click behavior, and window controls.
- Preserve user context across routine errors and reconnects. Do not replace a
  populated view with a blank error page when an inline or banner state suffices.
- Keep dialogs for decisions that truly block progress. Use non-modal progress for
  transfers and ordinary operations.
- Respect reduced motion, high contrast, text scaling, and OS theme. Maintain
  visible keyboard focus and WCAG AA contrast for essential text and controls.

All core actions must be reachable without a mouse. Test tab order, tree/grid
semantics, multiselect, range selection, rename, drag alternatives, preview, and
context actions with assistive technology in mind.

## Cross-platform rules

macOS, Linux, and Windows are first-class targets from the start.

- Keep platform differences behind typed adapters or narrowly scoped UI branches.
  Do not scatter OS-name checks through domain or feature code.
- Do not make POSIX-only assumptions in shared local-filesystem code. Test drive
  letters and UNC paths on Windows, volumes and normalization on macOS, and common
  desktop/trash variations on Linux.
- Use platform-native trash, reveal-in-file-manager, open-with, menus, shortcuts,
  and volume discovery when available.
- A feature is not complete if it silently degrades on another target. Either
  implement it, expose the missing capability honestly, or document an approved
  release exception.

## Security requirements

- Keep the Tauri command surface and capabilities/permissions allowlist minimal.
  Do not expose a generic read-path, write-path, or shell-command escape hatch to
  the webview.
- Configure a restrictive content security policy. Do not load preview content or
  application code from arbitrary remote origins.
- Treat file names, paths, metadata, preview content, SSH banners, and remote error
  messages as untrusted input. Escape them at every rendering boundary.
- Never interpolate paths or user input into a shell command. Prefer library APIs;
  if a platform command is unavoidable, pass arguments without a shell and cover
  it with adversarial tests.
- Use least-privilege file permissions for application state, connection metadata,
  caches, and temporary files.
- Add dependencies deliberately. Check maintenance status, platform support,
  license, transitive weight, and security posture; avoid packages for trivial
  helpers.

Any change to credential persistence, host verification, path authorization,
preview isolation, IPC exposure, or destructive-operation semantics requires a
focused security review and an ADR.

## Code and change discipline

- Read the relevant code, tests, manifests, and ADRs before editing. Search before
  inventing a parallel abstraction.
- Keep changes narrow and coherent. Do not reformat unrelated files or overwrite
  user-owned worktree changes.
- Prefer clear domain vocabulary over generic “manager”, “helper”, or “utils”
  modules. Keep UI components small enough that loading, empty, error, and success
  states remain understandable.
- In TypeScript, keep strict typing and validate data received across IPC. Avoid
  `any`, unchecked casts, and stringly typed operation states.
- In Rust, return structured errors, avoid `unwrap`/`expect` in user-triggerable
  paths, and document the safety invariants of concurrency and platform-specific
  code.
- Do not edit generated artifacts or lockfiles by hand. Use the owning tool and
  commit generated changes only when they are required and reviewable.
- Add comments for invariants, security decisions, and non-obvious platform
  behavior, not for syntax that the code already states.
- Record consequential, hard-to-reverse choices in `docs/adr/`. Keep product and
  architecture documentation current in the same change that changes behavior.

## Commands and repository state

At the time this charter was written, the repository contains no application
scaffold or package manifest. Therefore no build, lint, test, or development
command is currently verified. Do not claim otherwise.

The initial scaffold should provide this stable repository-level command surface
through Bun scripts, delegating to Rust or other tools as needed:

| Command | Required purpose |
| --- | --- |
| `bun install` | Install the locked JavaScript dependencies. |
| `bun run dev` | Launch the complete Tauri application in development mode. |
| `bun run format` | Format owned Rust, Svelte, TypeScript, and config files. |
| `bun run format:check` | Check formatting without rewriting files. |
| `bun run lint` | Run non-formatting linters. |
| `bun run check` | Run TypeScript/Svelte checks and Rust static checks. |
| `bun run test` | Run the fast frontend and Rust unit/contract suites. |
| `bun run test:e2e` | Run packaged or driver-backed end-to-end workflows. |
| `bun run build` | Produce a release build/package for the current platform. |

Once scaffolding exists, replace this paragraph with exact prerequisites and any
necessary platform-specific commands. Keep the command names above stable where
possible so local development and CI share one interface. Never run a mutating
formatter when the task only calls for inspection or validation.

## Testing strategy

Test behavior at the lowest useful layer and retain regression coverage for every
bug fix.

### Required layers

- **Rust unit tests:** path identities, capability logic, conflict decisions,
  cancellation, transfer state machines, SSH config resolution, host-key decisions,
  and preview limits.
- **Backend contract tests:** run the same listing, metadata, streaming, mutation,
  symlink, conflict, and error expectations against a temporary local tree and a
  disposable SSH/SFTP server.
- **Frontend component tests:** navigation, selection, keyboard behavior, loading,
  empty/error states, operation progress, dialogs, and preview interactions.
- **Integration tests:** typed IPC, incremental events, cancellation, stale-result
  rejection, reconnect behavior, and cross-backend transfers.
- **End-to-end tests:** real user workflows in a built application, including at
  least representative coverage on macOS, Linux, and Windows.
- **Accessibility checks:** automated checks plus keyboard and screen-reader-aware
  manual scenarios for the primary shell and blocking dialogs.

SSH tests must cover agent/key authentication, passphrase/password prompts without
secret leakage, unknown and changed host keys, aliases, reconnects, latency,
permission failures, symlinks, interrupted transfers, and permanent deletion.

Preview tests must cover each supported category plus empty, oversized, malformed,
unsupported, renamed, changed, remote, slow, and cancelled files. Assert temporary
resource and cache cleanup.

Do not overstate evidence. A type check, mocked UI test, or development smoke test
is not end-to-end proof. Report exactly what ran, on which platform/backend, and
what remains unverified.

## Definition of done

A change is done only when:

- The intended behavior and important failure states are implemented.
- Security, capability, accessibility, cancellation, and cross-platform effects
  have been considered explicitly.
- Relevant tests were added or updated and the narrowest useful checks pass.
- Formatting, linting, type/Rust checks, and broader tests were run in proportion
  to the risk; any unrun validation is called out.
- User-facing behavior, commands, architecture documents, and this charter are
  updated when necessary.
- No secrets, local paths, debug output, temporary files, or unrelated changes are
  included.

For the first stable release, “done” additionally means the documented full suite
passes, packaging is exercised on macOS, Linux, and Windows, and real local and
SSH smoke scenarios demonstrate browsing, search, previews, core file operations,
progress/cancellation, conflict handling, disconnect recovery, and safe deletion.

## Working agreement for agents

Before changing code, state the intended slice and inspect the current worktree.
During the work, preserve unrelated edits and keep the user informed when a test
is slow, a platform is unavailable, or a security/product decision is required.
At handoff, lead with the outcome, list the validation actually performed, and
name remaining risks without disguising them as completed work.

Favor a small, finished vertical slice over a broad scaffold full of placeholders.
Every slice should move Explora toward being calm, trustworthy, and pleasant for
both local and remote files.
