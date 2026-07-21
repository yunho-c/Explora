# ADR 0005: Render bounded local PDFs in an isolated canvas viewer

- Status: Accepted
- Date: 2026-07-21

## Context

Quick Preview needs useful PDF support without adopting the generic PDF.js viewer,
exposing local paths, or allowing active document features into the application
DOM. Native PDF viewers differ across the system WebViews used on macOS, Linux,
and Windows and do not provide a consistent, controllable interface. Fully
rasterizing an entire document in Rust would add significant latency, memory use,
and platform packaging weight.

## Decision

Rust recognizes a local `.pdf` candidate, rejects symlinks and files over 32 MiB,
requires the original bytes to begin with `%PDF-`, and places accepted bytes in
the existing random, one-shot preview resource store. The per-resource store
ceiling increases to 32 MiB while direct and sanitized raster images retain their
16 MiB output ceiling. The store still holds at most four resources and 64 MiB in
total and expires unused resources after five minutes.

The frontend validates a typed `application/pdf` result and transfers the bytes
directly to a pinned PDF.js worker. Explora uses PDF.js's maintained legacy
distribution and its matching worker because Tauri relies on system WebViews,
which can lag proposal-stage JavaScript collection methods used by the modern
distribution. This remains the same pinned parser version; the legacy build adds
compatibility transforms and polyfills rather than selecting an older PDF.js
release. PDF.js, its worker, CMaps, ICC profile, standard fonts, and WASM decoders
are packaged with Explora and served only from the application origin. The CSP
permits only same-origin workers and assets; no CDN or arbitrary remote origin is
introduced.

Explora owns the viewer UI. It renders pages to canvases in a continuous scroll
surface with a responsive thumbnail rail, current-page navigation, fit-width
zoom from 50 through 200 percent, and no stock PDF.js header. Only visible pages
plus one page of overscan are scheduled. Main pages and thumbnails each have at
most two concurrent render tasks, and work is cancelled when it leaves the
render window, the selected file changes, or Quick Preview closes. Device pixel
ratio is capped at two, rendered canvases and PDF images are capped at 16 million
pixels, documents are capped at 500 pages, document loading at ten seconds, and
individual renders at five seconds.

The canvas renderer disables annotation appearances and does not mount text,
annotation, form, attachment, outline, scripting, embedded-media, or external-link
layers. XFA rendering is disabled. Password-protected documents show a concise
unsupported state rather than collecting a password in this slice.

## Security review

- PDF bytes cross IPC only after an opaque local entry is authorized; the WebView
  receives no path, file URL, generic read primitive, or reusable backend handle.
- PDF.js parses untrusted bytes in its dedicated worker. This deliberately accepts
  PDF.js and its packaged decoders as attack surface, but avoids native viewer
  inconsistency and keeps active document features out of the application DOM.
- File size, page count, image pixels, canvas pixels, concurrency, time, and
  resource lifetime are bounded. PDF.js or a platform canvas implementation may
  still allocate transient internal memory that JavaScript cannot strictly cap.
- Document JavaScript is never requested or executed, and annotation-derived URLs
  are never rendered as interactive elements. The restrictive application CSP
  remains defense in depth rather than the primary parser boundary.
- Prepared resources are consumed once or explicitly discarded on cancellation,
  malformed IPC, stale selection, or failure.

## Consequences

PDF preview is visually consistent and avoids Rust-side re-encoding while keeping
the viewer small and content-first. Canvas pages are intentionally not selectable,
searchable, form-capable, or exposed as a screen-reader document structure.
Accessible control names, focus visibility, and keyboard operation remain, while
advanced document accessibility is deferred until the interaction model is
validated.

This slice supports local PDFs only. SSH PDFs remain metadata-only until the SFTP
backend provides bounded or ranged preview reads with cancellation and owned
temporary-resource cleanup where needed.

Do not replace the legacy PDF.js entry points with the modern distribution based
only on Chromium testing. The modern 6.1.200 build reaches
`Map.prototype.getOrInsertComputed` during full-page rendering, but the system
`WKWebView` on macOS 15.6 does not provide that method. The exact same document
can therefore render thumbnails while both main pages fail. Any future switch
requires native render smoke tests on all three desktop WebView engines.
