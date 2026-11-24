//! Pixel deltas for the X11 capture worker. Only the estimator is
//! approximate: every accepted translation is corrected by exact row diffs.
use std::slice;
#[cfg(test)]
mod framebuffer;
mod palette;

fn score(now: &[u32], previous: &[u32], stride: usize, shift: i16, area: Rect) -> usize {
    let mut matches = 0;
    for y in (area.top + 32..area.bottom.saturating_sub(32)).step_by(31) {
        let a = y * stride;
        let b = (y as isize + shift as isize) as usize * stride;
        for x in (area.left..area.right.saturating_sub(7)).step_by(127) {
            matches += usize::from(now[a + x..a + x + 8] == previous[b + x..b + x + 8]);
        }
    }
    matches
}

#[derive(Clone, Copy)]
struct Rect {
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
}

fn changes<const W: usize, const H: usize>(a: &[u32], b: &[u32], stride: usize) -> Rect {
    let mut r = Rect {
        left: W,
        right: 0,
        top: H,
        bottom: 0,
    };
    for y in 0..H {
        let a = &a[y * stride..y * stride + W];
        let b = &b[y * stride..y * stride + W];
        if a == b {
            continue;
        }
        r.top = r.top.min(y);
        r.bottom = y + 1;
        // Only scan outside the bounding box already accounted for.
        if let Some(x) = a[..r.left]
            .iter()
            .zip(&b[..r.left])
            .position(|(x, y)| x != y)
        {
            r.left = x;
        }
        if let Some(x) = a[r.right..]
            .iter()
            .zip(&b[r.right..])
            .rposition(|(x, y)| x != y)
        {
            r.right += x + 1;
        }
    }
    r
}

fn write_span(out: &mut [u8], pos: usize, left: usize, y: usize, pixels: &[u32]) -> Option<usize> {
    let size = pixels.len() * 4;
    let span = out.get_mut(pos..pos + 6 + size)?;
    for (dest, val) in span[..6].chunks_exact_mut(2).zip([left, y, pixels.len()]) {
        dest.copy_from_slice(&(val as u16).to_le_bytes());
    }
    for (dest, px) in span[6..].chunks_exact_mut(4).zip(pixels) {
        dest.copy_from_slice(&[(px >> 16) as u8, (px >> 8) as u8, *px as u8, 255]);
    }
    Some(pos + 6 + size)
}

fn patch<const WIDTH: usize, const HEIGHT: usize>(
    now: &[u32],
    previous: &[u32],
    stride: usize,
    last_shift: &mut i16,
    out: &mut [u8],
    regional: bool,
) -> Option<usize> {
    let full = Rect {
        left: 0,
        top: 0,
        right: WIDTH,
        bottom: HEIGHT,
    };
    let r = if regional {
        changes::<WIDTH, HEIGHT>(now, previous, stride)
    } else {
        full
    };
    if r.bottom <= r.top {
        out[..2].fill(0);
        return Some(2);
    }
    let samples = (r.top + 32..r.bottom.saturating_sub(32))
        .step_by(31)
        .count()
        * (r.left..r.right.saturating_sub(7)).step_by(127).count();
    let mut best = 0;
    let mut shift = 0;
    if samples >= 4 {
        for candidate in [0, *last_shift]
            .into_iter()
            .chain((1..=32).flat_map(|n| [n, -n]))
        {
            let matches = score(now, previous, stride, candidate, r);
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
    }
    *last_shift = shift;
    out[..2].copy_from_slice(&shift.to_le_bytes());
    let mut pos = 2;
    if r.right - r.left < WIDTH || r.bottom - r.top < HEIGHT {
        // Escape followed by shift, x, y, width, height, all little endian.
        out[..2].copy_from_slice(&i16::MIN.to_le_bytes());
        for n in [
            shift as u16,
            r.left as u16,
            r.top as u16,
            (r.right - r.left) as u16,
            (r.bottom - r.top) as u16,
        ] {
            out.get_mut(pos..pos + 2)?.copy_from_slice(&n.to_le_bytes());
            pos += 2;
        }
    }
    for y in r.top..r.bottom {
        let row = &now[y * stride..y * stride + WIDTH];
        let sy = y as isize + shift as isize;
        let mut left = r.left;
        let mut right = r.right;
        if (r.top as isize..r.bottom as isize).contains(&sy) {
            let base = sy as usize * stride;
            let other = &previous[base..base + WIDTH];
            if row[left..right] == other[left..right] {
                continue;
            }
            while row[left] == other[left] {
                left += 1;
            }
            while row[right - 1] == other[right - 1] {
                right -= 1;
            }
            // Far-apart edits on one scanline can otherwise exhaust the patch
            // budget with unchanged pixels and force an expensive keyframe.
            if regional && right - left > 256 {
                let mut x = left;
                while x + 64 < right {
                    if row[x..x + 64] == other[x..x + 64] {
                        if left < x {
                            pos = write_span(out, pos, left, y, &row[left..x])?;
                        }
                        x += 64;
                        while x + 64 < right && row[x..x + 64] == other[x..x + 64] {
                            x += 64;
                        }
                        left = x;
                    } else {
                        x += 64;
                    }
                }
            }
        }
        pos = write_span(out, pos, left, y, &row[left..right])?;
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
    width: usize,
    height: usize,
    last_shift: &mut i16,
    out: *mut u8,
    capacity: usize,
    regional: bool,
) -> usize {
    let stride = stride_bytes / 4;
    if !matches!((width, height), (1920, 1080) | (2560, 1600)) || stride < width || capacity < 2 {
        return 0;
    }
    let now = slice::from_raw_parts(now, stride * height);
    let previous = slice::from_raw_parts(previous, stride * height);
    let out = slice::from_raw_parts_mut(out, capacity);
    match width {
        1920 => patch::<1920, 1080>(now, previous, stride, last_shift, out, regional),
        _ => patch::<2560, 1600>(now, previous, stride, last_shift, out, regional),
    }
    .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconstructs_scrolled_pixels_and_separate_edits() {
        reconstruct::<1920, 1080>();
        reconstruct::<2560, 1600>();
    }
    #[test]
    fn scrolls_a_window_on_a_textured_background() {
        const W: usize = 1920;
        const H: usize = 1080;
        let old: Vec<_> = (0..(W * H) as u32)
            .map(|i| i.wrapping_mul(2654435761) & 0xffffff)
            .collect();
        for shift in [-11_i16, 7] {
            let mut new = old.clone();
            for y in 120..540 {
                let from = (y as isize + shift as isize) as usize * W + 80;
                new[y * W + 80..y * W + 720].copy_from_slice(&old[from..from + 640]);
            }
            let mut out = vec![0; 32768];
            let n = patch::<W, H>(&new, &old, W, &mut 0, &mut out, true).unwrap();
            let mut words = out[..12]
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]));
            assert!(words.by_ref().eq([i16::MIN, shift, 80, 120, 640, 420]));
            assert_eq!(n, 12 + shift.unsigned_abs() as usize * (6 + 640 * 4));
        }
    }
    #[test]
    fn disjoint_edits_round_trip_without_transmitting_the_gap() {
        const W: usize = 1920;
        const H: usize = 1080;
        let old = vec![0x102030_u32; W * H];
        let mut new = old.clone();
        for y in 100..135 {
            new[y * W + 64..y * W + 80].fill(0x123456);
            new[y * W + 1500..y * W + 1510].fill(0xabcdef);
        }
        let mut packet = vec![0; 20_000];
        let len = patch::<W, H>(&new, &old, W, &mut 0, &mut packet, true).unwrap();
        let rgba = |color: u32| [(color >> 16) as u8, (color >> 8) as u8, color as u8, 255];
        let mut output = rgba(old[0]).repeat(W * H);
        assert!(unsafe {
            framebuffer::poof_apply_patch(output.as_mut_ptr(), W, H, packet.as_ptr(), len)
        });
        assert!(output
            .chunks_exact(4)
            .zip(new)
            .all(|(pixel, color)| pixel == rgba(color)));
    }
    fn reconstruct<const WIDTH: usize, const HEIGHT: usize>() {
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
            let len = patch::<WIDTH, HEIGHT>(&new, &old, WIDTH, &mut 0, &mut out, false).unwrap();
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
            assert!(
                patch::<WIDTH, HEIGHT>(&new, &old, WIDTH, &mut 0, &mut [0; 8], false).is_none()
            );
        }
    }
}
