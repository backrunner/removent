# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.11.0] - 2026-09-24

### Security

- `CFFileDescriptor::from_raw_fd(fd, true)` let safe code make Core Foundation close a descriptor it didn't own (double close, or closing an unrelated file after reuse). It is replaced by `from_owned_fd(OwnedFd)`, which hands the descriptor to Core Foundation, and `from_borrowed_fd(BorrowedFd)`, which never closes it.

### Fixed

- Process aborts reachable from safe code:
  - `CFURL::file_system_path` trapped in Swift when `CFURLCopyFileSystemPath` returned NULL (for example `file:///tmp/%FF` or `mailto:` URLs).
  - Hashing a `CFType` wrapper trapped whenever `CFHash` exceeded `isize::MAX`, which happens for negative floating-point `CFNumber`s, so putting one in a `HashSet` aborted.
  - `IOSurface::create` overflowed `width * bytes_per_element * height`; the CGImage render and copy exports and the planar `CVPixelBuffer` export had the same kind of trap. Exported names and signatures are unchanged.
  - `DispatchSource::timer(Duration::MAX, ..)` trapped converting the interval; intervals are now passed in nanoseconds, so sub-millisecond intervals no longer collapse to a 1 ns busy timer.
  - GCD aborted when a `DispatchSemaphore` was released below its initial count, when a `DispatchGroup` was released while entered, and on an unbalanced `DispatchGroup::leave`.
  - `CFFileSecurity::set_mode` with a value above `u16::MAX`, and force-unwrapped formatter styles and CF getters in the bridge.
- Panics on ordinary input: interior NUL in `CFString::new`, `CFUUID::parse_str`, `CFError::new`, `CFLocale::new`, `CFBundle::resource_url`, `CFMessagePort::connect_remote` and `DispatchQueue::new`; huge dates in `CFDate::to_system_time`; an unknown format from `CFPropertyList` decoding.
- `CFData::to_vec`, `CFSet::values` and `CFMutableSet::values` pass their buffer capacity to the bridge, which no longer writes a re-read length into a buffer sized by an earlier call.
- `CFNumber::from_u64` stores values above `i64::MAX` as unsigned, and `to_i64`/`to_u64` only succeed when the value is exactly representable (no more `Some(i64::MAX)` for `1e19`, or `Some(u64::MAX)` for `-1`).
- `CFCharacterSet::contains` works for characters outside the Basic Multilingual Plane, and `CFCharacterSet::from_characters_in_string` keeps them on every release: through macOS 26, `CFCharacterSetCreateWithCharactersInString` truncated them to 16 bits when a string under 64 UTF-16 units also held BMP characters (`"a😀"` gave U+F600, `"b𠀀"` gave U+0000). The set is now built one scalar at a time, with unpaired surrogates mapped to U+FFFD as macOS 27 does.
- `CFURL::absolute_string` resolves relative URLs against their base.
- `CFString::to_string_lossy` and `CFType::description` keep embedded NUL and replace unpaired surrogates instead of returning an empty string; `CFString::len` is documented as UTF-16 code units.
- C strings returned by the bridge are allocated with `malloc`, matching the `free` in `acf_free_string`.
- `CMClock::time` returns the clock's time; it used to always return `CMTime::INVALID`.
- Describing a `CFRunLoop` (its `Debug` output, and `CFType::description`, `Debug` and `Display` for an erased run loop) no longer calls `CFCopyDescription`, which walks the loop's mode, source and timer sets without taking the loop's lock and so raced with any thread scheduling on that loop. It prints `<CFRunLoop 0x…>`.
- IOSurface `plane_data` and `plane_row` check that the plane lies inside the surface allocation.
- Docs: the README IOSurface-backed `CVPixelBuffer` example really is IOSurface-backed, the crate no longer claims to be dependency-free, feature flags are documented as gating only the Rust modules, the README lists the URL strings that `CFURL::from_string` rejects on macOS 27 but earlier releases accept, and `COVERAGE*.md` state that most `VERIFIED` rows are raw bindgen declarations, that V2 is a self-selected sample, and that `CFTree.h` is not wrapped.

### Changed

- **BREAKING:** `CFURL::from_string`, `CFURL::from_file_system_path`, `CFTimeZone::new`, `CFCalendar::new`, `CFMessagePort::create_echo_local` and `DispatchSemaphore::new` return `Option<Self>`, and `CFURL::file_system_path` returns `Option<CFString>`.
- **BREAKING:** `CFRunLoop::run_in_default_mode(duration, return_after_source_handled)` is an associated function; it always runs the current thread's run loop.
- **BREAKING:** `CMBlockBuffer::as_slice` and `cursor_ref` are `unsafe`, like the other zero-copy views of framework-owned memory; `cursor` always copies.
- **BREAKING:** `CMTimebase::with_master_clock` and `master_clock` are now `with_source_clock` and `source_clock`, backed by the non-deprecated `CMTimebaseCreateWithSourceClock`/`CMTimebaseCopySourceClock`.
- **BREAKING:** `CMClock::time` is no longer a `const fn`.
- **BREAKING:** the crate-root `CFError` null-pointer error is now `NullPointerError`; `apple_cf::CFError` remains as a deprecated alias, and `apple_cf::cf::CFError` still wraps `CFErrorRef`.
- **BREAKING:** `CFPropertyListError` has a new `UnknownFormat(isize)` variant.
- **BREAKING:** `apple_cf::ffi`: the capacity-less `cf_data_copy_bytes` and `cf_set_get_values` are replaced by `acf_cf_data_copy_bytes` and `acf_cf_set_copy_values`, and the group and semaphore functions are renamed to `acf_dispatch_group_holder_*` and `acf_dispatch_semaphore_holder_*` because their handles are now bridge holders. The bridge exports used directly by dependents (`cgimage_*`, `io_surface_*`, the legacy `iosurface_*`, `cm_*`, `cv_pixel_buffer_*`, `dispatch_queue_retain`/`release`) keep their names and signatures.
- `rust-version` is 1.82 (was 1.76), and `doom-fish-utils` is required at `>=0.4.1, <0.5`.

### Added

- `DispatchQueue::main`, `DispatchQueue::global(qos)`, `DispatchQueue::concurrent(label, qos)` and `dispatch_after(delay, queue, work)`.
- `CFNotificationCenter::add_observer(name, callback)`, returning a `CFNotificationObserver` that unregisters when dropped. Callbacks may run on any thread and a panic in one is contained.
- `CMSampleBuffer::audio_buffer_list`, which makes `cm::AudioBufferList` constructible; its views are read-only and keep the backing block buffer alive.
- `CMSampleBuffer::sample_attachments` and `is_sync_sample` (`kCMSampleAttachmentKey_NotSync`).
- `CMFormatDescription::video_dimensions` and `video_parameter_sets` (H.264 or HEVC parameter sets plus NAL unit header length, as `CMVideoParameterSets`).
- `CFFileDescriptor::from_owned_fd` and `from_borrowed_fd`.
- `CFRunLoop` is `Send` and `Sync`, so another thread's run loop can be passed around and targeted. Its methods (`wake_up`, `stop`, `add_timer`) are thread-safe Core Foundation calls, and running a loop stays an associated function that runs the calling thread's own loop.

### Removed

- `CFFileDescriptor::from_raw_fd`, replaced by `from_owned_fd` and `from_borrowed_fd`.
- `AudioBuffer::data_mut` and `AudioBufferList::get_mut`: the buffers alias Core Media memory that other sample buffers can see. Neither was reachable before, because nothing could construct an `AudioBufferList`.

## [0.10.0] - 2026-09-07

### Changed (breaking)

- **BREAKING:** ownership-adopting `from_raw` constructors are now unsafe across Core Foundation, Core Media, Core Video, and IOSurface wrappers. Borrowed imports use the explicit `from_raw_borrowed` name, and `AsCFType` is now an unsafe trait.
- **BREAKING:** `CVPixelBuffer` and `IOSurface` raw lock/unlock calls and zero-copy slice, row, plane, and cursor views now require unsafe caller guarantees; lock-guard `Deref` implementations were removed. `CGContext` byte views are also unsafe because retained clones share the same mutable context.
- **BREAKING:** `CVPixelBufferLockFlags` now uses the native 64-bit `CVOptionFlags` ABI.
- **BREAKING:** `CVPixelBufferPool::try_create_pixel_buffer` now returns `Result<Option<_>, i32>`, the misleading `is_empty` and ignored `flush_with_options` APIs were removed, and pool attribute getters now return owned `CFDictionary` values.
- `CVPixelBufferPool` now uses direct Core Video calls, applies `max_buffers` as an allocation threshold on every wrapper allocation, shares that policy across clones, honors per-call auxiliary attributes, and exposes native flush flags.
- `CVPixelBufferPool` remains `Send + Sync`; clones share the same immutable allocation policy across threads.
- `CMTime` now matches SDK flag values, zero/indefinite representations, numeric predicates, seconds conversion, and default half-away-from-zero scale conversion.
- Dispatch timer resume/cancel/drop transitions are synchronized and idempotent across retained clones, and fire counts are synchronized.
- `CMSampleBuffer` now exposes an explicit borrowed image-buffer pointer and an owned typed `image_buffer()` getter; the retained bridge export is named `cm_sample_buffer_copy_image_buffer`.
- Raised the `doom-fish-utils` requirement to `>=0.4, <0.5`.
- Kept the explicitly aliased `apple_cf_compat_04` and VideoToolbox 0.10 dev fixtures for the cross-version `03_cm_sample_buffer` smoke test; using VideoToolbox 0.20 would create a dev-dependency cycle back to apple-cf 0.10.

### Fixed

- `CFSet` callback snapshots retain every value before invoking the first callback, allowing reentrant mutation such as clearing a mutable set.

## [0.9.3] - 2026-05-20

- Clippy hygiene sweep: cleared all `-D warnings` lints across the crate. No public API change.

## [0.9.2] - 2026-05-20

- Widen `doom-fish-utils` dependency bound to `<0.4` so the 0.3.x SPSC-ring release resolves cleanly across the fleet. No source changes.

## [0.9.1] - 2026-05-19

- Bump MSRV from 1.70 to 1.76 to match fleet baseline.

## [0.9.0] - 2026-05-18

### Changed

- Added the last three missing CoreGraphics raw aliases for downstream re-export and dedup work: `CGContextRef`, `CGCharCode`, and `CGKeyCode`.

## [0.8.1] - 2026-05-18

### Changed

- Added `Debug` coverage to the remaining public structs that can safely expose it in `apple-cf`: 5 structs touched total, with `CFPropertyList`, `CFPreferences`, and `CFXML` now deriving `Debug`, plus manual pointer-aware `Debug` implementations for `CGColorSpace` and `CGImage`.

## [0.8.0] - 2026-05-18

### Changed

- **BREAKING**: `CGRect` now matches Apple's `CGGeometry.h` definition with nested `origin: CGPoint` and `size: CGSize` fields. The previous flat `{ x, y, width, height }` layout had the same byte ordering, so FFI compatibility is preserved, but field access must change: `rect.x` → `rect.origin.x`, `rect.y` → `rect.origin.y`, `rect.width` → `rect.size.width`, and `rect.height` → `rect.size.height`.
- `CGRect::new(x, y, width, height)` is unchanged and still takes four `f64` values.
- Added `CGRect::from_origin_size(origin, size)`.
- Added `CGRect::is_empty()` and `CGRect::contains_point()`.

## [0.7.2] - 2026-05-18

### Changed

- Added one-line `///` documentation across the raw bindings, bridge FFI surface, and safe wrappers, raising measured public-item coverage to 100.0% (5,904 of 5,904 items documented).
- Clarified ownership expectations for raw-pointer constructors and added `# Safety` sections to public `unsafe fn` items where needed.
- Bumped `Cargo.toml` from `0.7.1` to `0.7.2` for this documentation-only release with no API or ABI changes.

## [0.7.1] — 2026-05-20

### Changed

- Added `// SAFETY:` justification comments to every bare `unsafe impl Send`
  and `unsafe impl Sync` declaration across `iosurface`, `cv`, `cm`, `cg`, and
  `dispatch_queue` modules.  All Apple Core Foundation / Core Media / Core Video
  / GCD opaque-pointer types are documented as thread-safe by Apple; the
  comments now make this contract explicit in source.
- Normalised `Drop` implementations: `IOSurface`, `CVPixelBuffer`,
  `CVPixelBufferPool`, and `CVMetalTextureCache` now null-guard their release
  calls, matching the convention already in place for `CFType` and
  `CMSampleBuffer`.
- Updated `doom-fish-utils` version constraint from `"0.1"` to `">=0.1, <0.3"`
  following the crate-family `>=X.Y, <X.(Y+2)` convention.
- README installation snippet updated to reflect the current `0.7` line.

## [0.7.0] — 2026-05-17

### Changed

- **Factored framework-agnostic helpers into the new
  [`doom-fish-utils`](https://crates.io/crates/doom-fish-utils) crate**:
  `completion`, `ffi_string`, `four_char_code`, and `panic_safe` now
  live in `doom-fish-utils` so any doom-fish family crate can pull them
  in without depending on the full `apple-cf` Core* surface.
- `apple_cf::utils` is preserved as a re-export shim, so downstream
  call sites (`apple_cf::utils::{completion, ffi_string,
  four_char_code, FourCharCode, panic_safe}`) keep compiling without
  any changes. The string-owning helpers
  (`ffi_string_owned`, `ffi_string_owned_or_empty`) remain
  apple-cf-specific shims that bake in `acf_free_string` as the
  deallocator; the underlying generic helpers in `doom-fish-utils`
  take a caller-supplied `free_fn` for sibling crates that need a
  different `_free_string` symbol.
- Added `doom-fish-utils = { version = "0.1" }` dependency.
- `Cargo.toml` version bumped to `0.7.0` (minor: dependency graph
  change, no public API breakage).
- `COVERAGE_AUDIT_V2.md` published — independent re-verification
  against `MacOSX26.2.sdk` confirming 100% non-exempt coverage of
  the sampled top-300 symbols per framework.

### Removed

- `apple_cf/src/utils/{completion.rs, four_char_code.rs,
  panic_safe.rs}` — relocated to `doom-fish-utils`. The
  `apple_cf::utils::*` import paths still resolve via re-exports.

## [0.6.2]

### Added

- New `raw` module with exhaustive low-level CoreFoundation / CoreMedia / CoreVideo / IOSurface / Dispatch bindings generated from the active macOS SDK, plus hand-written coverage for inline helpers like `CFByteOrder*`, `CFString*InlineBuffer`, `CMTag*`, `CMTimebase*` compatibility shims, `dispatch_get_main_queue`, and the remaining CVMetal declarations.
- Smoke example `15_raw_bindings` and matching `raw_bindings_tests` coverage harness for the new exhaustive raw surface.

### Changed

- `COVERAGE_AUDIT.md` now reports `0` remaining gaps and `100.00%` coverable coverage (`95.18%` overall once deprecated / unavailable exemptions are included).
- README / COVERAGE docs refreshed for the new `apple_cf::raw` module.
- Dispatch queue creation bridge renamed to `acf_dispatch_queue_create` to avoid colliding with the system `dispatch_queue_create` symbol now exposed through `apple_cf::raw`.
- `Cargo.toml` version bumped to `0.6.2`.

## [0.6.1]

### Added

- `cf::CFSet` / `CFMutableSet` and `cf::CFPropertyList`, including Swift bridge coverage, examples, and smoke tests.
- `dispatch_queue::dispatch_async`, `dispatch_async_and_wait`, and `dispatch_apply` safe helpers.
- `cm::CMMetadataFormatDescription` plus metadata-description constants, constructors, identifier lookup, and merge/extend helpers.
- New numbered example `14_cm_metadata_format_description` and matching CoreMedia metadata smoke tests.

### Changed

- `COVERAGE_AUDIT.md` refreshed for the highest-value remaining gaps; deprecated `CVDisplayLink` symbols now live in the exempt bucket.
- README / COVERAGE docs refreshed for the new CoreFoundation, Dispatch, and CoreMedia surface.
- `Cargo.toml` version bumped to `0.6.1`.

## [0.6.0]

### Added

- **`cf` module** — safe Core Foundation wrappers for:
  - value types: `CFType`, `CFString`, `CFNumber`, `CFData`, `CFDate`, `CFUUID`, `CFError`
  - collections: `CFArray`, `CFDictionary` / `CFDict`, `CFBag`, `CFTree`, `CFAttributedString`
  - resources / locale / formatting: `CFURL`, `CFBundle`, `CFLocale`, `CFCalendar`, `CFTimeZone`, `CFCharacterSet`, `CFNumberFormatter`, `CFDateFormatter`, `CFPreferences`, `CFFileSecurity`, `CFXML`
  - runtime helpers: `CFNotificationCenter`, `CFRunLoop`, `CFTimer`, `CFMessagePort`, `CFStreamPair`, `CFSocket`, `CFFileDescriptor`
- **Dispatch sync primitives** — `DispatchGroup`, `DispatchSemaphore`, and timer-backed `DispatchSource` in `dispatch_queue`.
- **CoreMedia time extras** — `CMTimeRange`, `CMClock::host_time_clock()`, and `CMTimebase`.
- **CoreVideo extras** — `CVBuffer`, `CVImageBuffer`, `CVMetalTextureCache`.
- Eight new numbered examples (`06_` through `13_`) covering the new CoreFoundation / Dispatch / CoreMedia / CoreVideo surface.
- Seven new test files covering the new wrappers.
- `COVERAGE.md` header-audit summary for the Wave-C sweep.

### Changed

- README refreshed for the expanded CoreFoundation / Dispatch / media coverage.
- `Cargo.toml` version bumped to `0.6.0`.
- `build.rs` now links the `Metal` framework for `CVMetalTextureCache` support.

## [0.5.0]

### Added

- **`cg::CGContext`** — safe Core Graphics bitmap-context wrapper with RGBA8 and grayscale constructors, byte accessors, path/rect drawing, transforms, graphics-state save/restore, image drawing, and bitmap snapshots.
- `CGImage::save_png()` helper backed by the existing ImageIO Swift bridge so bitmap snapshots can be written to disk without extra dependencies.
- Smoke example `05_cgcontext_smoke` proving a 64×64 offscreen `CGContext` can draw shapes, snapshot to `CGImage`, export a PNG, and verify pixel contents.

### Changed

- `cg` module docs now cover both value types and bitmap drawing wrappers.

### Added

- **`cv` module** — `CVPixelBuffer` and `CVPixelBufferPool` carved out of
  `screencapturekit-rs`. Wraps the CoreVideo primitives that pair an
  IOSurface with format metadata.
- `cv` feature flag (on by default; implies `iosurface`).
- `CoreVideoBridge` Swift target with the underlying `cv_pixel_buffer_*`
  and `cv_pixel_buffer_pool_*` `@_cdecl` exports.
- `CVPixelBuffer::create_with_io_surface(&IOSurface)` lets downstream
  consumers (e.g. `vision-rs`) ingest live capture data without a PNG
  round-trip.
- Smoke test `04_cv_pixel_buffer` proves the IOSurface ↔ CVPixelBuffer
  round-trip: write `[0xDE, 0xAD, 0xBE, 0xEF]` via IOSurface, read back
  via the wrapped CVPixelBuffer, verify identical bytes and identical
  IOSurface id on the round-trip.
- API harness extended to CVPixelBuffer + CVPixelBufferPool — 7/7 tests
  pass at 100% coverable.

### Added

- **`cm` module** — CoreMedia value types and reference-counted wrappers
  carved out of `screencapturekit-rs`:
  - `CMTime` / `CMSampleTimingInfo` (pure value types, 0 deps)
  - `CMSampleBuffer` — safe Drop/Clone wrapper with accessors for PTS, DTS,
    duration, num_samples, validity, format description, data buffer, and
    raw image-buffer pointer hand-off. SCStreamFrameInfo attachment readers
    intentionally **not** ported — those stay in screencapturekit-rs.
  - `CMBlockBuffer` — Drop/Clone wrapper with data length, contiguous-range
    check, byte-copy, data pointer access, and create-with-data / create-empty
    constructors.
  - `CMFormatDescription` — Drop/Clone wrapper with media type / subtype /
    extensions, plus audio-specific accessors (sample rate, channel count,
    bits-per-channel, bytes-per-frame, format flags).
  - `audio` — `AudioBuffer` / `AudioBufferList` / `AudioBufferListRaw`
    bridging types ported verbatim.
- **CoreMediaBridge Swift target** with 28 `@_cdecl` exports covering the
  generic CMSampleBuffer / CMBlockBuffer / CMFormatDescription surface.
- **`cm` feature flag** (on by default) so audio-only consumers can opt
  out of the CoreMedia symbols.
- Smoke test `03_cm_sample_buffer` proves end-to-end retain/release across
  the videotoolbox ↔ apple-cf boundary: encodes one H.264 frame, wraps the
  resulting CMSampleBuffer in our safe type, and inspects PTS/data-buffer/
  format-description with real values (`vide` / `avc1`, 142 bytes of H.264).
- API coverage harness extended to CMSampleBuffer / CMBlockBuffer /
  CMFormatDescription — 5/5 tests pass at 100% coverable coverage.

### Changed

- Re-exports from `prelude`: `CMTime`, `CMSampleBuffer`, `CMBlockBuffer`,
  `CMFormatDescription` join the ergonomic prelude (gated on `cm` feature).


### Added

- Initial scaffold carved out of `screencapturekit-rs`.
- `cg` — CoreGraphics value types (`CGRect`, `CGPoint`, `CGSize`).
- `iosurface` — full `IOSurface` API (single- and multi-planar, lock/unlock,
  use-count tracking, properties).
- `dispatch_queue` — `DispatchQueue` + `DispatchQoS`.
- `utils` — `FourCharCode`, `SyncCompletion` / `AsyncCompletion`,
  `ffi_string_owned`, `panic_safe` callback wrapper.
- Swift bridge with separate `CoreGraphicsBridge`, `IOSurfaceBridge`,
  `DispatchBridge` targets aggregated under a single static
  `AppleCFBridge` library.
- `acf_free_string` centralised heap-string deallocator.
- Two smoke-test examples that exercise the full Rust → C FFI → Swift →
  Apple framework path.

### Planned

- `cm` (CoreMedia) once `SCStreamFrameInfo` attachments are decoupled
  upstream in screencapturekit-rs.
- `cv` (CoreVideo).
- `metal` (Metal).
