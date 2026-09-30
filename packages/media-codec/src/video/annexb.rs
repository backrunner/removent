use super::*;

pub(super) const START_CODE: &[u8; 4] = &[0, 0, 0, 1];

/// AVCC (4-byte length-prefixed NALU sequence) → Annex-B.
pub fn avcc_to_annexb(data: &[u8]) -> Result<Vec<u8>, VideoError> {
    let mut out = Vec::with_capacity(data.len() + 16);
    let mut rest = data;
    while !rest.is_empty() {
        if rest.len() < 4 {
            return Err(VideoError::InvalidAvcc);
        }
        let nalu_len = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
        let total = 4 + nalu_len;
        if nalu_len == 0 || rest.len() < total {
            return Err(VideoError::InvalidAvcc);
        }
        out.extend_from_slice(START_CODE);
        out.extend_from_slice(&rest[4..total]);
        rest = &rest[total..];
    }
    Ok(out)
}

/// Splits Annex-B by start codes, returning NALUs without start codes
/// (trailing zero bytes stripped).
pub fn split_annexb_nals(data: &[u8]) -> Vec<&[u8]> {
    // Collect (NAL data start, start-code start). Consecutive zero bytes before
    // a start code belong to the separator.
    let mut marks: Vec<(usize, usize)> = Vec::new();
    let mut i = 0usize;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            let mut code_start = i;
            while code_start > 0 && data[code_start - 1] == 0 {
                code_start -= 1;
            }
            marks.push((i + 3, code_start));
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut nals = Vec::with_capacity(marks.len());
    for w in 0..marks.len() {
        let s = marks[w].0;
        let e = marks.get(w + 1).map(|m| m.1).unwrap_or(data.len());
        if e > s {
            nals.push(&data[s..e]);
        }
    }
    nals
}

pub(super) fn nal_type(nal: &[u8], hevc: bool) -> u8 {
    if nal.is_empty() {
        return 0;
    }
    if hevc {
        (nal[0] >> 1) & 0x3F
    } else {
        nal[0] & 0x1F
    }
}

pub(super) fn is_param_set(nal_type: u8, hevc: bool) -> bool {
    if hevc {
        matches!(nal_type, 32..=34) // VPS/SPS/PPS
    } else {
        matches!(nal_type, 7 | 8) // SPS/PPS
    }
}

pub(super) fn is_idr(nal_type: u8, hevc: bool) -> bool {
    if hevc {
        matches!(nal_type, 19 | 20)
    } else {
        nal_type == 5
    }
}

/// Extracts parameter-set NALUs from Annex-B (in order of appearance).
pub fn extract_param_sets(annexb: &[u8], hevc: bool) -> Vec<Vec<u8>> {
    split_annexb_nals(annexb)
        .into_iter()
        .filter(|n| is_param_set(nal_type(n, hevc), hevc))
        .map(|n| n.to_vec())
        .collect()
}

/// Returns whether the Annex-B data contains an IDR frame.
pub fn annexb_has_idr(annexb: &[u8], hevc: bool) -> bool {
    split_annexb_nals(annexb)
        .iter()
        .any(|n| is_idr(nal_type(n, hevc), hevc))
}
