//! Scanline unfiltering (PNG §9) and the Adam7 interlacing geometry (§8.2).
//!
//! PNG never stores raw scanlines: each one is delta-coded against the pixel
//! to its left and/or the scanline above, by a filter chosen per row, so this
//! runs before any pixel means anything. Interlacing reorders which pixels
//! belong to which scanline in the first place — Adam7 splits an image into
//! seven passes, each its own tiny sub-image with its own filtering — so the
//! two are handled together here rather than as an afterthought bolted onto
//! [`super::decode`].

/// PNG's Paeth predictor: whichever of the left, upper and upper-left
/// neighbours is closest to `a + b - c`.
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (a, b, c) = (i32::from(a), i32::from(b), i32::from(c));
    let p = a + b - c;
    let pa = (p - a).abs();
    let pb = (p - b).abs();
    let pc = (p - c).abs();
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}

/// Undo the per-scanline filter. `data` must hold exactly
/// `(width * bpp + 1) * height` bytes — a filter-type byte followed by one
/// row, repeated — and `bpp` is whole bytes per pixel (channels, since this
/// decoder only reads 8-bit samples). Returns the pixel bytes with the
/// filter-type bytes removed.
pub(crate) fn unfilter(data: &[u8], width: usize, height: usize, bpp: usize) -> Option<Vec<u8>> {
    if width == 0 || height == 0 {
        return Some(Vec::new());
    }
    let stride = width.checked_mul(bpp)?;
    let row_bytes = stride.checked_add(1)?;
    let needed = row_bytes.checked_mul(height)?;
    let data = data.get(..needed)?;

    let mut out = vec![0u8; stride * height];
    for y in 0..height {
        let filter_type = data[y * row_bytes];
        for i in 0..stride {
            let raw = data[y * row_bytes + 1 + i];
            let left = if i >= bpp {
                out[y * stride + i - bpp]
            } else {
                0
            };
            let up = if y > 0 { out[(y - 1) * stride + i] } else { 0 };
            let up_left = if i >= bpp && y > 0 {
                out[(y - 1) * stride + i - bpp]
            } else {
                0
            };
            let recon = match filter_type {
                0 => raw,
                1 => raw.wrapping_add(left),
                2 => raw.wrapping_add(up),
                3 => raw.wrapping_add(((u16::from(left) + u16::from(up)) / 2) as u8),
                4 => raw.wrapping_add(paeth(left, up, up_left)),
                _ => return None, // Filter types are 0-4; anything else is a broken stream.
            };
            out[y * stride + i] = recon;
        }
    }
    Some(out)
}

/// One Adam7 pass: which pixel it starts at and how far apart its pixels are,
/// along each axis, in the full image's pixel grid.
pub(crate) struct Adam7Pass {
    pub x0: u32,
    pub y0: u32,
    pub dx: u32,
    pub dy: u32,
}

/// The seven passes in transmission order (PNG §8.2). Interlacing exists so a
/// slow connection shows a blurry whole image early rather than a sharp image
/// growing top to bottom; that history is why the passes interleave by
/// bit-reversed position rather than, say, splitting the image into stripes.
pub(crate) const ADAM7_PASSES: [Adam7Pass; 7] = [
    Adam7Pass {
        x0: 0,
        y0: 0,
        dx: 8,
        dy: 8,
    },
    Adam7Pass {
        x0: 4,
        y0: 0,
        dx: 8,
        dy: 8,
    },
    Adam7Pass {
        x0: 0,
        y0: 4,
        dx: 4,
        dy: 8,
    },
    Adam7Pass {
        x0: 2,
        y0: 0,
        dx: 4,
        dy: 4,
    },
    Adam7Pass {
        x0: 0,
        y0: 2,
        dx: 2,
        dy: 4,
    },
    Adam7Pass {
        x0: 1,
        y0: 0,
        dx: 2,
        dy: 2,
    },
    Adam7Pass {
        x0: 0,
        y0: 1,
        dx: 1,
        dy: 2,
    },
];

/// How many pixels of the full `width × height` image fall in this pass —
/// zero along an axis where the image is smaller than the pass's own start
/// offset, which is how a very small image legitimately has empty passes.
pub(crate) fn pass_dims(pass: &Adam7Pass, width: u32, height: u32) -> (u32, u32) {
    let w = if width > pass.x0 {
        (width - pass.x0).div_ceil(pass.dx)
    } else {
        0
    };
    let h = if height > pass.y0 {
        (height - pass.y0).div_ceil(pass.dy)
    } else {
        0
    };
    (w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paeth_prefers_the_closest_neighbour() {
        // The three reference cases from the PNG spec's worked example.
        assert_eq!(paeth(0, 0, 0), 0);
        assert_eq!(
            paeth(10, 0, 0),
            10,
            "left wins when it is the only nonzero one"
        );
        assert_eq!(paeth(0, 10, 0), 10, "and up");
        // A tie between left and up is broken in favour of left.
        assert_eq!(paeth(5, 5, 0), 5);
    }

    #[test]
    fn filter_type_zero_is_a_pass_through() {
        let mut data = vec![0u8]; // filter type 0
        data.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
        let out = unfilter(&data, 2, 1, 3).unwrap();
        assert_eq!(out, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn each_filter_type_round_trips_a_hand_filtered_row() {
        // Two 3-byte pixels over two rows, filtered by hand for each type,
        // then unfiltered and checked against the original bytes.
        let bpp = 3;
        let width = 2;
        let rows: [[u8; 6]; 2] = [[10, 20, 30, 200, 210, 220], [1, 2, 3, 250, 251, 252]];

        for filter_type in 0u8..=4 {
            let mut prev = [0u8; 6];
            let mut data = Vec::new();
            for row in &rows {
                data.push(filter_type);
                for i in 0..6 {
                    let left = if i >= bpp { row[i - bpp] } else { 0 };
                    let up = prev[i];
                    let up_left = if i >= bpp { prev[i - bpp] } else { 0 };
                    let filtered = match filter_type {
                        0 => row[i],
                        1 => row[i].wrapping_sub(left),
                        2 => row[i].wrapping_sub(up),
                        3 => row[i].wrapping_sub(((u16::from(left) + u16::from(up)) / 2) as u8),
                        4 => row[i].wrapping_sub(paeth(left, up, up_left)),
                        _ => unreachable!(),
                    };
                    data.push(filtered);
                }
                prev = *row;
            }
            let out = unfilter(&data, width, 2, bpp).unwrap();
            assert_eq!(out, rows.concat(), "filter type {filter_type}");
        }
    }

    #[test]
    fn an_unknown_filter_type_is_rejected_not_guessed_at() {
        let data = vec![5u8, 1, 2, 3];
        assert!(unfilter(&data, 1, 1, 3).is_none());
    }

    #[test]
    fn a_short_buffer_is_rejected_rather_than_read_out_of_bounds() {
        // Two 1-byte pixels need a filter byte plus 2 bytes; only 1 is here.
        let data = vec![0u8, 1];
        assert!(unfilter(&data, 2, 1, 1).is_none());
    }

    #[test]
    fn a_zero_sized_image_has_no_scanlines_to_unfilter() {
        assert_eq!(unfilter(&[], 0, 0, 4), Some(Vec::new()));
    }

    #[test]
    fn adam7_pass_sizes_match_the_classic_eight_by_eight_breakdown() {
        // The textbook example: an 8x8 image's seven passes are exactly
        // 1, 1, 2, 4, 8, 16, 32 pixels, in that order, summing to all 64.
        let expected = [(1, 1), (1, 1), (2, 1), (2, 2), (4, 2), (4, 4), (8, 4)];
        let mut total = 0u32;
        for (pass, &(w, h)) in ADAM7_PASSES.iter().zip(&expected) {
            let dims = pass_dims(pass, 8, 8);
            assert_eq!(dims, (w, h));
            total += dims.0 * dims.1;
        }
        assert_eq!(total, 64);
    }

    #[test]
    fn a_one_pixel_image_only_fills_the_first_pass() {
        let mut total = 0u32;
        for pass in &ADAM7_PASSES {
            let (w, h) = pass_dims(pass, 1, 1);
            total += w * h;
        }
        assert_eq!(total, 1);
        assert_eq!(pass_dims(&ADAM7_PASSES[0], 1, 1), (1, 1));
    }

    #[test]
    fn every_pass_over_a_range_of_sizes_accounts_for_every_pixel_exactly_once() {
        for width in 1u32..=20 {
            for height in 1u32..=20 {
                let total: u32 = ADAM7_PASSES
                    .iter()
                    .map(|pass| {
                        let (w, h) = pass_dims(pass, width, height);
                        w * h
                    })
                    .sum();
                assert_eq!(total, width * height, "at {width}x{height}");
            }
        }
    }
}
