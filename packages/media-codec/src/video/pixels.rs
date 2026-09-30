use super::*;

/// CVPixelBuffer → BGRA (8/10-bit bi-planar YUV conversion for non-BGRA formats).
pub(super) fn pixel_buffer_to_bgra(pb: &apple_cf::cv::CVPixelBuffer) -> Option<Vec<u8>> {
    let w = pb.width();
    let h = pb.height();
    let fmt = pb.pixel_format();
    let out_len = w.checked_mul(h)?.checked_mul(4)?;
    let guard = pb.lock_read_only().ok()?;

    if fmt == BGRA_FOURCC {
        let base = guard.base_address();
        let bpr = pb.bytes_per_row();
        let mut out = vec![0u8; out_len];
        for row in 0..h {
            // SAFETY: row ranges are protected by the read lock; bpr >= w*4 is
            // guaranteed by the pixel format.
            let src = unsafe { base.add(row * bpr) };
            let dst = &mut out[row * w * 4..(row + 1) * w * 4];
            dst.copy_from_slice(unsafe { std::slice::from_raw_parts(src, w * 4) });
        }
        return Some(out);
    }

    if fmt == u32::from_be_bytes(*b"ARGB") {
        // ARGB memory order [A,R,G,B] must be reordered to BGRA [B,G,R,A]
        // (i.e. reverse each 4-byte pixel).
        let base = guard.base_address();
        let bpr = pb.bytes_per_row();
        let mut out = vec![0u8; out_len];
        for row in 0..h {
            // SAFETY: row ranges are protected by the read lock; bpr >= w*4 is
            // guaranteed by the pixel format.
            let src = unsafe { std::slice::from_raw_parts(base.add(row * bpr), w * 4) };
            let dst = &mut out[row * w * 4..(row + 1) * w * 4];
            let (src_px, _) = src.as_chunks::<4>();
            let (dst_px, _) = dst.as_chunks_mut::<4>();
            for (s, d) in src_px.iter().zip(dst_px.iter_mut()) {
                *d = [s[3], s[2], s[1], s[0]]; // [B,G,R,A]
            }
        }
        return Some(out);
    }

    if matches!(fmt, f if f == u32::from_be_bytes(*b"420v") || f == u32::from_be_bytes(*b"420f")) {
        // Bi-planar NV12: use the plane APIs to get each base address and
        // stride, avoiding layout guessing.
        let y_ptr = guard.base_address_of_plane(0)?;
        let uv_ptr = guard.base_address_of_plane(1)?;
        let y_stride = pb.bytes_per_row_of_plane(0);
        let uv_stride = pb.bytes_per_row_of_plane(1);
        let bt709 = is_bt709(pb);
        let color = YuvColor {
            bt709,
            full_range: fmt == u32::from_be_bytes(*b"420f"),
        };
        let out = nv12_to_bgra(y_ptr, y_stride, uv_ptr, uv_stride, w, h, color);
        // Unlock only after the planes have been fully read.
        drop(guard);
        return Some(out);
    }

    if fmt == u32::from_be_bytes(*b"x420") {
        // 10-bit video-range 4:2:0 (P010): each little-endian component is
        // stored in the ten most-significant bits of a 16-bit word.
        let y_ptr = guard.base_address_of_plane(0)?;
        let uv_ptr = guard.base_address_of_plane(1)?;
        let y_stride = pb.bytes_per_row_of_plane(0);
        let uv_stride = pb.bytes_per_row_of_plane(1);
        let out = p010_video_to_bgra(y_ptr, y_stride, uv_ptr, uv_stride, w, h, is_bt709(pb));
        // Unlock only after the planes have been fully read.
        drop(guard);
        return Some(out);
    }
    tracing::warn!(
        fmt = format_args!("{fmt:#010x}"),
        "unexpected decode output format"
    );
    None
}

/// Reads the color-matrix attachment of the image buffer; returns true for
/// BT.709, treats missing/other as BT.601.
pub(super) fn is_bt709(pb: &apple_cf::cv::CVPixelBuffer) -> bool {
    // SAFETY: pb is valid; the attachment is a +0 borrowed reference and
    // from_raw_retained retains it itself.
    unsafe {
        let key = std::ptr::addr_of!(apple_cf::raw::kCVImageBufferYCbCrMatrixKey).read();
        let v = apple_cf::raw::CVBufferGetAttachment(pb.as_ptr().cast(), key, std::ptr::null_mut());
        if v.is_null() {
            return false;
        }
        apple_cf::cf::CFString::from_raw_retained(v.cast_mut())
            .is_some_and(|s| s.to_string_lossy() == "ITU_R_709-2")
    }
}

/// BT.601/BT.709 NV12 → BGRA. Valid uv bytes per row = ceil(w/2)*2.
#[derive(Clone, Copy)]
pub(super) struct YuvColor {
    pub(super) bt709: bool,
    pub(super) full_range: bool,
}

pub(super) fn nv12_to_bgra(
    y_plane: *const u8,
    y_stride: usize,
    uv_plane: *const u8,
    uv_stride: usize,
    w: usize,
    h: usize,
    color: YuvColor,
) -> Vec<u8> {
    // Fixed-point 8.8 coefficients; BT.601: Kr=0.299 Kb=0.114,
    // BT.709: Kr=0.2126 Kb=0.0722.
    let (y_scale, y_offset, cr, cgu, cgv, cb) = match (color.bt709, color.full_range) {
        (false, false) => (298, 16, 409, 100, 208, 516),
        (true, false) => (298, 16, 459, 55, 136, 541),
        (false, true) => (256, 0, 359, 88, 183, 454),
        (true, true) => (256, 0, 403, 48, 120, 475),
    };
    let mut out = vec![0u8; w * h * 4];
    let uv_w = w.div_ceil(2) * 2;
    for row in 0..h {
        // SAFETY: all row offsets stay within the corresponding plane
        // allocation (stride*h / stride*(h/2)).
        let y_row = unsafe { std::slice::from_raw_parts(y_plane.add(row * y_stride), w) };
        let uv_row =
            unsafe { std::slice::from_raw_parts(uv_plane.add((row / 2) * uv_stride), uv_w) };
        for (col, &y_raw) in y_row.iter().enumerate() {
            let y = i32::from(y_raw);
            let uv_i = (col / 2) * 2;
            let u = i32::from(uv_row[uv_i]) - 128;
            let v = i32::from(uv_row[uv_i + 1]) - 128;
            let c = y - y_offset;
            let r = ((y_scale * c + cr * v + 128) >> 8).clamp(0, 255);
            let g = ((y_scale * c - cgu * u - cgv * v + 128) >> 8).clamp(0, 255);
            let b = ((y_scale * c + cb * u + 128) >> 8).clamp(0, 255);
            let o = (row * w + col) * 4;
            out[o] = b as u8;
            out[o + 1] = g as u8;
            out[o + 2] = r as u8;
            out[o + 3] = 255;
        }
    }
    out
}

/// BT.601/BT.709 limited-range P010 (`x420`) → BGRA.
pub(super) fn p010_video_to_bgra(
    y_plane: *const u8,
    y_stride: usize,
    uv_plane: *const u8,
    uv_stride: usize,
    w: usize,
    h: usize,
    bt709: bool,
) -> Vec<u8> {
    let (cr, cgu, cgv, cb) = if bt709 {
        (459, 55, 136, 541)
    } else {
        (409, 100, 208, 516)
    };
    let mut out = vec![0u8; w * h * 4];
    for row in 0..h {
        for col in 0..w {
            let y_offset = row * y_stride + col * 2;
            let uv_offset = (row / 2) * uv_stride + (col / 2) * 4;
            // SAFETY: CoreVideo guarantees that each plane contains its
            // stride-sized rows. `read_unaligned` avoids assuming word
            // alignment for a plane base or padded row.
            let y = i32::from(unsafe { read_p010(y_plane.add(y_offset)) });
            let u = i32::from(unsafe { read_p010(uv_plane.add(uv_offset)) }) - 512;
            let v = i32::from(unsafe { read_p010(uv_plane.add(uv_offset + 2)) }) - 512;
            let c = y - 64;
            // The 10-bit ranges are exactly four times their 8-bit
            // counterparts, so 8.8 coefficients use a 10-bit final shift.
            let r = ((298 * c + cr * v + 512) >> 10).clamp(0, 255);
            let g = ((298 * c - cgu * u - cgv * v + 512) >> 10).clamp(0, 255);
            let b = ((298 * c + cb * u + 512) >> 10).clamp(0, 255);
            let o = (row * w + col) * 4;
            out[o] = b as u8;
            out[o + 1] = g as u8;
            out[o + 2] = r as u8;
            out[o + 3] = 255;
        }
    }
    out
}

/// Reads one little-endian P010 word and removes its six padding bits.
unsafe fn read_p010(ptr: *const u8) -> u16 {
    u16::from_le(unsafe { ptr.cast::<u16>().read_unaligned() }) >> 6
}

// SAFETY: CoreFoundation/CoreMedia objects are thread-safe (CFType docs);
// the mpsc receiver is protected by a Mutex, so shared (&) access is safe.
// VideoDecoder only holds these members.
unsafe impl Send for VideoDecoder {}
unsafe impl Sync for VideoDecoder {}
