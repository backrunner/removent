//! RFC 6143 CopyRect and Hextile, using the negotiated 32-bit BGRA format.
//! Scratch storage is bounded to one 16x16 tile, independent of desktop size.

use std::io;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt};

#[derive(Clone, Copy)]
pub(super) struct Rectangle {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub(super) async fn copy_rect(
    stream: &mut (impl AsyncRead + Unpin),
    pixels: &mut [u8],
    width: usize,
    height: usize,
    rect: Rectangle,
) -> io::Result<Duration> {
    let sx = stream.read_u16().await? as usize;
    let sy = stream.read_u16().await? as usize;
    if sx + rect.width > width || sy + rect.height > height {
        return Err(invalid("VNC CopyRect source outside framebuffer"));
    }
    let started = Instant::now();
    // memmove semantics on both axes, including overlapping scroll regions.
    for index in 0..rect.height {
        let row = if rect.y > sy {
            rect.height - index - 1
        } else {
            index
        };
        let from = ((sy + row) * width + sx) * 4;
        let to = ((rect.y + row) * width + rect.x) * 4;
        pixels.copy_within(from..from + rect.width * 4, to);
        if index % 32 == 31 {
            tokio::task::yield_now().await;
        }
    }
    Ok(started.elapsed())
}

async fn pixel(stream: &mut (impl AsyncRead + Unpin)) -> io::Result<[u8; 4]> {
    let mut color = [0; 4];
    stream.read_exact(&mut color).await?;
    color[3] = 255;
    Ok(color)
}

fn fill(tile: &mut [u8; 1024], width: usize, rect: Rectangle, color: [u8; 4]) -> Duration {
    let started = Instant::now();
    for y in rect.y..rect.y + rect.height {
        let start = (y * width + rect.x) * 4;
        for p in tile[start..start + rect.width * 4].as_chunks_mut::<4>().0 {
            *p = color;
        }
    }
    started.elapsed()
}

pub(super) async fn hextile(
    stream: &mut (impl AsyncRead + Unpin),
    pixels: &mut [u8],
    stride: usize,
    rect: Rectangle,
) -> io::Result<Duration> {
    let mut decode_time = Duration::ZERO;
    let mut background = None;
    let mut foreground = None;
    let mut tile = [0; 1024];
    for ty in (0..rect.height).step_by(16) {
        for tx in (0..rect.width).step_by(16) {
            let width = (rect.width - tx).min(16);
            let height = (rect.height - ty).min(16);
            let flags = stream.read_u8().await?;
            if flags & 1 != 0 {
                // When Raw is set all other bits are ignored (RFC 6143).
                stream.read_exact(&mut tile[..width * height * 4]).await?;
                let started = Instant::now();
                for p in tile[..width * height * 4].as_chunks_mut::<4>().0 {
                    p[3] = 255;
                }
                decode_time += started.elapsed();
                background = None;
                foreground = None;
            } else {
                if flags & !0x1f != 0 {
                    return Err(invalid("invalid Hextile flags"));
                }
                if flags & 2 != 0 {
                    background = Some(pixel(stream).await?);
                }
                let bg = background.ok_or_else(|| invalid("missing Hextile background"))?;
                decode_time += fill(
                    &mut tile,
                    width,
                    Rectangle {
                        x: 0,
                        y: 0,
                        width,
                        height,
                    },
                    bg,
                );
                if flags & 4 != 0 {
                    foreground = Some(pixel(stream).await?);
                }
                if flags & 8 != 0 {
                    let count = stream.read_u8().await?;
                    for _ in 0..count {
                        let color = if flags & 16 != 0 {
                            pixel(stream).await?
                        } else {
                            foreground.ok_or_else(|| invalid("missing Hextile foreground"))?
                        };
                        let xy = stream.read_u8().await?;
                        let wh = stream.read_u8().await?;
                        let sub = Rectangle {
                            x: (xy >> 4) as usize,
                            y: (xy & 15) as usize,
                            width: (wh >> 4) as usize + 1,
                            height: (wh & 15) as usize + 1,
                        };
                        if sub.x + sub.width > width || sub.y + sub.height > height {
                            return Err(invalid("Hextile subrectangle outside tile"));
                        }
                        decode_time += fill(&mut tile, width, sub, color);
                    }
                }
                if flags & 16 != 0 {
                    foreground = None;
                }
            }
            let started = Instant::now();
            for row in 0..height {
                let start = ((rect.y + ty + row) * stride + rect.x + tx) * 4;
                let source = row * width * 4;
                pixels[start..start + width * 4].copy_from_slice(&tile[source..source + width * 4]);
            }
            decode_time += started.elapsed();
        }
        // Explicit cooperation even when a whole compressed screen is buffered.
        tokio::task::yield_now().await;
    }
    Ok(decode_time)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn copy_rect_matches_snapshot_for_overlapping_regions() {
        for (sx, sy, x, y) in [(0, 0, 1, 1), (1, 1, 0, 0), (0, 0, 1, 0), (1, 0, 0, 0)] {
            let original: Vec<u8> = (0..64).collect();
            let mut pixels = original.clone();
            let rect = Rectangle {
                x,
                y,
                width: 3,
                height: 3,
            };
            copy_rect(
                &mut [0, sx as u8, 0, sy as u8].as_slice(),
                &mut pixels,
                4,
                4,
                rect,
            )
            .await
            .unwrap();
            let mut expected = original.clone();
            for row in 0..3 {
                let from = ((sy + row) * 4 + sx) * 4;
                let to = ((y + row) * 4 + x) * 4;
                expected[to..to + 12].copy_from_slice(&original[from..from + 12]);
            }
            assert_eq!(pixels, expected);
        }
        assert!(
            copy_rect(
                &mut [0, 4, 0, 0].as_slice(),
                &mut [0; 64],
                4,
                4,
                Rectangle {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1
                }
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn hextile_preserves_colors_across_tiles_and_handles_partial_edges() {
        let mut pixels = vec![99; 35 * 19 * 4];
        // A 33x17 rectangle at (1,1): six tiles, including right/bottom edges.
        let bytes = [
            14, 1, 2, 3, 0, 4, 5, 6, 0, 1, 0x12, 0x23, 8, 1, 0x00,
            0x00, // inherited background and foreground
            0,    // 1x16 tile with inherited background
            24, 1, 7, 8, 9, 0, 0x00, 0xf0, // colored 16x1 subrectangle
            0,    // 16x1 background
            1, 10, 11, 12, 0, // raw 1x1 edge
        ];
        hextile(
            &mut bytes.as_slice(),
            &mut pixels,
            35,
            Rectangle {
                x: 1,
                y: 1,
                width: 33,
                height: 17,
            },
        )
        .await
        .unwrap();
        let p = |x: usize, y: usize| &pixels[(y * 35 + x) * 4..(y * 35 + x) * 4 + 4];
        assert_eq!(p(0, 0), &[99; 4]);
        assert_eq!(p(1, 1), &[1, 2, 3, 255]);
        assert_eq!(p(2, 3), &[4, 5, 6, 255]);
        assert_eq!(p(17, 1), &[4, 5, 6, 255]);
        assert_eq!(p(33, 1), &[1, 2, 3, 255]);
        assert_eq!(p(1, 17), &[7, 8, 9, 255]);
        assert_eq!(p(17, 17), &[1, 2, 3, 255]);
        assert_eq!(p(33, 17), &[10, 11, 12, 255]);
        assert_eq!(p(34, 18), &[99; 4]);
    }

    #[tokio::test]
    async fn malformed_or_truncated_hextile_is_rejected() {
        for bytes in [
            vec![0],
            vec![32],
            vec![2, 1],
            vec![14, 1, 2, 3, 0, 4, 5, 6, 0, 1, 0x11, 0x11],
        ] {
            assert!(
                hextile(
                    &mut bytes.as_slice(),
                    &mut [0; 16],
                    2,
                    Rectangle {
                        x: 0,
                        y: 0,
                        width: 2,
                        height: 2
                    }
                )
                .await
                .is_err()
            );
        }
    }

    #[tokio::test]
    async fn solid_4k_hextile_is_bounded_and_yields_to_input() {
        let mut bytes = vec![0; 240 * 135 + 4];
        bytes[..5].copy_from_slice(&[2, 1, 2, 3, 0]);
        let mut pixels = vec![0; 3840 * 2160 * 4];
        let input_ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let marker = input_ran.clone();
        let input = tokio::spawn(async move {
            marker.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        let started = std::time::Instant::now();
        hextile(
            &mut bytes.as_slice(),
            &mut pixels,
            3840,
            Rectangle {
                x: 0,
                y: 0,
                width: 3840,
                height: 2160,
            },
        )
        .await
        .unwrap();
        eprintln!(
            "4K solid Hextile: {} bytes, decode {:?}; Raw: {} bytes",
            bytes.len(),
            started.elapsed(),
            pixels.len()
        );
        assert!(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [1, 2, 3, 255])
        );
        assert!(input_ran.load(std::sync::atomic::Ordering::Relaxed));
        input.await.unwrap();
    }
}
