# ADR 0003: Bound and isolate local content previews

- Status: Superseded in part by ADR 0004
- Date: 2026-07-20

## Context

ADR 0004 changes the default raster-image policy to bounded original-byte
rendering in the system WebView and retains this sanitized pipeline as an
explicitly enabled mode. The text pipeline and resource-lifecycle decisions in
this ADR remain current.

Quick Preview must show useful file content without giving the webview general
filesystem access or asking it to decode arbitrarily large, malformed source
files. The first content-backed slice covers local plain text, source files, and
common raster images while preserving the same typed result shape for later
SFTP range reads.

## Decision

Rust resolves opaque entry references and prepares every content preview. The
webview sends an entry ID, location ID, and bounded request ID; it never sends an
authoritative path. Symlinks are not followed. Directories, unsupported formats,
remote files, and files rejected by a safety limit return a typed metadata state
instead of raw content.

Text preparation reads at most 256 KiB. It supports UTF-8, BOM-marked UTF-16,
and a Windows-1252 fallback after a binary-data heuristic. Unsafe control
characters are replaced, and Svelte renders the result as textarea value text,
never HTML. Larger files show a clear truncated state.

Raster preparation supports PNG, JPEG, the first frame of GIF, WebP, BMP, and
TIFF. Input files are limited to 64 MiB, either dimension to 16,384 pixels, total
pixels to 40 million, and decoder allocations to 192 MiB. Successful images are
orientation-corrected where metadata is available, resized to fit 1920 by 1920,
and re-encoded as a PNG no larger than 16 MiB. SVG is deferred until it has a
separately reviewed sanitization and isolation strategy.

At most two preview workers run concurrently, and the coordinator returns a
timed-out metadata state after five seconds. Cancellation is checked before and
after reads, decoding, resizing, and encoding. In-process blocking decoders
cannot be forcibly terminated mid-call, so timed-out work may finish in the
background while still occupying one of the bounded worker slots; its result is
discarded.

Text and metadata results cross IPC as validated structured data. Prepared image
bytes live behind random, one-shot resource IDs and use Tauri's raw binary IPC
response. The resource store holds at most four items and 64 MiB, evicts oldest
items when full, and expires items after five minutes. The frontend consumes a
resource immediately, creates a Blob URL, and revokes it on replacement, close,
error, or stale-result rejection. Explora does not enable Tauri's asset protocol
or disclose a local path.

## Security review

- All command identifiers are length-bounded and local entries must resolve in
  the Rust-owned opaque registry for a known location.
- File extensions select candidate behavior, but raster format recognition uses
  file bytes and successful content is always re-encoded.
- Active markup, external URLs, scripts, macros, original image bytes, and
  arbitrary filesystem URLs never reach the preview surface.
- Malformed decoder details are not returned to the webview. Permission, missing
  file, cancellation, and stale-reference failures keep their structured error
  categories.
- Resource IDs are unguessable, single-use, memory-bounded, and short-lived.

## Consequences

Very large or complex files may show metadata even when another desktop viewer
could open them. Animated images display a static first frame. PDF, SVG,
audio/video, syntax highlighting, persistent caching, and remote content reads
remain separate slices. The future SFTP implementation can feed bounded bytes
into the same preparation pipeline and return the same IPC DTOs without changing
the Quick Preview UI.
