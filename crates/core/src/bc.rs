//! The pixel formats the major-choice masks come in (RESEARCH 13k): BC1
//! (`PF_DXT1`), BC3 (`PF_DXT5`) and `PF_B8G8R8A8`, decoded to RGBA and
//! encoded back, and the one image operation the fix needs: squeezing a
//! 16:9 image into the centre of a wider frame with its edge columns
//! carried out to the sides.
//!
//! The encoder is a plain range fit: the block's extreme colours become the
//! endpoints and each pixel takes the nearest palette entry. For the soft
//! masks it is applied to, that is as good as the original encoding, and it
//! needs no search.

/// One RGBA pixel.
pub type Rgba = [u8; 4];

/// An image as RGBA pixels, row-major.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<Rgba>,
}

/// The pixel formats this module reads and writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Bc1,
    Bc3,
    Bgra8,
}

impl Format {
    pub fn from_name(name: &str) -> Option<Format> {
        match name {
            "PF_DXT1" => Some(Format::Bc1),
            "PF_DXT5" => Some(Format::Bc3),
            "PF_B8G8R8A8" => Some(Format::Bgra8),
            _ => None,
        }
    }

    /// Bytes a `width` x `height` mip takes.
    pub fn mip_size(self, width: usize, height: usize) -> usize {
        match self {
            Format::Bc1 => width.div_ceil(4) * height.div_ceil(4) * 8,
            Format::Bc3 => width.div_ceil(4) * height.div_ceil(4) * 16,
            Format::Bgra8 => width * height * 4,
        }
    }
}

fn rgb565(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 0x1F) as u32;
    let g = ((c >> 5) & 0x3F) as u32;
    let b = (c & 0x1F) as u32;
    [((r * 255 + 15) / 31) as u8, ((g * 255 + 31) / 63) as u8, ((b * 255 + 15) / 31) as u8]
}

fn to565(c: [u8; 3]) -> u16 {
    let r = (c[0] as u32 * 31 + 127) / 255;
    let g = (c[1] as u32 * 63 + 127) / 255;
    let b = (c[2] as u32 * 31 + 127) / 255;
    ((r << 11) | (g << 5) | b) as u16
}

fn lerp3(a: [u8; 3], b: [u8; 3], num: u32, den: u32) -> [u8; 3] {
    let mut o = [0u8; 3];
    for i in 0..3 {
        o[i] = ((a[i] as u32 * (den - num) + b[i] as u32 * num + den / 2) / den) as u8;
    }
    o
}

/// The colour palette of a BC1 block, for its two endpoints.
fn bc1_palette(c0: u16, c1: u16) -> [[u8; 4]; 4] {
    let a = rgb565(c0);
    let b = rgb565(c1);
    let opaque = |c: [u8; 3]| [c[0], c[1], c[2], 255];
    if c0 > c1 {
        [opaque(a), opaque(b), opaque(lerp3(a, b, 1, 3)), opaque(lerp3(a, b, 2, 3))]
    } else {
        [opaque(a), opaque(b), opaque(lerp3(a, b, 1, 2)), [0, 0, 0, 0]]
    }
}

/// Decode the 8-byte colour block shared by BC1 and BC3 into `out`
/// (16 pixels, row-major). BC3 treats the block as always four-colour.
fn decode_colour_block(d: &[u8], four_colour: bool, out: &mut [Rgba; 16]) {
    let c0 = u16::from_le_bytes([d[0], d[1]]);
    let c1 = u16::from_le_bytes([d[2], d[3]]);
    let palette = if four_colour && c0 <= c1 {
        let a = rgb565(c0);
        let b = rgb565(c1);
        let opaque = |c: [u8; 3]| [c[0], c[1], c[2], 255];
        [opaque(a), opaque(b), opaque(lerp3(a, b, 1, 3)), opaque(lerp3(a, b, 2, 3))]
    } else {
        bc1_palette(c0, c1)
    };
    let bits = u32::from_le_bytes([d[4], d[5], d[6], d[7]]);
    for (i, px) in out.iter_mut().enumerate() {
        *px = palette[((bits >> (2 * i)) & 3) as usize];
    }
}

/// The alpha palette of a BC3 block.
fn bc3_alpha_palette(a0: u8, a1: u8) -> [u8; 8] {
    let (a0f, a1f) = (a0 as u32, a1 as u32);
    if a0 > a1 {
        let mut p = [a0, a1, 0, 0, 0, 0, 0, 0];
        for (i, slot) in p.iter_mut().enumerate().skip(2) {
            let k = (i - 1) as u32;
            *slot = ((a0f * (7 - k) + a1f * k + 3) / 7) as u8;
        }
        p
    } else {
        let mut p = [a0, a1, 0, 0, 0, 0, 0, 255];
        for (i, slot) in p.iter_mut().enumerate().skip(2).take(4) {
            let k = (i - 1) as u32;
            *slot = ((a0f * (5 - k) + a1f * k + 2) / 5) as u8;
        }
        p
    }
}

fn decode_alpha_block(d: &[u8], out: &mut [Rgba; 16]) {
    let palette = bc3_alpha_palette(d[0], d[1]);
    let mut bits: u64 = 0;
    for i in 0..6 {
        bits |= (d[2 + i] as u64) << (8 * i);
    }
    for (i, px) in out.iter_mut().enumerate() {
        px[3] = palette[((bits >> (3 * i)) & 7) as usize];
    }
}

/// Decode a mip.
pub fn decode(format: Format, width: usize, height: usize, data: &[u8]) -> Result<Image, String> {
    if data.len() != format.mip_size(width, height) {
        return Err(format!(
            "{} bytes for a {width}x{height} {format:?} mip, expected {}",
            data.len(),
            format.mip_size(width, height)
        ));
    }
    let mut pixels = vec![[0u8; 4]; width * height];
    match format {
        Format::Bgra8 => {
            for (i, px) in pixels.iter_mut().enumerate() {
                let b = &data[4 * i..4 * i + 4];
                *px = [b[2], b[1], b[0], b[3]];
            }
        }
        Format::Bc1 | Format::Bc3 => {
            let bw = width.div_ceil(4);
            let bh = height.div_ceil(4);
            let bs = if format == Format::Bc1 { 8 } else { 16 };
            let mut block = [[0u8; 4]; 16];
            for by in 0..bh {
                for bx in 0..bw {
                    let d = &data[(by * bw + bx) * bs..][..bs];
                    if format == Format::Bc1 {
                        decode_colour_block(d, false, &mut block);
                    } else {
                        decode_colour_block(&d[8..], true, &mut block);
                        decode_alpha_block(d, &mut block);
                    }
                    for (i, px) in block.iter().enumerate() {
                        let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                        if x < width && y < height {
                            pixels[y * width + x] = *px;
                        }
                    }
                }
            }
        }
    }
    Ok(Image { width, height, pixels })
}

/// Encode the colour half of a block: endpoints from the block's extreme
/// colours, four-colour mode, nearest palette entry per pixel.
fn encode_colour_block(block: &[Rgba; 16], out: &mut [u8]) {
    let sum = |c: &Rgba| c[0] as u32 + c[1] as u32 + c[2] as u32;
    let hi = block.iter().max_by_key(|c| sum(c)).unwrap();
    let lo = block.iter().min_by_key(|c| sum(c)).unwrap();
    let mut c0 = to565([hi[0], hi[1], hi[2]]);
    let mut c1 = to565([lo[0], lo[1], lo[2]]);
    if c0 < c1 {
        std::mem::swap(&mut c0, &mut c1);
    }
    let palette = bc1_palette(c0, c1);
    let mut bits = 0u32;
    if c0 != c1 {
        for (i, px) in block.iter().enumerate() {
            let dist = |p: &Rgba| (0..3).map(|k| (p[k] as i32 - px[k] as i32).pow(2)).sum::<i32>();
            let best = (0..4).min_by_key(|&k| dist(&palette[k])).unwrap();
            bits |= (best as u32) << (2 * i);
        }
    }
    out[..2].copy_from_slice(&c0.to_le_bytes());
    out[2..4].copy_from_slice(&c1.to_le_bytes());
    out[4..8].copy_from_slice(&bits.to_le_bytes());
}

/// Encode the alpha half of a BC3 block: eight-level mode between the
/// block's extreme alphas.
fn encode_alpha_block(block: &[Rgba; 16], out: &mut [u8]) {
    let a0 = block.iter().map(|p| p[3]).max().unwrap();
    let a1 = block.iter().map(|p| p[3]).min().unwrap();
    out[0] = a0;
    out[1] = a1;
    let mut bits = 0u64;
    if a0 != a1 {
        let palette = bc3_alpha_palette(a0, a1);
        for (i, px) in block.iter().enumerate() {
            let best = (0..8).min_by_key(|&k| (palette[k] as i32 - px[3] as i32).abs()).unwrap();
            bits |= (best as u64) << (3 * i);
        }
    }
    for i in 0..6 {
        out[2 + i] = (bits >> (8 * i)) as u8;
    }
}

/// Encode an image as a mip.
pub fn encode(format: Format, img: &Image) -> Vec<u8> {
    let (width, height) = (img.width, img.height);
    let mut out = vec![0u8; format.mip_size(width, height)];
    match format {
        Format::Bgra8 => {
            for (i, px) in img.pixels.iter().enumerate() {
                out[4 * i..4 * i + 4].copy_from_slice(&[px[2], px[1], px[0], px[3]]);
            }
        }
        Format::Bc1 | Format::Bc3 => {
            let bw = width.div_ceil(4);
            let bh = height.div_ceil(4);
            let bs = if format == Format::Bc1 { 8 } else { 16 };
            for by in 0..bh {
                for bx in 0..bw {
                    let mut block = [[0u8; 4]; 16];
                    for (i, px) in block.iter_mut().enumerate() {
                        let (x, y) = ((bx * 4 + i % 4).min(width - 1), (by * 4 + i / 4).min(height - 1));
                        *px = img.pixels[y * width + x];
                    }
                    let o = &mut out[(by * bw + bx) * bs..][..bs];
                    if format == Format::Bc1 {
                        encode_colour_block(&block, o);
                    } else {
                        encode_alpha_block(&block, &mut o[..8]);
                        encode_colour_block(&block, &mut o[8..]);
                    }
                }
            }
        }
    }
    out
}

/// The image squeezed horizontally by `factor` (> 1) into the centre of
/// the same frame, the columns beyond its edges repeating the edge column:
/// what a 16:9-authored screen mask must become for a frame `factor` times
/// wider, so that it still lines up with the 16:9 picture in the middle
/// and continues without a seam to the sides. Linear interpolation.
pub fn refit(img: &Image, factor: f64) -> Image {
    let (w, h) = (img.width, img.height);
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        let row = &img.pixels[y * w..(y + 1) * w];
        for x in 0..w {
            // the source column this output column samples, in pixel centres
            let sx = ((x as f64 + 0.5 - w as f64 / 2.0) * factor + w as f64 / 2.0 - 0.5).clamp(0.0, (w - 1) as f64);
            let x0 = sx.floor() as usize;
            let x1 = (x0 + 1).min(w - 1);
            let t = sx - x0 as f64;
            let mut px = [0u8; 4];
            for k in 0..4 {
                px[k] = (row[x0][k] as f64 * (1.0 - t) + row[x1][k] as f64 * t).round() as u8;
            }
            pixels.push(px);
        }
    }
    Image { width: w, height: h, pixels }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: usize, h: usize) -> Image {
        let pixels = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let v = (255 * x / (w - 1)) as u8;
                [v, v / 2, 255 - v, (255 * y / (h.max(2) - 1)) as u8]
            })
            .collect();
        Image { width: w, height: h, pixels }
    }

    fn max_error(a: &Image, b: &Image, channels: std::ops::Range<usize>) -> i32 {
        a.pixels
            .iter()
            .zip(&b.pixels)
            .map(|(p, q)| channels.clone().map(|k| (p[k] as i32 - q[k] as i32).abs()).max().unwrap())
            .max()
            .unwrap()
    }

    #[test]
    fn bgra_round_trips_exactly() {
        let img = gradient(7, 5);
        let data = encode(Format::Bgra8, &img);
        assert_eq!(data.len(), 140);
        assert_eq!(decode(Format::Bgra8, 7, 5, &data).unwrap(), img);
    }

    #[test]
    fn bc1_and_bc3_round_trip_a_gradient_closely() {
        let img = gradient(64, 16);
        let bc1 = encode(Format::Bc1, &img);
        assert_eq!(bc1.len(), 16 * 4 * 8);
        let back = decode(Format::Bc1, 64, 16, &bc1).unwrap();
        assert!(max_error(&img, &back, 0..3) <= 12, "bc1 error {}", max_error(&img, &back, 0..3));
        assert!(back.pixels.iter().all(|p| p[3] == 255));

        let bc3 = encode(Format::Bc3, &img);
        assert_eq!(bc3.len(), 16 * 4 * 16);
        let back = decode(Format::Bc3, 64, 16, &bc3).unwrap();
        assert!(max_error(&img, &back, 0..3) <= 12, "bc3 colour error {}", max_error(&img, &back, 0..3));
        assert!(max_error(&img, &back, 3..4) <= 10, "bc3 alpha error {}", max_error(&img, &back, 3..4));
    }

    #[test]
    fn flat_blocks_and_odd_sizes_encode() {
        let flat = Image { width: 5, height: 3, pixels: vec![[10, 20, 30, 40]; 15] };
        for f in [Format::Bc1, Format::Bc3] {
            let back = decode(f, 5, 3, &encode(f, &flat)).unwrap();
            assert!(max_error(&flat, &back, 0..3) <= 6);
            if f == Format::Bc3 {
                assert!(back.pixels.iter().all(|p| p[3] == 40));
            }
        }
    }

    #[test]
    fn decodes_the_known_bc1_block() {
        // c0 = white, c1 = black, indices: 0,1,2,3 repeating
        let block = [0xFF, 0xFF, 0x00, 0x00, 0xE4, 0xE4, 0xE4, 0xE4];
        let img = decode(Format::Bc1, 4, 4, &block).unwrap();
        assert_eq!(img.pixels[0], [255, 255, 255, 255]);
        assert_eq!(img.pixels[1], [0, 0, 0, 255]);
        assert_eq!(img.pixels[2], [170, 170, 170, 255]);
        assert_eq!(img.pixels[3], [85, 85, 85, 255]);
    }

    #[test]
    fn refit_squeezes_to_the_centre_and_extends_the_edges() {
        // a 12-wide image, black left half, white right half
        let pixels: Vec<Rgba> = (0..12).map(|x| if x < 6 { [0, 0, 0, 0] } else { [255, 255, 255, 255] }).collect();
        let img = Image { width: 12, height: 1, pixels };
        let out = refit(&img, 2.0);
        // the transition stays in the middle, the outer columns are the edge values
        assert_eq!(out.pixels[0], [0, 0, 0, 0]);
        assert_eq!(out.pixels[5], [0, 0, 0, 0]);
        assert_eq!(out.pixels[6], [255, 255, 255, 255]);
        assert_eq!(out.pixels[11], [255, 255, 255, 255]);
        // factor 1 is the identity
        assert_eq!(refit(&img, 1.0), img);
        // a ramp: the centre keeps its value, the sides clamp
        let ramp = Image { width: 9, height: 1, pixels: (0..9).map(|x| [(x * 30) as u8; 4]).collect() };
        let out = refit(&ramp, 3.0);
        assert_eq!(out.pixels[4], [120; 4]);
        assert_eq!(out.pixels[0], [0; 4]);
        assert_eq!(out.pixels[8], [240; 4]);
        assert_eq!(out.pixels[3], [30; 4]);
    }
}
