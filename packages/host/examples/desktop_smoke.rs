//! Run only in an isolated interactive desktop (CI Xvfb/Windows runner).
use removent_proto::{CodecId, KeyKind, KeyModifiers, MouseKind};
use std::time::Duration;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    anyhow::ensure!(
        std::env::var("REMOVENT_DESKTOP_TEST").as_deref() == Ok("1"),
        "Requires an isolated desktop and REMOVENT_DESKTOP_TEST=1"
    );
    let display = removent_input::DisplayGeometry::main()
        .ok_or_else(|| anyhow::anyhow!("No interactive display"))?;
    let (tx, mut rx) = removent_core::latest::channel();
    let _capture = removent_media_capture::start_display_capture(display.id, 640, 480, tx, None)?;
    let frame = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await?
        .ok_or_else(|| anyhow::anyhow!("No captured frame"))?;
    let pixels = frame
        .pixels
        .bgra(frame.width, frame.height)
        .map_err(anyhow::Error::msg)?;
    anyhow::ensure!(pixels.len() == 640 * 480 * 4, "Capture size");
    // The UI fixture paints red; verify real capture contains the known color.
    anyhow::ensure!(
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[2] > 180 && p[1] < 80 && p[0] < 80),
        "Fixture color absent from capture"
    );
    let mut encoder = removent_media_codec::VideoEncoder::new(CodecId::H264, 640, 480, 6000, 30)?;
    let packets = encoder.encode_bgra(&pixels, 0)?;
    anyhow::ensure!(
        !packets.is_empty() && packets[0].keyframe,
        "No initial H264 keyframe"
    );
    let decoder = removent_media_codec::VideoDecoder::new(
        CodecId::H264,
        640,
        480,
        encoder.parameter_sets().unwrap_or(&[]),
    )?;
    for packet in packets {
        decoder.decode_annexb(&packet.data, packet.pts_us)?;
    }
    decoder.flush()?;
    let decoded = decoder
        .try_recv_decoded()
        .ok_or_else(|| anyhow::anyhow!("No decoded H264 frame"))?;
    anyhow::ensure!(decoded.data.len() == pixels.len(), "Decoded size");
    anyhow::ensure!(
        decoded
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[2] > 160 && p[1] < 100 && p[0] < 100),
        "Decoded color absent"
    );
    removent_input::write_text("Removent clipboard smoke")?;
    anyhow::ensure!(
        removent_input::read_text()? == "Removent clipboard smoke",
        "Clipboard roundtrip"
    );
    let mut mouse = removent_input::mouse::MouseInjector::default();
    let mut keyboard = removent_input::keyboard::KeyboardInjector::default();
    // The fixture owns these coordinates and reports events independently.
    mouse
        .inject(MouseKind::LeftDown, 200., 200., KeyModifiers::empty())
        .map_err(anyhow::Error::msg)?;
    mouse
        .inject(MouseKind::LeftUp, 200., 200., KeyModifiers::empty())
        .map_err(anyhow::Error::msg)?;
    for (vk, mods, kind) in [
        (0x3b, KeyModifiers::CONTROL, KeyKind::FlagsChanged),
        (0x00, KeyModifiers::CONTROL, KeyKind::Down),
        (0x00, KeyModifiers::CONTROL, KeyKind::Up),
        (0x3b, KeyModifiers::empty(), KeyKind::FlagsChanged),
    ] {
        keyboard
            .inject(vk, mods, kind, None)
            .map_err(anyhow::Error::msg)?;
    }
    for ch in "hello".chars() {
        keyboard
            .inject(
                removent_input::UNICODE_ONLY_VK,
                KeyModifiers::empty(),
                KeyKind::Down,
                Some(ch),
            )
            .map_err(anyhow::Error::msg)?;
        keyboard
            .inject(
                removent_input::UNICODE_ONLY_VK,
                KeyModifiers::empty(),
                KeyKind::Up,
                Some(ch),
            )
            .map_err(anyhow::Error::msg)?;
    }
    println!("capture, H264 encode/decode, clipboard and native input submitted");
    Ok(())
}
