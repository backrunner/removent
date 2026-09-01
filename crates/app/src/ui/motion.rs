//! Motion presets approximating macOS system animations.
//!
//! Apple's current UI motion (SwiftUI `.smooth`, AppKit sheets and menus) is spring-driven:
//! fast attack, gentle settle, and critically damped on macOS surfaces so nothing overshoots.
//! gpui's `Animation` feeds a normalized `0..=1` time delta through an easing function, so the
//! spring is expressed in closed form (damped harmonic oscillator) as the easing.

use gpui::Animation;
use std::f32::consts::PI;
use std::time::Duration;

/// Damped-spring easing: `response` is the nominal settle time in seconds (maps to the spring
/// frequency), `damping` is the damping ratio (1.0 = critically damped, no overshoot). The
/// output is clamped to `0..=1` to satisfy gpui's delta assertion even for underdamped springs.
pub fn spring(response: f32, damping: f32) -> impl Fn(f32) -> f32 {
    let omega = 2.0 * PI / response.max(0.001);
    let zeta = damping.clamp(0.05, 1.0);
    move |t: f32| {
        if t <= 0.0 {
            return 0.0;
        }
        if t >= 1.0 {
            return 1.0;
        }
        let x = if zeta < 1.0 {
            // Underdamped: decaying cosine with phase correction so x(0) = 0 exactly.
            let wd = omega * (1.0 - zeta * zeta).sqrt();
            let envelope = (-zeta * omega * t).exp();
            1.0 - envelope * ((wd * t).cos() + (zeta * omega / wd) * (wd * t).sin())
        } else {
            // Critically damped: the macOS sheet/menu feel — no bounce, ever.
            1.0 - (-omega * t).exp() * (1.0 + omega * t)
        };
        x.clamp(0.0, 1.0)
    }
}

/// Modal dialog entry (scrim fade + card slide share one curve, like an AppKit sheet and its
/// dimming layer): ~320ms, critically damped.
pub fn modal_enter() -> Animation {
    Animation::new(Duration::from_secs_f32(0.32)).with_easing(spring(0.32, 1.0))
}

/// Floating toolbar entry: menu-like snap, ~180ms.
pub fn toolbar_enter() -> Animation {
    Animation::new(Duration::from_secs_f32(0.18)).with_easing(spring(0.18, 1.0))
}

/// Full-screen overlay fade (e.g. session-ended scrim): ~240ms.
pub fn overlay_fade() -> Animation {
    Animation::new(Duration::from_secs_f32(0.24)).with_easing(spring(0.24, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_endpoints_are_exact() {
        for damping in [0.6, 0.85, 1.0] {
            let s = spring(0.3, damping);
            assert_eq!(s(0.0), 0.0);
            assert_eq!(s(1.0), 1.0);
        }
    }

    #[test]
    fn critically_damped_never_overshoots_and_converges() {
        let s = spring(0.32, 1.0);
        let mut prev = 0.0f32;
        for i in 1..=100 {
            let x = s(i as f32 / 100.0);
            assert!((0.0..=1.0).contains(&x), "out of range at {i}: {x}");
            assert!(x >= prev, "not monotonic at {i}: {x} < {prev}");
            prev = x;
        }
        assert!(s(0.98) > 0.99, "should be essentially settled near the end");
    }

    #[test]
    fn underdamped_stays_within_assertion_bounds() {
        let s = spring(0.3, 0.6);
        for i in 0..=200 {
            let x = s(i as f32 / 200.0);
            assert!((0.0..=1.0).contains(&x), "out of range at {i}: {x}");
        }
    }
}
