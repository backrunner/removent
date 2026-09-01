//! Display information and coordinate conversion.

use core_graphics::display::{CGDisplay, CGMainDisplayID};

#[derive(Debug, Clone, Copy)]
pub struct DisplayGeometry {
    pub id: u32,
    /// Logical point width (CG bounds).
    pub width_pt: f64,
    pub height_pt: f64,
    /// Physical pixel width.
    pub width_px: u64,
    pub height_px: u64,
    pub scale: f64,
    /// Origin in the global coordinate system (multi-display arrangement).
    pub origin_x: f64,
    pub origin_y: f64,
    pub is_main: bool,
}

impl DisplayGeometry {
    pub fn from_cg(id: u32) -> Self {
        let d = CGDisplay::new(id);
        let b = d.bounds();
        let scale = if b.size.width > 0.0 {
            d.pixels_wide() as f64 / b.size.width
        } else {
            1.0
        };
        Self {
            id,
            width_pt: b.size.width,
            height_pt: b.size.height,
            width_px: d.pixels_wide(),
            height_px: d.pixels_high(),
            scale,
            origin_x: b.origin.x,
            origin_y: b.origin.y,
            is_main: unsafe { CGMainDisplayID() } == id,
        }
    }

    pub fn all() -> Vec<Self> {
        CGDisplay::active_displays()
            .unwrap_or_default()
            .into_iter()
            .map(Self::from_cg)
            .collect()
    }

    pub fn main() -> Option<Self> {
        let id = unsafe { CGMainDisplayID() };
        (id != 0).then(|| Self::from_cg(id))
    }

    /// Looks up an active display by id (the id advertised in DisplayInfo).
    pub fn by_id(id: u64) -> Option<Self> {
        Self::all().into_iter().find(|g| u64::from(g.id) == id)
    }

    /// Effective DPI, matching the value advertised in DisplayInfo (96 × scale).
    pub fn dpi(&self) -> f64 {
        96.0 * self.scale
    }

    /// Converts capture-space pixels (origin at this display's top-left) into
    /// global display points — the coordinate space CGEvent injection expects.
    pub fn px_to_global_pt(&self, x_px: f64, y_px: f64) -> (f64, f64) {
        let scale = if self.scale > 0.0 { self.scale } else { 1.0 };
        (self.origin_x + x_px / scale, self.origin_y + y_px / scale)
    }

    /// Converts capture-frame pixels into global display points, where the
    /// capture was scaled to `capture_w_px`×`capture_h_px` (the host caps the
    /// capture at 1920×1080, so on Retina displays capture pixels no longer
    /// equal physical pixels). Falls back to [`Self::px_to_global_pt`] when
    /// the capture dims are unknown or match the display's physical pixels.
    pub fn capture_px_to_global_pt(
        &self,
        x_px: f64,
        y_px: f64,
        capture_w_px: f64,
        capture_h_px: f64,
    ) -> (f64, f64) {
        let (w, h) = (self.width_px as f64, self.height_px as f64);
        let matches_display =
            (capture_w_px - w).abs() < f64::EPSILON && (capture_h_px - h).abs() < f64::EPSILON;
        if capture_w_px <= 0.0 || capture_h_px <= 0.0 || matches_display {
            return self.px_to_global_pt(x_px, y_px);
        }
        self.px_to_global_pt(x_px * w / capture_w_px, y_px * h / capture_h_px)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geo(scale: f64, origin_x: f64, origin_y: f64) -> DisplayGeometry {
        DisplayGeometry {
            id: 1,
            width_pt: 1440.0,
            height_pt: 900.0,
            width_px: 2880,
            height_px: 1800,
            scale,
            origin_x,
            origin_y,
            is_main: true,
        }
    }

    #[test]
    fn retina_px_maps_to_half_points() {
        let g = geo(2.0, 0.0, 0.0);
        assert_eq!(g.px_to_global_pt(200.0, 100.0), (100.0, 50.0));
    }

    #[test]
    fn origin_is_added_after_scaling() {
        // Secondary display arranged to the right of a 1440pt main display.
        let g = geo(2.0, 1440.0, -50.0);
        assert_eq!(g.px_to_global_pt(200.0, 100.0), (1540.0, 0.0));
    }

    #[test]
    fn non_retina_is_identity_plus_origin() {
        let g = geo(1.0, 100.0, 200.0);
        assert_eq!(g.px_to_global_pt(10.0, 20.0), (110.0, 220.0));
    }

    #[test]
    fn zero_scale_falls_back_to_one() {
        let g = geo(0.0, 0.0, 0.0);
        assert_eq!(g.px_to_global_pt(10.0, 20.0), (10.0, 20.0));
    }

    #[test]
    fn dpi_scales_with_display() {
        assert_eq!(geo(2.0, 0.0, 0.0).dpi(), 192.0);
        assert_eq!(geo(1.0, 0.0, 0.0).dpi(), 96.0);
    }

    #[test]
    fn downscaled_capture_remaps_to_display_pixels() {
        // 2880×1800 @2x display captured at 1920×1200 (aspect-fit into the
        // 1920×1080 bounding box would be 1728×1080; use 1920×1200 here to
        // exercise a plain 1.5× downscale).
        let g = geo(2.0, 0.0, 0.0);
        // Capture center (960, 600) → display px (1440, 900) → pt (720, 450).
        assert_eq!(
            g.capture_px_to_global_pt(960.0, 600.0, 1920.0, 1200.0),
            (720.0, 450.0)
        );
        // Corner: capture (1920, 1200) → display px (2880, 1800) → pt (1440, 900).
        assert_eq!(
            g.capture_px_to_global_pt(1920.0, 1200.0, 1920.0, 1200.0),
            (1440.0, 900.0)
        );
    }

    #[test]
    fn capture_dims_matching_display_are_identity() {
        let g = geo(2.0, 1440.0, -50.0);
        assert_eq!(
            g.capture_px_to_global_pt(200.0, 100.0, 2880.0, 1800.0),
            (1540.0, 0.0)
        );
    }
}
