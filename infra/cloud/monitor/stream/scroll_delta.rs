//! Pixel deltas for the 1920×1080 X11 capture worker. Only the estimator is
//! approximate: every accepted translation is corrected by exact row diffs.
use std::slice;

const WIDTH: usize = 1920;
const HEIGHT: usize = 1080;

fn score(now: &[u32], previous: &[u32], stride: usize, shift: i16) -> usize {
    let mut matches = 0;
    for y in (48..HEIGHT - 48).step_by(31) {
        let a = y * stride;
        let b = (y as isize + shift as isize) as usize * stride;
        for x in (32..WIDTH - 16).step_by(127) {
            matches += usize::from(now[a + x..a + x + 8] == previous[b + x..b + x + 8]);
        }
    }
    matches
}

fn patch(
    now: &[u32],
    previous: &[u32],
    stride: usize,
    last_shift: &mut i16,
    out: &mut [u8],
) -> Option<usize> {
    let samples = (48..HEIGHT - 48).step_by(31).count() * (32..WIDTH - 16).step_by(127).count();
    let mut best = 0;
    let mut shift = 0;
    for candidate in [0, *last_shift]
        .into_iter()
        .chain((1..=32).flat_map(|n| [n, -n]))
    {
        let matches = score(now, previous, stride, candidate);
        if matches > best {
            best = matches;
            shift = candidate;
        }
        if best * 100 >= samples * 97 {
            break;
        }
    }
    if best * 4 < samples * 3 {
        return None;
    }
    *last_shift = shift;
    out[..2].copy_from_slice(&shift.to_le_bytes());
    let mut pos = 2;
    for y in 0..HEIGHT {
        let row = &now[y * stride..y * stride + WIDTH];
        let sy = y as isize + shift as isize;
        let (left, right) = if (0..HEIGHT as isize).contains(&sy) {
            let base = sy as usize * stride;
            let other = &previous[base..base + WIDTH];
            if row == other {
                continue;
            }
            let mut left = 0;
            while row[left] == other[left] {
                left += 1;
            }
            let mut right = WIDTH;
            while row[right - 1] == other[right - 1] {
                right -= 1;
            }
            (left, right)
        } else {
            (0, WIDTH)
        };
        let size = (right - left) * 4;
        let span = out.get_mut(pos..pos + 6 + size)?;
        for (dest, val) in span[..6].chunks_exact_mut(2).zip([left, y, right - left]) {
            dest.copy_from_slice(&(val as u16).to_le_bytes());
        }
        for (dest, px) in span[6..].chunks_exact_mut(4).zip(&row[left..right]) {
            dest.copy_from_slice(&[(px >> 16) as u8, (px >> 8) as u8, *px as u8, 255]);
        }
        pos += 6 + size;
    }
    Some(pos)
}

// The caller owns two aligned, independent XImages of stride_bytes * HEIGHT
// bytes, a writable output buffer of capacity bytes and an initialized shift.
#[no_mangle]
pub unsafe extern "C" fn zuse_patch(
    now: *const u32,
    previous: *const u32,
    stride_bytes: usize,
    last_shift: &mut i16,
    out: *mut u8,
    capacity: usize,
) -> usize {
    let stride = stride_bytes / 4;
    if stride < WIDTH || capacity < 2 {
        return 0;
    }
    patch(
        slice::from_raw_parts(now, stride * HEIGHT),
        slice::from_raw_parts(previous, stride * HEIGHT),
        stride,
        last_shift,
        slice::from_raw_parts_mut(out, capacity),
    )
    .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconstructs_scrolled_pixels_and_separate_edits() {
        let old: Vec<u32> = (0..WIDTH * HEIGHT)
            .map(|i| (i as u32).wrapping_mul(2654435761) & 0xffffff)
            .collect();
        for shift in [-6i16, 0, 12] {
            let mut new = old.clone();
            for (y, row) in new.chunks_exact_mut(WIDTH).enumerate() {
                let source = y as isize + shift as isize;
                if (0..HEIGHT as isize).contains(&source) {
                    let i = source as usize * WIDTH;
                    row.copy_from_slice(&old[i..i + WIDTH]);
                }
            }
            new[42] ^= 0xaabbcc;
            new[WIDTH * (HEIGHT - 1)] ^= 0x112233;
            let mut out = vec![0; 1024 * 1024];
            let len = patch(&new, &old, WIDTH, &mut 0, &mut out).unwrap();
            assert_eq!(i16::from_le_bytes([out[0], out[1]]), shift);
            let mut result = old.clone();
            let delta = usize::from(shift.unsigned_abs()) * WIDTH;
            let end = result.len();
            if shift >= 0 {
                result.copy_within(delta.., 0);
            } else {
                result.copy_within(..end - delta, delta);
            }
            let mut pos = 2;
            while pos < len {
                let word = |i| u16::from_le_bytes([out[i], out[i + 1]]) as usize;
                let start = word(pos + 2) * WIDTH + word(pos);
                let span = word(pos + 4);
                pos += 6;
                for px in &mut result[start..start + span] {
                    *px = u32::from_be_bytes([0, out[pos], out[pos + 1], out[pos + 2]]);
                    pos += 4;
                }
            }
            assert_eq!(new, result);
            assert!(patch(&new, &old, WIDTH, &mut 0, &mut [0; 8]).is_none());
        }
    }
}
