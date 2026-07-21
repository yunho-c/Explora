# ADR 0004: Render bounded original image bytes by default

- Status: Accepted
- Date: 2026-07-20

## Context

ADR 0003 required every raster image to be fully decoded, orientation-corrected,
resized, and re-encoded as PNG in Rust before it reached the WebView. That model
provides the strongest content normalization, but it adds visible latency in
development builds and prevents the system WebView from using its optimized
native image path. The product decision is to make original-byte rendering the
default and make sanitized thumbnails an explicitly enabled mode.

The change must preserve opaque filesystem references and must not introduce a
generic file URL, asset-protocol scope, or arbitrary MIME response.

## Decision

Quick Preview has a typed image mode with two values: `direct` and `sanitized`.
Each application session starts in direct mode. An accessible shield toggle in
Quick Preview explicitly selects sanitized mode, and the choice remains active
for later images in that session. It is not persisted as a hidden preference.

Direct mode reads at most 16 MiB from the Rust-authorized opaque entry. Rust
recognizes an allowlisted raster format, reads dimensions and orientation from
the same bounded bytes, and rejects zero-sized images, dimensions above 16,384
pixels, or more than 40 million pixels. Static JPEG, PNG, WebP, and BMP bytes may
cross IPC. APNG and animated WebP are rejected; GIF and TIFF require sanitized
mode because animation and cross-platform WebView behavior cannot be bounded
consistently. SVG remains unsupported.

Accepted bytes use the existing random, one-shot resource IDs and Tauri binary
response. The frontend validates the returned MIME type and mode, creates a Blob
URL, and renders it only in an `img` element under the existing restrictive CSP.
The URL is revoked when the preview changes, closes, fails, or becomes stale. A
five-second render guard replaces a direct image that does not load with a typed
unavailable state. This guard is best effort because a stalled WebView decoder
cannot be forcibly interrupted from JavaScript.

Sanitized mode retains the ADR 0003 pipeline: Rust fully decodes the image in a
bounded worker, applies orientation, resizes it to fit 1920 by 1920, and emits a
PNG no larger than 16 MiB. Its 64 MiB input, decoder allocation, worker,
cancellation, and timeout limits remain unchanged.

## Security review

- Direct mode intentionally gives the platform WebView decoder original,
  potentially malformed raster bytes. This increases decoder attack surface
  relative to sanitized mode and is the principal accepted tradeoff.
- The WebView still receives neither an authoritative path nor filesystem
  permission. Tauri's asset protocol remains disabled, and direct resources are
  memory-bounded, single-use, and short-lived.
- The MIME allowlist excludes SVG, HTML, PDF, and other active or compound
  content. Blob URLs cannot initiate network access under the preview CSP.
- Compressed byte and declared pixel bounds reduce decompression-bomb exposure,
  but they cannot place a hard upper bound on a platform decoder's transient
  allocations or execution time. Sanitized mode remains available when stronger
  normalization is desired.
- Rust inspects and transfers the same byte buffer, avoiding a validation/read
  race within preview preparation. Existing symlink rejection and opaque
  location authorization remain in force.
- Animated sources do not reach direct mode because repeat duration and frame
  allocation cannot be controlled reliably across macOS, Linux, and Windows
  WebViews.

## Consequences

Typical static photographs avoid Rust-side full decoding, resizing, and PNG
encoding, making direct preview substantially faster in development and release
builds. The WebView performs the final decode, so image-rendering behavior and
codec security now partly follow the packaged operating-system WebView.

Images above the direct byte limit, animated formats, and TIFF show an
unavailable state until the user enables sanitized mode. Direct and sanitized
results continue to share the same frontend content contract, cancellation,
stale-result rejection, and one-shot resource lifecycle. Remote preview can use
the same explicit mode once bounded SFTP reads are implemented.
