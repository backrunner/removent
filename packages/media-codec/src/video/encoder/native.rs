use super::*;

impl VideoEncoder {
    /// Captured buffers remain on the GPU/media path. Only changed dimensions
    /// invoke Metal; full-size frames go directly to the hardware encoder.
    pub fn encode_native(
        &mut self,
        buffer: &apple_cf::cv::CVPixelBuffer,
        pts_us: i64,
    ) -> Result<Vec<EncodedVideoFrame>, VideoError> {
        if buffer.pixel_format() != BGRA_FOURCC || self.session.is_none() {
            return Err(VideoError::Gpu(
                "native BGRA input is unavailable for this codec".into(),
            ));
        }
        let input = if (buffer.width(), buffer.height()) == (self.width, self.height) {
            buffer.clone()
        } else {
            if self.scaler.is_none() {
                self.scaler = Some(super::super::gpu::MetalScaler::new(
                    self.width,
                    self.height,
                )?);
                tracing::info!(
                    width = self.width,
                    height = self.height,
                    "Metal Lanczos scaling enabled"
                );
            }
            self.scaler.as_mut().unwrap().scale(buffer)?
        };
        let force = std::mem::take(&mut self.force_keyframe_pending);
        let encoded = encode_maybe_forced(force, |force| {
            self.session
                .as_ref()
                .unwrap()
                .encode_buffer(&input, pts_us, force)
        });
        if encoded.is_err() {
            // An unsuccessful VT flush may still retain the scaled output.
            self.scaler = None;
        }
        self.finish_encoded(encoded?, pts_us)
    }
}
