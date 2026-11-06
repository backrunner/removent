# Removent changes to gpui-pre-apple 0.3.8

This copy is the version already pinned by Cargo.lock, with its Apache-2.0
license retained. It is paired with the existing `vendor/gpui` patch.

- Accept native BGRA CVPixelBuffers in the surface pipeline. Sample BGRA through
  Metal directly; retain the existing full-range NV12 shader for other callers.
- Invoke optional surface callbacks with MTLDrawable.presentedTime after actual
  presentation. Paint/layout time is not treated as display completion.
- Keep native pixel buffers and CVMetalTexture wrappers alive through GPU
  completion. Texture creation failures reject the frame instead of panicking
  or reporting a presentation that did not happen.

The application opts into native BGRA decoder output; offline callers and
CPU-only codecs retain their existing byte-buffer path.
