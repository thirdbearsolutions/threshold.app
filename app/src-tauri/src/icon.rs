//! Home-screen icons for the phone app, drawn per machine.
//!
//! Each Mac is its own home-screen app, and two identical icons side by
//! side are exactly the confusion phone access has to rule out. So each
//! icon is the machine's colour with the first letter of its name in
//! white — distinct at a glance, before the label underneath is read.
//!
//! iOS wants PNG for a home-screen icon, so this carries a minimal PNG
//! writer (a two-colour palette, stored deflate) and a 5×7 bitmap font
//! rather than an image library.

/// 5×7 glyphs, one row per byte, high five bits used.
fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x11, 0x0A, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        _ => [0x00, 0x00, 0x00, 0x0E, 0x00, 0x00, 0x00],
    }
}

/// `#rrggbb` to bytes; anything else is a neutral grey rather than an
/// error, since an icon is not worth failing a page over.
pub fn parse_color(s: &str) -> [u8; 3] {
    let hex = s.trim().trim_start_matches('#');
    let byte = |i: usize| hex.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok());
    match (hex.len(), byte(0), byte(2), byte(4)) {
        (6, Some(r), Some(g), Some(b)) => [r, g, b],
        _ => [0x55, 0x55, 0x55],
    }
}

/// Whether pixel (x, y) of a `size` icon is part of the letter.
fn ink(letter: char, size: usize, x: usize, y: usize) -> bool {
    // The glyph fills the middle half of the icon, on whole-pixel cells so
    // edges stay crisp at every size.
    let cell = (size / 2 / 7).max(1);
    let (w, h) = (5 * cell, 7 * cell);
    let (x0, y0) = ((size - w) / 2, (size - h) / 2);
    if x < x0 || y < y0 || x >= x0 + w || y >= y0 + h {
        return false;
    }
    let (gx, gy) = ((x - x0) / cell, (y - y0) / cell);
    glyph(letter)[gy] & (0x10 >> gx) != 0
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// zlib with stored (uncompressed) blocks: valid everywhere, and a 1-bit
/// image is small enough that compression would not matter.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = if data.is_empty() { vec![&[][..]] } else { data.chunks(65_535).collect() };
    for (i, block) in blocks.iter().enumerate() {
        out.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    out.extend_from_slice(&((b << 16) | a).to_be_bytes());
    out
}

/// A square PNG: `color` background, white first letter of `name`.
pub fn png(name: &str, color: &str, size: usize) -> Vec<u8> {
    let letter = name.chars().next().unwrap_or('?');
    let row_bytes = size.div_ceil(8);
    let mut raw = Vec::with_capacity((row_bytes + 1) * size);
    for y in 0..size {
        raw.push(0); // no filter
        for bx in 0..row_bytes {
            let mut byte = 0u8;
            for bit in 0..8 {
                let x = bx * 8 + bit;
                if x < size && ink(letter, size, x, y) {
                    byte |= 0x80 >> bit;
                }
            }
            raw.push(byte);
        }
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(size as u32).to_be_bytes());
    ihdr.extend_from_slice(&(size as u32).to_be_bytes());
    ihdr.extend_from_slice(&[1, 3, 0, 0, 0]); // 1-bit, palette
    chunk(&mut out, b"IHDR", &ihdr);
    let [r, g, b] = parse_color(color);
    chunk(&mut out, b"PLTE", &[r, g, b, 255, 255, 255]);
    chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

/// The same icon as SVG, for browsers that take one.
pub fn svg(name: &str, color: &str) -> String {
    let [r, g, b] = parse_color(color);
    let letter = name.chars().next().unwrap_or('?');
    let mut rects = String::new();
    let rows = glyph(letter);
    for (y, row) in rows.iter().enumerate() {
        for x in 0..5 {
            if row & (0x10 >> x) != 0 {
                rects.push_str(&format!("<rect x=\"{}\" y=\"{}\" width=\"1\" height=\"1\"/>", 4 + x, 2 + y));
            }
        }
    }
    format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 13 11\"><rect width=\"13\" height=\"11\" rx=\"2\" fill=\"#{r:02x}{g:02x}{b:02x}\"/><g fill=\"#fff\">{rects}</g></svg>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_png_has_the_right_signature_size_and_checksums() {
        let p = png("studio", "#2f6fdf", 180);
        assert_eq!(&p[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&p[16..24], &[0, 0, 0, 180, 0, 0, 0, 180]);
        // Every chunk's CRC must match or iOS shows a blank icon.
        let mut i = 8;
        while i < p.len() {
            let len = u32::from_be_bytes(p[i..i + 4].try_into().unwrap()) as usize;
            let crc = u32::from_be_bytes(p[i + 8 + len..i + 12 + len].try_into().unwrap());
            assert_eq!(crc, crc32(&p[i + 4..i + 8 + len]));
            i += 12 + len;
        }
        assert_eq!(i, p.len());
    }

    #[test]
    fn two_machines_get_different_icons() {
        assert_ne!(png("acme", "#2f6fdf", 64), png("studio", "#2f6fdf", 64), "the letter differs");
        assert_ne!(png("acme", "#2f6fdf", 64), png("acme", "#1f9d55", 64), "the colour differs");
    }

    #[test]
    fn a_bad_colour_is_grey_not_a_failure() {
        assert_eq!(parse_color("nope"), [0x55, 0x55, 0x55]);
        assert_eq!(parse_color("#1f9d55"), [0x1f, 0x9d, 0x55]);
    }

    #[test]
    fn the_stored_zlib_spans_blocks_correctly() {
        let data = vec![7u8; 70_000];
        let z = zlib_stored(&data);
        // Header, two block headers, the data, and the checksum.
        assert_eq!(z.len(), 2 + 5 + 5 + data.len() + 4);
        assert_eq!(z[2], 0, "first block is not final");
        assert_eq!(z[2 + 5 + 65_535], 1, "second block is");
    }
}
