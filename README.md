# Explora

Explora is a calm, modern desktop file explorer for local and SSH locations. The
packaged Tauri application provides read-only local and SSH/SFTP navigation
through Rust-owned backends. It opens at Home, streams directory listings,
supports folder, breadcrumb, Up, Back, Forward, and tab navigation, and provides
metadata-only Quick Preview. Saved SSH targets and concrete aliases from
`~/.ssh/config` appear alongside local favorites. The browser-only Vite
application retains deterministic local and remote demo assets for UI development
and tests.

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
silently executed. File watching, mounted-volume discovery, hidden-file controls,
mutations, and content previews remain later vertical slices.

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

The Playwright suite validates the browser-rendered shell. It is not packaged
Tauri end-to-end proof; native menus, window behavior, and IPC require separate
desktop smoke tests as those capabilities are introduced.
