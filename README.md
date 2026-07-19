# Explora

Explora is a calm, modern desktop file explorer for local and SSH locations. The
packaged Tauri application now provides read-only local filesystem navigation
through a Rust-owned backend. It opens at Home, streams directory listings,
supports folder, breadcrumb, Up, Back, Forward, and tab navigation, and provides
metadata-only Quick Preview. The browser-only Vite application retains a
deterministic demo backend for UI development and tests.

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

src-tauri/               Tauri configuration, typed IPC, and local filesystem backend
tests/                   browser-level shell smoke tests
docs/adr/                consequential architecture and security decisions
```

Local paths remain in Rust and cross IPC as opaque, session-scoped references;
display paths are never authoritative. See
[`docs/adr/0001-opaque-local-path-references.md`](docs/adr/0001-opaque-local-path-references.md)
for the authorization and security model.

The current filesystem backend is deliberately read-only. File watching, mounted
volume discovery, hidden-file controls, mutations, content previews, and SSH/SFTP
remain later vertical slices.

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
