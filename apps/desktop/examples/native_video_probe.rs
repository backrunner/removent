//! Real VT decode -> CVMetalTexture -> GPUI shader -> GPU readback.
//! This verifies colors and retention, not display timing or end-to-end latency.
//! Run: cargo run -p removent-app --example native_video_probe -- /tmp/native-video.png
use core_foundation::base::TCFType;
use core_video::pixel_buffer::CVPixelBuffer;
use gpui::{App, Context, HeadlessAppContext, Window, canvas, prelude::*, px, size};
use removent_media_codec::{VideoDecoder, VideoEncoder};
use removent_proto::CodecId;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

struct Picture {
    surface: CVPixelBuffer,
    presented: Arc<AtomicUsize>,
    animate: bool,
}
impl Render for Picture {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if self.animate {
            window.request_animation_frame();
        }
        let buffer = self.surface.clone();
        let presented = self.presented.clone();
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                window.paint_surface_with_callback(
                    bounds,
                    buffer,
                    Some(gpui::SurfacePresented(Arc::new(move |_| {
                        presented.fetch_add(1, Ordering::Relaxed);
                    }))),
                );
            },
        )
        .size_full()
    }
}
fn main() -> anyhow::Result<()> {
    let (w, h) = (318, 242);
    let colors = [
        [0, 0, 235, 255],
        [0, 235, 0, 255],
        [235, 0, 0, 255],
        [235, 235, 235, 255],
    ];
    let mut pixels = vec![0; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            pixels[(y * w + x) * 4..][..4]
                .copy_from_slice(&colors[(y / (h / 2)) * 2 + x / (w / 2)]);
        }
    }
    let mut encoder = VideoEncoder::new(CodecId::H264, w, h, 4000, 60)?;
    let packets = encoder.encode_bgra(&pixels, 12_345)?;
    let (tx, rx) = std::sync::mpsc::channel();
    let decoder = VideoDecoder::with_surface_output(
        CodecId::H264,
        w,
        h,
        encoder.parameter_sets().unwrap(),
        true,
        move |frame| {
            let _ = tx.send(frame);
        },
    )?;
    for packet in packets {
        decoder.decode_annexb(&packet.data, packet.pts_us)?;
    }
    let decoded = rx.recv_timeout(Duration::from_secs(3))?;
    anyhow::ensure!(decoded.data.is_empty(), "unexpected CPU readback");
    let native = decoded.surface.unwrap();
    let surface = unsafe { CVPixelBuffer::wrap_under_get_rule(native.as_ptr().cast()) };
    drop(native);
    drop(decoder);
    drop(encoder);
    if std::env::args().any(|arg| arg == "--present") {
        let callbacks = Arc::new(AtomicUsize::new(0));
        gpui_platform::application().run(move |cx| {
            let observed = callbacks.clone();
            cx.open_window(
                gpui::WindowOptions {
                    window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(
                        None,
                        size(px(636.), px(484.)),
                        cx,
                    ))),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|_| Picture {
                        surface,
                        presented: callbacks,
                        animate: true,
                    })
                },
            )
            .unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let count = observed.load(Ordering::Relaxed);
                if count == 0 {
                    eprintln!("FAIL: no actual drawable presentation callback");
                    std::process::exit(1);
                }
                println!("PASS: {count} actual drawable callbacks in a 3-second redraw probe (not streaming FPS)");
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
        return Ok(());
    }
    let platform = gpui_platform::current_platform(true);
    let mut cx = HeadlessAppContext::with_platform(
        platform.text_system(),
        Arc::new(()),
        gpui_platform::current_headless_renderer,
    );
    let presented = Arc::new(AtomicUsize::new(0));
    let window = cx.open_window(size(px(w as f32), px(h as f32)), |_, cx: &mut App| {
        cx.new(|_| Picture {
            surface,
            presented: presented.clone(),
            animate: false,
        })
    })?;
    cx.update_window(window.into(), |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    })?;
    let image = cx.capture_screenshot(window.into())?;
    let (w, h) = image.dimensions(); // Device pixels include the window scale factor.
    for (x, y, expected) in [
        (w / 4, h / 4, [235, 0, 0]),
        (w * 3 / 4, h / 4, [0, 235, 0]),
        (w / 4, h * 3 / 4, [0, 0, 235]),
        (w * 3 / 4, h * 3 / 4, [235, 235, 235]),
    ] {
        let got = image.get_pixel(x, y).0;
        for ch in 0..3 {
            anyhow::ensure!(
                got[ch].abs_diff(expected[ch]) < 20,
                "GPU pixel ({x},{y}): {got:?}, expected {expected:?}"
            );
        }
        anyhow::ensure!(got[3] == 255, "opaque video alpha");
    }
    anyhow::ensure!(
        presented.load(Ordering::Relaxed) == 0,
        "offscreen paint must not claim presentation"
    );
    if let Some(path) = std::env::args().nth(1) {
        image.save(path)?;
    }
    println!(
        "PASS: H264 native decode, retained buffer, Metal BGRA colors/orientation/alpha; no false presentation"
    );
    Ok(())
}
