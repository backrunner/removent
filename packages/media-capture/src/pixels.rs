use apple_cf::cv::CVPixelBuffer;
use std::borrow::Cow;

/// Native buffers retain the capture pool's pixel buffer until encoding ends.
/// Retaining only its IOSurface would not prevent the capture pool reusing it.
#[derive(Debug)]
pub enum CapturedPixels {
    Bgra(std::sync::Arc<Vec<u8>>),
    Native(CVPixelBuffer),
}

impl CapturedPixels {
    pub fn valid_for(&self, width: usize, height: usize) -> bool {
        match self {
            Self::Bgra(bytes) => {
                width.checked_mul(height).and_then(|n| n.checked_mul(4)) == Some(bytes.len())
            }
            Self::Native(buffer) => {
                buffer.width() == width
                    && buffer.height() == height
                    && buffer.pixel_format() == u32::from_be_bytes(*b"BGRA")
            }
        }
    }

    /// CPU readback is reserved for VNC, software AV1 and GPU fallback.
    pub fn bgra(&self, width: usize, height: usize) -> Result<Cow<'_, [u8]>, String> {
        if !self.valid_for(width, height) {
            return Err("invalid captured pixel layout".into());
        }
        let Self::Native(buffer) = self else {
            let Self::Bgra(bytes) = self else {
                unreachable!()
            };
            return Ok(Cow::Borrowed(bytes));
        };
        let guard = buffer
            .lock_read_only()
            .map_err(|e| format!("capture readback lock: {e}"))?;
        let stride = guard.bytes_per_row();
        let row_bytes = width * 4;
        let base = guard.base_address();
        if base.is_null() || stride < row_bytes {
            return Err("invalid capture readback stride".into());
        }
        let mut bytes = vec![0; row_bytes * height];
        // SAFETY: a retained, read-locked BGRA pixel buffer owns each source
        // row; the destination is exclusively owned and sized above.
        unsafe {
            if stride == row_bytes {
                std::ptr::copy_nonoverlapping(base, bytes.as_mut_ptr(), bytes.len());
            } else {
                for row in 0..height {
                    std::ptr::copy_nonoverlapping(
                        base.add(row * stride),
                        bytes.as_mut_ptr().add(row * row_bytes),
                        row_bytes,
                    );
                }
            }
        }
        Ok(Cow::Owned(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_readback_honors_padding_and_retains_the_capture_buffer() {
        let surface = apple_cf::iosurface::IOSurface::create_with_properties(
            6,
            4,
            u32::from_be_bytes(*b"BGRA"),
            4,
            64,
            256,
            None,
        )
        .unwrap();
        let original = CVPixelBuffer::create_with_io_surface(&surface).unwrap();
        {
            let mut guard = original.lock_read_write().unwrap();
            let base = guard.base_address_mut().unwrap();
            for row in 0..4 {
                unsafe {
                    std::ptr::write_bytes(base.add(row * 64), row as u8 + 10, 24);
                }
            }
        }
        // Match SCK's +0-to-owned retain, then release the producer references.
        let retained = unsafe { CVPixelBuffer::from_raw_borrowed(original.as_ptr()) }.unwrap();
        drop(original);
        drop(surface);
        let pixels = CapturedPixels::Native(retained);
        assert!(!pixels.valid_for(4, 6));
        let bgra = pixels.bgra(6, 4).unwrap();
        assert_eq!(bgra.len(), 96);
        for row in 0..4 {
            assert_eq!(&bgra[row * 24..(row + 1) * 24], &[row as u8 + 10; 24]);
        }
    }
}
