use super::{FrameSignal, FrameSize, MAX_NAME, MAX_PIXELS, PixelFormat, VncStats};
use crate::DecodedFrame;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader, ReadBuf};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{RwLock, mpsc};

pub(super) async fn write_pixel_format(stream: &mut OwnedWriteHalf) -> io::Result<()> {
    stream
        .write_all(&[
            0, 0, 0, 0, // type + padding
            32, 24, 0, 1, // 32bpp, depth 24, little endian, true colour
            0, 255, 0, 255, 0, 255, // max RGB (big endian)
            16, 8, 0, 0, 0, 0, // shifts + padding
        ])
        .await
}

pub(super) async fn write_set_encodings(stream: &mut OwnedWriteHalf) -> io::Result<()> {
    // Keep Raw as the only requested encoding so every supported server can
    // produce pixels without a codec-specific decoder.
    let encodings: &[i32] = &[0];
    let count = u16::try_from(encodings.len()).expect("static encoding list fits u16");
    let mut msg = Vec::with_capacity(4 + encodings.len() * 4);
    msg.extend_from_slice(&[2, 0]);
    msg.extend_from_slice(&count.to_be_bytes());
    for encoding in encodings {
        msg.extend_from_slice(&encoding.to_be_bytes());
    }
    stream.write_all(&msg).await
}

pub(super) fn requested_pixel_format() -> PixelFormat {
    PixelFormat {
        bits_per_pixel: 32,
        depth: 24,
        big_endian: false,
        red_max: 255,
        green_max: 255,
        blue_max: 255,
        red_shift: 16,
        green_shift: 8,
        blue_shift: 0,
    }
}

pub(super) async fn write_framebuffer_request(
    stream: &mut OwnedWriteHalf,
    incremental: bool,
    width: u32,
    height: u32,
) -> io::Result<()> {
    let mut msg = [0u8; 10];
    msg[0] = 3;
    msg[1] = incremental as u8;
    msg[6..8].copy_from_slice(&(width as u16).to_be_bytes());
    msg[8..10].copy_from_slice(&(height as u16).to_be_bytes());
    stream.write_all(&msg).await
}

pub(super) async fn read_frames(
    stream: OwnedReadHalf,
    frame_tx: removent_core::latest::Sender<DecodedFrame>,
    signal_tx: mpsc::Sender<FrameSignal>,
    dimensions: Arc<RwLock<FrameSize>>,
    format: PixelFormat,
    stats: Arc<VncStats>,
) {
    let mut stream = BufReader::with_capacity(
        256 * 1024,
        CountingReader {
            stream,
            stats: stats.clone(),
        },
    );
    let initial = *dimensions.read().await;
    let mut framebuffer = vec![0u8; initial.width as usize * initial.height as usize * 4];
    let started = Instant::now();
    loop {
        let mut kind = [0u8; 1];
        if let Err(error) = stream.read_exact(&mut kind).await {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                tracing::info!("VNC server closed the stream");
            } else {
                tracing::warn!(%error, "VNC stream read failed");
            }
            break;
        }
        let update_started = Instant::now();
        let mut decode_time = Duration::ZERO;
        let result = match kind[0] {
            0 => {
                read_update(
                    &mut stream,
                    &mut framebuffer,
                    &dimensions,
                    format,
                    &mut decode_time,
                )
                .await
            }
            1 => read_color_map(&mut stream).await.map(|_| false),
            2 => Ok(false), // Bell
            3 => read_cut_text(&mut stream).await.map(|_| false),
            150 => Ok(false), // EndOfContinuousUpdates pseudo-message
            248 => read_fence(&mut stream).await.map(|_| false),
            250 => read_xvp(&mut stream).await.map(|_| false),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported VNC server message",
            )),
        };
        let changed = match result {
            Ok(changed) => changed,
            Err(error) => {
                tracing::warn!(%error, message_type = kind[0], "VNC server message failed");
                break;
            }
        };
        if kind[0] == 0 {
            let size = *dimensions.read().await;
            if size.width == 0 || size.height == 0 {
                continue;
            }
            if changed {
                let copy_started = Instant::now();
                let frame = DecodedFrame {
                    data: framebuffer.clone(),
                    width: size.width,
                    height: size.height,
                    pts_us: started.elapsed().as_micros() as i64,
                };
                decode_time += copy_started.elapsed();
                stats
                    .decode_us
                    .store(decode_time.as_micros() as u64, Ordering::Relaxed);
                stats.update_us.store(
                    update_started.elapsed().as_micros() as u64,
                    Ordering::Relaxed,
                );
                stats.received_frames.fetch_add(1, Ordering::Relaxed);
                if frame_tx.send(frame).is_err() {
                    break;
                }
            }
            if signal_tx.send(FrameSignal::Updated).await.is_err() {
                break;
            }
        }
    }
    let _ = signal_tx.send(FrameSignal::Closed).await;
}

struct CountingReader {
    stream: OwnedReadHalf,
    stats: Arc<VncStats>,
}

impl AsyncRead for CountingReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let result = Pin::new(&mut self.stream).poll_read(cx, buf);
        self.stats
            .received_bytes
            .fetch_add((buf.filled().len() - before) as u64, Ordering::Relaxed);
        result
    }
}

async fn read_color_map(stream: &mut (impl AsyncRead + Unpin)) -> io::Result<()> {
    // padding(1), first colour(2), number of colours(2), then six bytes/RGB
    // entry. We request true-colour pixels, but consuming this message keeps
    // compatibility with servers that still send a colour map notification.
    let mut header = [0u8; 5];
    stream.read_exact(&mut header).await?;
    let count = u16::from_be_bytes([header[3], header[4]]) as usize;
    let len = count
        .checked_mul(6)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "VNC colour map is too large"))?;
    if len > MAX_NAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "VNC colour map is too large",
        ));
    }
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await.map(|_| ())
}

async fn read_fence(stream: &mut (impl AsyncRead + Unpin)) -> io::Result<()> {
    // padding(3), flags(4), length(1), payload.
    let mut header = [0u8; 8];
    stream.read_exact(&mut header).await?;
    let len = header[7] as usize;
    if len > MAX_NAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "VNC fence payload is too large",
        ));
    }
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await.map(|_| ())
}

async fn read_xvp(stream: &mut (impl AsyncRead + Unpin)) -> io::Result<()> {
    // XVP version, message, and one byte of padding.
    let mut payload = [0u8; 3];
    stream.read_exact(&mut payload).await.map(|_| ())
}

async fn read_update(
    stream: &mut (impl AsyncRead + Unpin),
    framebuffer: &mut Vec<u8>,
    dimensions: &Arc<RwLock<FrameSize>>,
    format: PixelFormat,
    decode_time: &mut Duration,
) -> io::Result<bool> {
    let mut head = [0u8; 3];
    stream.read_exact(&mut head).await?;
    let count = u16::from_be_bytes([head[1], head[2]]) as usize;
    for rectangle in 0..count {
        if rectangle % 32 == 31 {
            tokio::task::yield_now().await;
        }
        let mut rect = [0u8; 12];
        stream.read_exact(&mut rect).await?;
        let x = u16::from_be_bytes([rect[0], rect[1]]) as u32;
        let y = u16::from_be_bytes([rect[2], rect[3]]) as u32;
        let w = u16::from_be_bytes([rect[4], rect[5]]) as u32;
        let h = u16::from_be_bytes([rect[6], rect[7]]) as u32;
        let encoding = i32::from_be_bytes([rect[8], rect[9], rect[10], rect[11]]);
        if encoding == -223 {
            // Standard DesktopSize pseudo-encoding. Apple may use this after
            // Session Select to announce the real display dimensions.
            if w == 0 || h == 0 || w as usize * h as usize > MAX_PIXELS {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid VNC desktop dimensions",
                ));
            }
            let old = *dimensions.read().await;
            resize_framebuffer(
                framebuffer,
                old,
                FrameSize {
                    width: w,
                    height: h,
                    authoritative: true,
                },
            );
            *dimensions.write().await = FrameSize {
                width: w,
                height: h,
                authoritative: true,
            };
            continue;
        }
        if encoding == -308 {
            // ExtendedDesktopSize: number-of-screens(1), padding(3), then
            // sixteen bytes per screen. The rectangle header already carries
            // the aggregate desktop dimensions.
            let mut header = [0u8; 4];
            stream.read_exact(&mut header).await?;
            let screens = header[0] as usize;
            let payload_len = screens.checked_mul(16).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "VNC screen list is too large")
            })?;
            if payload_len > MAX_NAME {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "VNC screen list is too large",
                ));
            }
            let mut payload = vec![0u8; payload_len];
            stream.read_exact(&mut payload).await?;
            if w == 0 || h == 0 || w as usize * h as usize > MAX_PIXELS {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid VNC desktop dimensions",
                ));
            }
            let old = *dimensions.read().await;
            let new = FrameSize {
                width: w,
                height: h,
                authoritative: true,
            };
            resize_framebuffer(framebuffer, old, new);
            *dimensions.write().await = new;
            continue;
        }
        if encoding == -224 {
            // LastRect carries no payload and simply terminates this update.
            break;
        }
        if encoding == 1101 {
            // Apple DisplayInfo: aggregate width/height, display count, flags,
            // followed by 28 bytes per display. The metadata is optional, so
            // malformed/truncated records are rejected rather than desyncing
            // the stream.
            let mut header = [0u8; 8];
            stream.read_exact(&mut header).await?;
            let display_width = u16::from_be_bytes([header[0], header[1]]) as u32;
            let display_height = u16::from_be_bytes([header[2], header[3]]) as u32;
            let count = u16::from_be_bytes([header[4], header[5]]) as usize;
            let payload_len = count.checked_mul(28).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Apple display list is too large",
                )
            })?;
            if payload_len > MAX_NAME {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Apple display list is too large",
                ));
            }
            let mut displays = vec![0u8; payload_len];
            stream.read_exact(&mut displays).await?;
            if display_width > 0
                && display_height > 0
                && display_width as usize * display_height as usize <= MAX_PIXELS
            {
                let old = *dimensions.read().await;
                let new = FrameSize {
                    width: display_width,
                    height: display_height,
                    authoritative: true,
                };
                resize_framebuffer(framebuffer, old, new);
                *dimensions.write().await = new;
            }
            continue;
        }
        if encoding == 1105 {
            // DisplayInfo2 is a length-prefixed metadata blob. The rectangle's
            // dimensions identify the active desktop in Apple's implementation.
            let payload_len = stream.read_u16().await? as usize;
            if payload_len > MAX_NAME {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Apple display info payload too large",
                ));
            }
            let mut payload = vec![0u8; payload_len];
            stream.read_exact(&mut payload).await?;
            if w > 0 && h > 0 && w as usize * h as usize <= MAX_PIXELS {
                let old = *dimensions.read().await;
                let new = FrameSize {
                    width: w,
                    height: h,
                    authoritative: true,
                };
                resize_framebuffer(framebuffer, old, new);
                *dimensions.write().await = new;
            }
            continue;
        }
        let mut size = *dimensions.read().await;
        if (size.width == 0 || size.height == 0) && encoding == 0 {
            let new_width = x.saturating_add(w);
            let new_height = y.saturating_add(h);
            if new_width == 0
                || new_height == 0
                || new_width as usize * new_height as usize > MAX_PIXELS
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid VNC framebuffer dimensions",
                ));
            }
            size = FrameSize {
                width: new_width,
                height: new_height,
                authoritative: false,
            };
            resize_framebuffer(framebuffer, *dimensions.read().await, size);
            *dimensions.write().await = size;
        }
        if encoding == 0
            && !size.authoritative
            && (x.saturating_add(w) > size.width || y.saturating_add(h) > size.height)
        {
            let expanded = FrameSize {
                width: size.width.max(x.saturating_add(w)),
                height: size.height.max(y.saturating_add(h)),
                authoritative: false,
            };
            if expanded.width == 0
                || expanded.height == 0
                || expanded.width as usize * expanded.height as usize > MAX_PIXELS
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "VNC rectangle exceeds framebuffer limit",
                ));
            }
            resize_framebuffer(framebuffer, size, expanded);
            *dimensions.write().await = expanded;
            size = expanded;
        }
        let width = size.width;
        let height = size.height;
        if x.saturating_add(w) > width || y.saturating_add(h) > height {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "VNC rectangle outside framebuffer",
            ));
        }
        if encoding != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "VNC server did not use raw encoding",
            ));
        }
        let bytes_per_pixel = (format.bits_per_pixel / 8) as usize;
        if !matches!(bytes_per_pixel, 2 | 4) || format.depth == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported VNC pixel format",
            ));
        }
        // The requested wire format already matches BGRA apart from padding.
        // Read directly into the framebuffer and only normalize alpha. This
        // avoids a rectangle-sized allocation and three divisions per pixel.
        let native_bgra = format.bits_per_pixel == 32
            && !format.big_endian
            && format.red_max == 255
            && format.green_max == 255
            && format.blue_max == 255
            && format.red_shift == 16
            && format.green_shift == 8
            && format.blue_shift == 0;
        if native_bgra {
            for row in 0..h as usize {
                let offset = ((y as usize + row) * width as usize + x as usize) * 4;
                let dst = &mut framebuffer[offset..offset + w as usize * 4];
                stream.read_exact(dst).await?;
                let conversion = Instant::now();
                for pixel in dst.as_chunks_mut::<4>().0 {
                    pixel[3] = 255;
                }
                *decode_time += conversion.elapsed();
                if row % 32 == 31 {
                    tokio::task::yield_now().await;
                }
            }
            continue;
        }
        let mut raw = vec![0u8; w as usize * bytes_per_pixel];
        for row in 0..h as usize {
            stream.read_exact(&mut raw).await?;
            let conversion = Instant::now();
            for col in 0..w as usize {
                let src = col * bytes_per_pixel;
                let value = if format.big_endian {
                    if bytes_per_pixel == 4 {
                        u32::from_be_bytes(raw[src..src + 4].try_into().unwrap()) as u64
                    } else {
                        u16::from_be_bytes(raw[src..src + 2].try_into().unwrap()) as u64
                    }
                } else if bytes_per_pixel == 4 {
                    u32::from_le_bytes(raw[src..src + 4].try_into().unwrap()) as u64
                } else {
                    u16::from_le_bytes(raw[src..src + 2].try_into().unwrap()) as u64
                };
                let red = scale_component(
                    (value >> format.red_shift) as u32 & u32::from(format.red_max),
                    format.red_max,
                );
                let green = scale_component(
                    (value >> format.green_shift) as u32 & u32::from(format.green_max),
                    format.green_max,
                );
                let blue = scale_component(
                    (value >> format.blue_shift) as u32 & u32::from(format.blue_max),
                    format.blue_max,
                );
                let dst = (((y as usize + row) * width as usize) + x as usize + col) * 4;
                framebuffer[dst..dst + 4].copy_from_slice(&[blue, green, red, 255]);
            }
            *decode_time += conversion.elapsed();
            if row % 32 == 31 {
                tokio::task::yield_now().await;
            }
        }
    }
    Ok(count > 0)
}

fn resize_framebuffer(framebuffer: &mut Vec<u8>, old: FrameSize, new: FrameSize) {
    if old == new {
        if framebuffer.len() != new.width as usize * new.height as usize * 4 {
            framebuffer.resize(new.width as usize * new.height as usize * 4, 0);
        }
        return;
    }
    let mut resized = vec![0u8; new.width as usize * new.height as usize * 4];
    let copy_width = old.width.min(new.width) as usize;
    let copy_height = old.height.min(new.height) as usize;
    for row in 0..copy_height {
        let old_start = row * old.width as usize * 4;
        let new_start = row * new.width as usize * 4;
        let len = copy_width * 4;
        if old_start + len <= framebuffer.len() {
            resized[new_start..new_start + len]
                .copy_from_slice(&framebuffer[old_start..old_start + len]);
        }
    }
    *framebuffer = resized;
}

pub(super) fn scale_component(value: u32, max: u16) -> u8 {
    if max == 0 {
        0
    } else {
        ((value * 255 + u32::from(max) / 2) / u32::from(max)).min(255) as u8
    }
}

async fn read_cut_text(stream: &mut (impl AsyncRead + Unpin)) -> io::Result<()> {
    let mut head = [0u8; 7];
    stream.read_exact(&mut head).await?;
    let len = u32::from_be_bytes([head[3], head[4], head[5], head[6]]) as usize;
    if len > MAX_NAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "VNC cut text too large",
        ));
    }
    let mut text = vec![0u8; len];
    stream.read_exact(&mut text).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rectangle(x: u16, y: u16, width: u16, height: u16, pixels: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0, 0, 1];
        for value in [x, y, width, height] {
            bytes.extend_from_slice(&value.to_be_bytes());
        }
        bytes.extend_from_slice(&0i32.to_be_bytes());
        bytes.extend_from_slice(pixels);
        bytes
    }

    #[tokio::test]
    async fn direct_bgra_reads_preserve_other_rows_and_normalize_padding() {
        let dimensions = Arc::new(RwLock::new(FrameSize {
            width: 4,
            height: 3,
            authoritative: true,
        }));
        let mut pixels = vec![42; 4 * 3 * 4];
        let mut decode_time = Duration::ZERO;
        let bytes = rectangle(
            1,
            1,
            2,
            2,
            &[1, 2, 3, 0, 4, 5, 6, 12, 7, 8, 9, 0, 10, 11, 12, 0],
        );
        assert!(
            read_update(
                &mut bytes.as_slice(),
                &mut pixels,
                &dimensions,
                requested_pixel_format(),
                &mut decode_time
            )
            .await
            .unwrap()
        );
        let mut expected = vec![42; 4 * 3 * 4];
        expected[20..28].copy_from_slice(&[1, 2, 3, 255, 4, 5, 6, 255]);
        expected[36..44].copy_from_slice(&[7, 8, 9, 255, 10, 11, 12, 255]);
        assert_eq!(pixels, expected);
        assert!(
            !read_update(
                &mut [0, 0, 0].as_slice(),
                &mut pixels,
                &dimensions,
                requested_pixel_format(),
                &mut decode_time
            )
            .await
            .unwrap()
        );
        assert_eq!(
            pixels, expected,
            "an empty update must not change or republish the picture"
        );
    }

    #[tokio::test]
    async fn fully_buffered_large_update_yields_for_input() {
        let dimensions = Arc::new(RwLock::new(FrameSize {
            width: 3840,
            height: 2160,
            authoritative: true,
        }));
        let mut pixels = vec![0; 3840 * 2160 * 4];
        let mut decode_time = Duration::ZERO;
        let bytes = rectangle(0, 0, 3840, 2160, &pixels);
        let input_ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let marker = input_ran.clone();
        let input = tokio::spawn(async move {
            marker.store(true, Ordering::Relaxed);
        });
        read_update(
            &mut bytes.as_slice(),
            &mut pixels,
            &dimensions,
            requested_pixel_format(),
            &mut decode_time,
        )
        .await
        .unwrap();
        assert!(
            input_ran.load(Ordering::Relaxed),
            "pixel processing must yield even with all bytes already buffered"
        );
        input.await.unwrap();
        assert!(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| *pixel == [0, 0, 0, 255])
        );
    }
}
