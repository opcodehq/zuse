//! Native RGBA framebuffer used by the pixel-marker client. Every patch is
//! applied to the whole desktop before the caller can inspect decoded pixels.
use std::slice;

fn word(packet: &mut &[u8]) -> Option<u16> {
    let (bytes, rest) = packet.split_first_chunk::<2>()?;
    *packet = rest;
    Some(u16::from_le_bytes(*bytes))
}

fn apply(frame: &mut [u8], width: usize, height: usize, mut packet: &[u8]) -> Option<()> {
    let mut shift = word(&mut packet)? as i16;
    let (mut left, mut top, mut w, mut h) = (0, 0, width, height);
    if shift == i16::MIN {
        shift = word(&mut packet)? as i16;
        left = word(&mut packet)? as usize;
        top = word(&mut packet)? as usize;
        w = word(&mut packet)? as usize;
        h = word(&mut packet)? as usize;
    }
    let step = usize::from(shift.unsigned_abs());
    if step > 32 || w == 0 || h == 0 || left + w > width || top + h > height {
        return None;
    }
    if step > 0 && step < h {
        for row in 0..h - step {
            let (src, dst) = if shift > 0 {
                (row + step, row)
            } else {
                (h - row - step - 1, h - row - 1)
            };
            let start = ((top + src) * width + left) * 4;
            let dest = ((top + dst) * width + left) * 4;
            frame.copy_within(start..start + w * 4, dest);
        }
    }
    while !packet.is_empty() {
        let x = word(&mut packet)? as usize;
        let y = word(&mut packet)? as usize;
        let n = word(&mut packet)? as usize;
        if n == 0 || y >= height || x + n > width {
            return None;
        }
        let (rgba, rest) = packet.split_at_checked(n * 4)?;
        let pos = (y * width + x) * 4;
        frame[pos..pos + rgba.len()].copy_from_slice(rgba);
        packet = rest;
    }
    Some(())
}

/// The caller supplies a writable width × height RGBA buffer and a disjoint
/// decompressed patch. Discard the framebuffer on error.
#[no_mangle]
pub unsafe extern "C" fn poof_apply_patch(
    frame: *mut u8,
    width: usize,
    height: usize,
    packet: *const u8,
    size: usize,
) -> bool {
    if frame.is_null()
        || packet.is_null()
        || !matches!((width, height), (1920, 1080) | (2560, 1600))
        || size > 1024 * 1024
    {
        return false;
    }
    apply(
        slice::from_raw_parts_mut(frame, width * height * 4),
        width,
        height,
        slice::from_raw_parts(packet, size),
    )
    .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn moves_only_the_selected_columns_in_both_directions() {
        let before: Vec<u8> = (0..80).collect();
        for shift in [-1_i16, 1] {
            let mut frame = before.clone();
            let bytes: Vec<_> = [i16::MIN, shift, 1, 1, 2, 4]
                .into_iter()
                .flat_map(i16::to_le_bytes)
                .collect();
            apply(&mut frame, 4, 5, &bytes).unwrap();
            let row = 3_usize;
            let source = (row as isize + shift as isize) as usize;
            assert_eq!(
                &frame[row * 16 + 4..row * 16 + 12],
                &before[source * 16 + 4..source * 16 + 12]
            );
            assert_eq!(
                &frame[row * 16..row * 16 + 4],
                &before[row * 16..row * 16 + 4]
            );
        }
        assert!(apply(&mut before.clone(), 4, 5, &[0, 0, 0, 0, 0, 0, 1, 0, 42]).is_none());
    }
}
