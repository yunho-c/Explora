# Explora

Explora is a calm, modern desktop file explorer for local and SSH locations. The
packaged Tauri application provides read-only local and SSH/SFTP navigation
through Rust-owned backends. It opens at Home, streams directory listings,
supports folder, breadcrumb, Up, Back, Forward, and tab navigation, and provides
bounded Quick Preview for local text, source, common raster-image, and PDF files.
Saved SSH targets and concrete aliases from
`~/.ssh/config` appear alongside local favorites. The browser-only Vite
application retains deterministic local and remote demo assets for UI development
and tests.

Work is underway on provider-neutral discovery of operating-system-managed
synced folders such as iCloud Drive, OneDrive, and Google Drive. Explora will
browse these through bounded filesystem adapters while the operating system or
installed provider retains authentication and synchronization ownership. See
[`docs/synced-folders.md`](docs/synced-folders.md) and
[`docs/adr/0009-os-managed-synced-folders.md`](docs/adr/0009-os-managed-synced-folders.md).

Current synced-folder support is intentionally phased:

| Target       | Root discovery                                                  | Browsing                                  | Content availability and hydration                                                                         |
| ------------ | --------------------------------------------------------------- | ----------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| macOS        | Initial iCloud Drive and `~/Library/CloudStorage` discovery     | Read-only through opaque local references | iCloud metadata and explicit Download to Preview; real online-only validation pending; third-party unknown |
| Windows      | Implemented through registered Storage Provider sync roots      | Read-only through opaque local references | Cloud Files metadata and explicit Download to Preview; native provider validation pending                  |
| Linux        | Explicit local folders plus GNOME/GVfs Google Drive mounts      | Read-only local and GIO transports        | Local mirrors and bounded GIO preview streams; native validation deferred pending a representative host    |
| Browser demo | Deterministic iCloud Drive, OneDrive, and Google Drive fixtures | Implemented for UI development and tests  | Synthetic explicit-download lifecycle and online-only states                                               |

Packaged macOS, Windows, and Linux provider validation, automatic discovery of
ordinary Linux sync-client folders, third-party File Provider availability on
macOS, and real-provider hydration validation remain tracked in the
synced-folders document.

The packaged application integrates its tab strip into a guarded custom
titlebar. macOS keeps native traffic lights, Windows 11 retains the native Snap
Layout menu, and supported GNOME/KDE Wayland sessions receive GTK-themed window
controls. Activation failures and unsupported Linux sessions automatically
restore the native operating-system titlebar; browser-only development does not
activate desktop window chrome. See
[`docs/adr/0006-integrated-window-chrome.md`](docs/adr/0006-integrated-window-chrome.md)
for the dependency, security, and fallback decision.

## Technology

- Tauri 2 and stable Rust for the desktop application boundary
- Svelte 5, TypeScript, and Vite for the frontend
- Bun for dependencies and repository-level commands
- Tailwind CSS 4 and stock shadcn-svelte Vega components

The generated components in `src/lib/components/ui` are intentionally kept at
their upstream shadcn-svelte styling. Application code composes those primitives;
it does not maintain a custom component theme.

## Structure

```text
src/
├── app/                 application entry and reactive explorer state
├── features/explorer/   file-explorer shell and feature components
└── lib/
    ├── components/ui/   generated shadcn-svelte components
    ├── contracts/       frontend domain summaries and state types
    ├── data/            replaceable data-source boundary and demo adapter
    └── hooks/           generated component hooks

src-tauri/               Tauri configuration, typed IPC, local filesystem, and SSH/SFTP
tests/                   browser-level shell smoke tests
docs/adr/                consequential architecture and security decisions
```

Local paths remain in Rust and cross IPC as opaque, session-scoped references;
remote paths use the same opaque-reference rule, and display paths are never
authoritative. See
[`docs/adr/0001-opaque-local-path-references.md`](docs/adr/0001-opaque-local-path-references.md)
and [`docs/adr/0002-read-only-ssh-sftp-locations.md`](docs/adr/0002-read-only-ssh-sftp-locations.md)
for the authorization, trust, and credential-handling model.

The current filesystem backends are deliberately read-only. SSH authentication
supports agents, standard or configured identity files, encrypted-key
passphrases, passwords, and keyboard-interactive prompts. Explora uses standard
`known_hosts` files, requires confirmation for unknown keys, and blocks changed
keys. `ProxyJump` and `ProxyCommand` are reported as unsupported and are never
silently executed. Bounded keepalives detect dropped sessions, offline tabs retain
their current folder and history, and an explicit reconnect resumes the same
opaque directory reference when it is still valid. Refresh reloads the active
folder without changing navigation history. Mounted local volumes are discovered
in the Rust boundary and updated through native platform notifications with a
bounded polling fallback. Sidebar layout, favorites, view mode, sorting, and SSH
target visibility persist as versioned local preferences. File watching,
hidden-file controls, mutations, remote content previews, and additional preview
formats remain later vertical slices. See
[`docs/adr/0007-versioned-user-preferences.md`](docs/adr/0007-versioned-user-preferences.md)
and
[`docs/adr/0008-cross-platform-volume-discovery.md`](docs/adr/0008-cross-platform-volume-discovery.md)
for the persistence and volume-lifecycle decisions.

Local preview reads are authorized by opaque entry references and performed in
bounded Rust workers. Text previews are capped and decoded without rendering
markup. Static JPEG, PNG, WebP, and BMP images default to dimension- and
size-validated original bytes rendered by the system WebView through one-shot
binary resources. A per-session shield control explicitly enables sanitized,
resized PNG thumbnails; formats that may animate or lack consistent WebView
support require that mode. Local PDFs pass through the same one-shot resource
boundary to a custom, canvas-only PDF.js viewer with continuous pages, bounded
rendering, responsive thumbnails, and no interactive document layer. SVG,
audio, video, and SSH content remain metadata-only. See
[`docs/adr/0003-bounded-local-preview-pipeline.md`](docs/adr/0003-bounded-local-preview-pipeline.md)
and
[`docs/adr/0004-direct-webview-image-preview.md`](docs/adr/0004-direct-webview-image-preview.md)
and
[`docs/adr/0005-bounded-pdf-preview.md`](docs/adr/0005-bounded-pdf-preview.md)
for the limits, resource lifecycle, and security tradeoff.

Rust integration tests start disposable loopback SSH/SFTP servers and cover host
trust, supported authentication methods, secret-safe prompts and errors,
permission and symlink behavior, missing SFTP, delayed-request cancellation,
disconnect detection, and reconnect continuity. The disposable SSH-agent test is
Unix-only; the Windows Pageant and named-pipe paths remain platform-gated code
that require Windows validation.

## Prerequisites

- [Bun](https://bun.sh/)
- [Rust](https://www.rust-lang.org/tools/install) through `rustup`
- The [Tauri system prerequisites](https://v2.tauri.app/start/prerequisites/)

The checked-in `rust-toolchain.toml` selects stable Rust with `rustfmt` and
`clippy`.

## Commands

```sh
bun install          # install locked dependencies
bun run dev          # launch the full Tauri application
bun run dev:web      # launch only the browser-rendered shell
bun run format       # format owned frontend and Rust code
bun run format:check # check formatting without rewriting
bun run lint         # run ESLint and Clippy
bun run check        # run Svelte/TypeScript and Rust checks
bun run test         # run Vitest and Rust tests
bun run test:e2e     # run browser-shell Playwright tests
bun run build        # create a Tauri release package
bun run build:web    # build only the static frontend
```

Install the Playwright browser once with `bun run test:e2e:install`.
The browser suite starts an isolated Vite server on port 6750 so it cannot reuse
an unrelated development worktree; set `EXPLORA_E2E_PORT` to another valid port
when needed.

The Playwright suite validates the browser-rendered shell. It is not packaged
Tauri end-to-end proof; native menus, window behavior, and IPC require separate
desktop smoke tests as those capabilities are introduced.

GitHub Actions is configured to run the locked format, lint, check, and unit-test
command surface on macOS, Windows, and Ubuntu. This compiles each target-gated
Rust adapter, but hosted runners do not have authenticated provider accounts and
therefore cannot run the ignored real-provider smokes or replace packaged native
validation. The first remote run remains required validation for the workflow.
