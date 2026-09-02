use super::{Frame, PixelFormat};

pub(super) fn send_frame_message(
    out: &mut Vec<u8>,
    frame: &Frame,
    x: u16,
    y: u16,
    req_w: u16,
    req_h: u16,
    format: PixelFormat,
) {
    let expected_len = frame.width as usize * frame.height as usize * 4;
    if frame.data.len() < expected_len {
        return;
    }
    let x = u32::from(x).min(frame.width);
    let y = u32::from(y).min(frame.height);
    let w = u32::from(req_w).min(frame.width.saturating_sub(x));
    let h = u32::from(req_h).min(frame.height.saturating_sub(y));
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&(x as u16).to_be_bytes());
    out.extend_from_slice(&(y as u16).to_be_bytes());
    out.extend_from_slice(&(w as u16).to_be_bytes());
    out.extend_from_slice(&(h as u16).to_be_bytes());
    out.extend_from_slice(&0i32.to_be_bytes());
    let bytes_per_pixel = usize::from(format.bits_per_pixel / 8);
    for row in 0..h as usize {
        let start = ((y as usize + row) * frame.width as usize + x as usize) * 4;
        for pixel in frame.data[start..start + w as usize * 4].as_chunks::<4>().0 {
            let b = u32::from(pixel[0]);
            let g = u32::from(pixel[1]);
            let r = u32::from(pixel[2]);
            let value = ((r * u32::from(format.red_max) + 127) / 255)
                .min(u32::from(format.red_max))
                << format.red_shift
                | (((g * u32::from(format.green_max) + 127) / 255)
                    .min(u32::from(format.green_max))
                    << format.green_shift)
                | (((b * u32::from(format.blue_max) + 127) / 255).min(u32::from(format.blue_max))
                    << format.blue_shift);
            if bytes_per_pixel == 4 {
                let bytes = if format.big_endian {
                    value.to_be_bytes()
                } else {
                    value.to_le_bytes()
                };
                out.extend_from_slice(&bytes);
            } else if format.big_endian {
                out.extend_from_slice(&(value as u16).to_be_bytes());
            } else {
                out.extend_from_slice(&(value as u16).to_le_bytes());
            }
        }
    }
}
