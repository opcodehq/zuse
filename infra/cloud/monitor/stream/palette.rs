//! Exact indexed-color recovery frames for desktops with at most 256 colors.
//! Return zero as soon as a picture needs the video encoder.
use std::slice;

fn index(
    pixels: &[u32],
    stride: usize,
    width: usize,
    palette: &mut [u32; 256],
    indices: &mut [u8],
) -> usize {
    let mut table = [u16::MAX; 1024];
    let mut colors = 0;
    for (row, dest) in pixels
        .chunks_exact(stride)
        .zip(indices.chunks_exact_mut(width))
    {
        for (&pixel, slot) in row.iter().zip(dest) {
            let color = pixel & 0xffffff;
            let mut h = (color.wrapping_mul(2654435761) >> 22) as usize;
            loop {
                let id = table[h];
                if id == u16::MAX {
                    if colors == 256 {
                        return 0;
                    }
                    palette[colors] = color;
                    table[h] = colors as u16;
                    *slot = colors as u8;
                    colors += 1;
                    break;
                }
                if palette[id as usize] == color {
                    *slot = id as u8;
                    break;
                }
                h = (h + 1) & 1023;
            }
        }
    }
    for color in &mut palette[..colors] {
        *color = u32::from_le_bytes([(*color >> 16) as u8, (*color >> 8) as u8, *color as u8, 255])
            .to_le();
    }
    colors
}

// The XImage owns stride_bytes * height readable bytes. The capture worker
// allocates 256 palette entries and width * height index bytes per session.
#[no_mangle]
pub unsafe extern "C" fn poof_palette(
    pixels: *const u32,
    stride_bytes: usize,
    width: usize,
    height: usize,
    palette: &mut [u32; 256],
    indices: *mut u8,
) -> usize {
    let stride = stride_bytes / 4;
    index(
        slice::from_raw_parts(pixels, stride * height),
        stride,
        width,
        palette,
        slice::from_raw_parts_mut(indices, width * height),
    )
}

/// Pack repeated palette-index rows into a little-endian row map followed by
/// distinct rows. A fixed output budget bounds the dictionary for noisy images.
#[no_mangle]
pub unsafe extern "C" fn poof_fold_rows(
    indices: *const u8,
    width: usize,
    height: usize,
    output: *mut u8,
    capacity: usize,
) -> usize {
    use std::collections::HashMap;
    if capacity < height * 2 || width == 0 || height > u16::MAX as usize {
        return 0;
    }
    let rows = slice::from_raw_parts(indices, width * height);
    let output = slice::from_raw_parts_mut(output, capacity);
    let (refs, pixels) = output.split_at_mut(height * 2);
    let mut seen: HashMap<&[u8], u16> = HashMap::new();
    let mut used = 0;
    for (row, reference) in rows.chunks_exact(width).zip(refs.chunks_exact_mut(2)) {
        let next = seen.len() as u16;
        let id = seen.entry(row).or_insert(next);
        if *id == next {
            let Some(dest) = pixels.get_mut(used..used + width) else {
                return 0;
            };
            dest.copy_from_slice(row);
            used += width;
        }
        reference.copy_from_slice(&id.to_le_bytes());
    }
    if used + refs.len() < rows.len() {
        used + refs.len()
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn indexes_visible_pixels_only() {
        let mut palette = [0; 256];
        let mut out = [0; 4];
        assert_eq!(
            index(
                &[0xff123456, 0, 42, 0, 0x123456, 99],
                3,
                2,
                &mut palette,
                &mut out
            ),
            2
        );
        assert_eq!(out, [0, 1, 1, 0]);
        assert_eq!(palette[0].to_ne_bytes(), [0x12, 0x34, 0x56, 255]);
        let pixels: Vec<_> = (0..257).collect();
        assert_eq!(index(&pixels, 257, 257, &mut palette, &mut [0; 257]), 0);
    }

    #[test]
    fn repeated_rows_reconstruct_exactly_with_a_bounded_dictionary() {
        let input = [
            1, 2, 3, 4, 5, 6, 7, 8, 1, 2, 3, 4, 5, 6, 7, 8, 1, 2, 3, 4, 5, 6, 7, 8,
        ];
        let mut packet = [0; 24];
        let encode = |packet: &mut [u8]| unsafe {
            poof_fold_rows(input.as_ptr(), 4, 6, packet.as_mut_ptr(), packet.len())
        };
        assert_eq!(encode(&mut packet), 20);
        for (row, reference) in input.chunks_exact(4).zip(packet[..12].chunks_exact(2)) {
            let pos = 12 + usize::from(u16::from_le_bytes([reference[0], reference[1]])) * 4;
            assert_eq!(row, &packet[pos..pos + 4]);
        }
        assert_eq!(encode(&mut packet[..19]), 0);
    }
}
