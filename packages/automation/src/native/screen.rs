use super::*;
use base64::Engine;
use removent_input::DisplayGeometry;
use removent_proto::automation::Point;
use std::collections::VecDeque;
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
}
pub fn permitted() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}
pub struct Screen {
    next: u64,
    observations: VecDeque<Observation>,
}
impl Default for Screen {
    fn default() -> Self {
        // Keep observation IDs exactly representable in JSON/JavaScript, but distinct across sessions.
        Self {
            next: (rand::random::<u64>() & ((1u64 << 40) - 1)) << 12,
            observations: Default::default(),
        }
    }
}
struct Observation {
    id: u64,
    display: DisplayGeometry,
    w: u32,
    h: u32,
    at: Instant,
}
impl Screen {
    pub fn capture(
        &mut self,
        id: u64,
        max_width: u32,
        runtime: &tokio::runtime::Handle,
        cancel: &CancellationToken,
    ) -> Outcome {
        if !permitted() {
            return Err(Failure::new(
                "permission_denied",
                "Enable Screen Recording for the Removent host",
            ));
        }
        let display = if id == 0 {
            DisplayGeometry::main()
        } else {
            DisplayGeometry::by_id(id)
        }
        .ok_or_else(|| Failure::new("display_missing", "Display is no longer available"))?;
        let w = (display.width_px as u32).min(max_width);
        let h =
            ((display.height_px as f64 * w as f64 / display.width_px as f64).round() as u32).max(1);
        if w == 0 || h > 8192 {
            return Err(failure("Unsupported display dimensions"));
        }
        let (tx, mut rx) = removent_core::latest::channel();
        // A new stream starts after the preceding action. A cached video frame cannot prove freshness.
        let _capture =
            removent_media_capture::sck::start_remote_capture(display.id, w, h, tx, None, 10)
                .map_err(|_| failure("Screen capture could not start"))?;
        let frame = runtime.block_on(async {
            tokio::select! { biased;
                _ = cancel.cancelled() => Err(Failure::new("cancelled","Screenshot cancelled")),
                frame = tokio::time::timeout(Duration::from_secs(5),rx.recv()) => frame.ok().flatten().ok_or_else(|| failure("No fresh screen frame")),
            }
        })?;
        check_cancel(cancel)?;
        let bgra = frame
            .pixels
            .bgra(frame.width, frame.height)
            .map_err(failure)?;
        if bgra.len() != frame.width * frame.height * 4 {
            return Err(failure("Invalid capture buffer"));
        }
        let mut rgba = bgra.into_owned();
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
            pixel[3] = 255;
        }
        let mut bytes = Vec::new();
        {
            let mut encoder =
                png::Encoder::new(&mut bytes, frame.width as u32, frame.height as u32);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .map_err(|_| failure("Cannot encode screenshot"))?
                .write_image_data(&rgba)
                .map_err(|_| failure("Cannot encode screenshot"))?;
        }
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(failure(
                "Screenshot exceeds response limit; reduce max_width",
            ));
        }
        self.next += 1;
        let id = self.next;
        self.observations.push_back(Observation {
            id,
            display,
            w: frame.width as u32,
            h: frame.height as u32,
            at: Instant::now(),
        });
        while self.observations.len() > 8 {
            self.observations.pop_front();
        }
        Ok(
            json!({"observation":id,"display_id":display.id,"width":frame.width,"height":frame.height,"mime_type":"image/png",
            "captured_at_unix_ms": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis(), "coordinate_space":"screenshot_pixels","data":base64::engine::general_purpose::STANDARD.encode(bytes)}),
        )
    }
    pub fn point(&self, p: Point) -> Result<(f64, f64), Failure> {
        let obs = self
            .observations
            .iter()
            .find(|o| o.id == p.observation)
            .filter(|o| o.at.elapsed() < Duration::from_secs(60))
            .ok_or_else(|| {
                Failure::new(
                    "stale_observation",
                    "Take another screenshot before positioning input",
                )
            })?;
        let current = DisplayGeometry::by_id(obs.display.id as u64)
            .ok_or_else(|| failure("Display disconnected"))?;
        if current.width_px != obs.display.width_px
            || current.height_px != obs.display.height_px
            || current.origin_x != obs.display.origin_x
            || current.origin_y != obs.display.origin_y
            || current.scale != obs.display.scale
            || current.width_pt != obs.display.width_pt
            || current.height_pt != obs.display.height_pt
        {
            return Err(Failure::new(
                "stale_observation",
                "Display geometry changed; take another screenshot",
            ));
        }
        if p.x >= obs.w as f64 || p.y >= obs.h as f64 {
            return Err(Failure::new(
                "invalid_argument",
                "Point is outside screenshot",
            ));
        }
        Ok(obs
            .display
            .capture_px_to_global_pt(p.x, p.y, obs.w as f64, obs.h as f64))
    }
}
