# apple-cf

Safe Rust bindings for Apple's shared **Core\*** frameworks — the foundation underneath the [doom-fish](https://github.com/doom-fish) macOS Rust suite. The only Rust dependency is the family's [`doom-fish-utils`](https://crates.io/crates/doom-fish-utils) helper crate.

> **Status:** Active correctness and FFI-contract maintenance. See [`COVERAGE.md`](COVERAGE.md) for what the safe layer covers and what the audit numbers in [`COVERAGE_AUDIT.md`](COVERAGE_AUDIT.md) and [`COVERAGE_AUDIT_V2.md`](COVERAGE_AUDIT_V2.md) do and don't measure.

## What's in the box

| Module | Framework | Feature flag | Status |
|---|---|---|---|
| [`cf`](src/cf) | CoreFoundation values, collections, property lists, locale/formatter helpers, runtime primitives | — | ✅ |
| [`raw`](src/raw) | Exhaustive low-level CoreFoundation/CoreMedia/CoreVideo/IOSurface/Dispatch bindings | — | ✅ |
| [`cg`](src/cg) | CoreGraphics value types + bitmap drawing wrappers | `cg` | ✅ |
| [`iosurface`](src/iosurface) | IOSurface (zero-copy GPU buffers, multi-planar formats) | `iosurface` | ✅ |
| [`dispatch_queue`](src/dispatch_queue.rs) | Dispatch queues, async/apply helpers, groups, semaphores, timer sources | `dispatch` | ✅ |
| [`cm`](src/cm) | `CMTime`, `CMTimeRange`, `CMTimebase`, `CMSampleBuffer`, `CMBlockBuffer`, `CMFormatDescription`, `CMMetadataFormatDescription` | `cm` | ✅ |
| [`cv`](src/cv) | `CVBuffer`, `CVImageBuffer`, `CVPixelBuffer`, `CVPixelBufferPool`, `CVMetalTextureCache` | `cv` | ✅ |
| [`utils`](src/utils) | Shared FFI helpers (always on) | — | ✅ |

## Why this crate exists

Every doom-fish crate that wraps a media-adjacent Apple framework needs the same primitives — `CFString`, `CGRect`, `CVPixelBuffer`, `IOSurface`, dispatch queues, time values. Instead of re-vendoring those inside every crate (and drifting), this crate owns them once.

```text
Safe Rust wrappers
    ├── CoreFoundation / Dispatch / CoreMedia / CoreVideo ergonomic APIs
    ├── exhaustive raw C bindings in `apple_cf::raw`
    ├── direct C FFI for value-only primitives where appropriate
    └── Swift @_cdecl bridge for reference-counted / callback-heavy surfaces
```

## Requirements

- macOS 13.0+
- Xcode toolchain (Swift 5.9+); Command Line Tools alone is **not** enough — `xcrun --sdk macosx --show-sdk-version` must succeed
- Apple Silicon or Intel Mac

## Installation

```toml
[dependencies]
apple-cf = "0.11"
```

Or enable only the Rust modules you use:

```toml
[dependencies]
apple-cf = { version = ">=0.11, <0.12", default-features = false, features = ["cg", "cm", "cv", "dispatch", "iosurface"] }
```

The `cg`, `iosurface`, `dispatch`, `cm` and `cv` features gate the Rust modules only. The Swift bridge is built as a single static library, so every build compiles all of it and links CoreFoundation, CoreGraphics, CoreMedia, CoreVideo, IOSurface, Metal and Foundation whichever features are enabled. The `metal` feature is a deprecated no-op.

## Quick examples

### Build CoreFoundation values and collections

```rust
use apple_cf::cf::{CFArray, CFString};

let first = CFString::new("first");
let second = CFString::new("second");
let array = CFArray::from_values(&[&first, &second]);
assert_eq!(array.len(), 2);
```

### Create a timer-backed dispatch source

```rust,no_run
use apple_cf::dispatch_queue::DispatchSource;
use std::thread;
use std::time::Duration;

let source = DispatchSource::timer(Duration::from_millis(5), Duration::from_millis(1));
source.resume();
thread::sleep(Duration::from_millis(20));
source.cancel();
assert!(source.fire_count() > 0);
```

### Allocate an IOSurface-backed `CVPixelBuffer`

```rust,no_run
use apple_cf::cv::CVPixelBuffer;
use apple_cf::iosurface::IOSurface;

let surface = IOSurface::create(16, 16, 0x4247_5241, 4).expect("surface");
let pixel_buffer = CVPixelBuffer::create_with_io_surface(&surface).expect("pixel buffer");
assert!(pixel_buffer.is_backed_by_io_surface());
assert_eq!(pixel_buffer.width(), 16);
```

`CVPixelBuffer::create` passes no attributes, so the buffer it returns is not guaranteed to be IOSurface-backed.

## Architecture

This crate uses the same Swift-bridge pattern as the rest of the doom-fish crates:

- `swift-bridge/Sources/<Framework>Bridge/` exposes thin `@_cdecl` entry points
- `src/ffi/*.rs` declares the matching `extern "C"` bindings
- `src/<framework>/` provides the safe Rust API on top

The only runtime Rust dependency is `doom-fish-utils`.

## Ownership and mapped-memory contracts

Raw `from_raw` constructors adopt one caller-owned `+1` retain and are therefore `unsafe`. Use `from_raw_borrowed` when importing a live `+0` pointer that the wrapper must retain. `AsCFType` is an unsafe trait because implementations promise a valid Core Foundation object pointer.

`CVPixelBuffer` and `IOSurface` lock guards balance native synchronization and mapping only; they do not establish Rust exclusivity across retained, native, GPU, or cross-process aliases. Raw pointers remain available, while slice, row, plane, and zero-copy cursor views require `unsafe` with an explicit no-alias/no-mutation guarantee. `CGContext` clones likewise share one mutable native context, so its byte-slice views are unsafe even though drawing methods remain safe.

`CMBlockBuffer::as_slice` and `CMBlockBuffer::cursor_ref` are `unsafe` for the same reason: the bytes belong to Core Media, can be shared with other sample buffers, and must be initialized and left unmodified while the slice lives. `CMBlockBuffer::cursor` and `copy_data_bytes` copy instead. `CMSampleBuffer::audio_buffer_list` hands out read-only views that keep the backing block buffer alive.

Constructors that can fail on ordinary input return `Option` (`CFURL::from_string`, `CFURL::from_file_system_path`, `CFTimeZone::new`, `CFCalendar::new`, `CFMessagePort::create_echo_local`, `DispatchSemaphore::new`), and `CFString::new` accepts interior NUL bytes. `CFURL::from_string` follows the running system's `CFURLCreateWithString`, whose rules changed in macOS 27: it rejects an authority with an unbalanced or misplaced `[` or `]` or a second `@` (`http://[bad`, `http://bad]`, `http://user@@host/`), which earlier releases accept unchanged. `CFFileDescriptor` takes an `OwnedFd`, which Core Foundation closes, or a `BorrowedFd`, which it never closes.

`CVPixelBufferPool::create(..., max_buffers)` enforces the cap through Core Video's per-allocation threshold. Clones share that immutable policy across threads. Per-call auxiliary attributes are honored, flush flags map directly to the native API, and `try_create_pixel_buffer` distinguishes threshold exhaustion from other errors.

## Examples and tests

This release ships 15 numbered examples plus dedicated smoke tests for:

- CoreFoundation primitives, collections, property lists, resources, runtime helpers
- Dispatch serial, concurrent, main and global queues, `dispatch_async`, `dispatch_async_and_wait`, `dispatch_apply`, `dispatch_after`, groups, semaphores, and timer sources
- `CMTimeRange`, `CMTimebase`, `CMMetadataFormatDescription`, and low-level `CMTag`/`CMSync` coverage through `apple_cf::raw`
- `CVBuffer`, `CVImageBuffer`, `CVPixelBuffer`, `CVMetalTextureCache`, and the remaining CVMetal entry points through `apple_cf::raw`
- Exhaustive low-level constants / inline helpers surfaced by `apple_cf::raw`

`CVDisplayLink` remains exempt in the audit because Apple deprecated the family on macOS 15.

## Not wrapped yet

These are reachable only through the unsafe declarations in `apple_cf::raw`:

- `CMSampleBuffer` creation, timing-info arrays, `CMSampleBufferCopyPCMDataIntoAudioBufferList` and the other audio-buffer-list constructors
- Run-loop sources and observers, `CFRunLoopTimer`/`CFTimer` with a user callback, and socket, file-descriptor and stream callbacks
- Dispatch sources with user event handlers, dispatch work items and dispatch I/O
- IOSurface lookup by ID, Mach port or XPC object, and Metal texture creation from `CVMetalTextureCache`
- `CFTreeRef`: `cf::CFTree` is a Swift-side tree of retained values, not a Core Foundation tree

## License

Licensed under either of [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT) at your option.
