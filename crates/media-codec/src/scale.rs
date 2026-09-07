//! Accelerate resampling of tightly packed BGRA frames.
use std::ffi::c_void;

#[repr(C)]
struct Buffer {
    data: *mut c_void,
    height: usize,
    width: usize,
    row_bytes: usize,
}

#[link(name = "Accelerate", kind = "framework")]
unsafe extern "C" {
    fn vImageScale_ARGB8888(
        src: *const Buffer,
        dst: *const Buffer,
        temp: *mut c_void,
        flags: u32,
    ) -> isize;
}

/// Channel-independent four-byte resampling works equally for BGRA and ARGB.
pub fn scale_bgra(
    input: &[u8],
    source: (usize, usize),
    target: (usize, usize),
) -> Result<Vec<u8>, String> {
    let size = |(w, h): (usize, usize)| w.checked_mul(h)?.checked_mul(4);
    if source.0 == 0
        || source.1 == 0
        || target.0 == 0
        || target.1 == 0
        || size(source) != Some(input.len())
    {
        return Err("invalid BGRA dimensions".into());
    }
    let len = size(target).ok_or("BGRA target size overflow")?;
    let mut output = vec![0; len];
    let src = Buffer {
        data: input.as_ptr().cast_mut().cast(),
        height: source.1,
        width: source.0,
        row_bytes: source.0 * 4,
    };
    let dst = Buffer {
        data: output.as_mut_ptr().cast(),
        height: target.1,
        width: target.0,
        row_bytes: target.0 * 4,
    };
    // SAFETY: dimensions/lengths were checked; input remains immutable and output
    // exclusively borrowed until this synchronous operation returns. flags=0 is
    // the low-latency resampling path; vImage allocates its own temporary storage.
    let result = unsafe { vImageScale_ARGB8888(&src, &dst, std::ptr::null_mut(), 0) };
    if result != 0 {
        return Err(format!("vImage scaling failed: {result}"));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scaling_preserves_constant_color_and_channel_order() {
        let input = [17, 63, 201, 255].repeat(32 * 24);
        for target in [(16, 12), (24, 18), (32, 24)] {
            let result = scale_bgra(&input, (32, 24), target).unwrap();
            assert_eq!(result, [17, 63, 201, 255].repeat(target.0 * target.1));
        }
        assert!(scale_bgra(&input[..10], (32, 24), (16, 12)).is_err());
    }
}
