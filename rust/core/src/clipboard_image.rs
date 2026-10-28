//! Clipboard images (`/actions/clipboard?type=png`): PNG on the wire, and on
//! the PC the bitmap formats Windows keeps on its clipboard (`CF_DIB`,
//! `CF_DIBV5`) besides the registered `PNG` format.
//!
//! Every image a device sends, and every bitmap another program put on the
//! clipboard, is untrusted: it is decoded only within [`MAX_SIDE`] and
//! [`MAX_PIXELS`], with checked sizes, so a crafted one fails with an
//! [`ImageError`] instead of allocating without bound or panicking.
use std::io::Cursor;

/// The widest and tallest image, in pixels.
pub const MAX_SIDE: u32 = 16384;
/// The most pixels an image may have: 32 Mi, 128 MiB as RGBA.
pub const MAX_PIXELS: u64 = 1 << 25;
/// The longest packed DIB of at most [`MAX_PIXELS`]: its pixels, at most 4
/// bytes each with row padding, and room for headers, masks and a color
/// table. Clipboard memory longer than this is refused unread.
pub const MAX_DIB_BYTES: usize = (MAX_PIXELS as usize) * 4 + (1 << 20);
/// What the PNG decoder may allocate besides the pixels (chunk data and
/// decompression state).
const DECODER_BYTES: usize = 16 << 20;
pub const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// `BITMAPINFOHEADER` and the later `BITMAPV4HEADER` and `BITMAPV5HEADER`.
const INFO_HEADER: usize = 40;
const V5_HEADER: usize = 124;
const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;
const BI_ALPHABITFIELDS: u32 = 6;
/// `LCS_sRGB` and `LCS_GM_IMAGES` for the V5 header this writes.
const LCS_SRGB: u32 = 0x7352_4742;
const LCS_GM_IMAGES: u32 = 4;

#[derive(Debug, PartialEq, Eq)]
pub enum ImageError {
    /// Not a PNG at all.
    NotPng,
    /// Larger than [`MAX_SIDE`] or [`MAX_PIXELS`].
    TooLarge,
    /// A PNG of `bytes`, over the `max` it may have.
    TooManyBytes { bytes: usize, max: usize },
    /// A bitmap this can't read, such as a palette or a compressed one.
    Unsupported(String),
    /// Cut short or inconsistent.
    Malformed(String),
}
impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotPng => f.write_str("the image is not a PNG"),
            Self::TooLarge => write!(
                f,
                "the image is larger than {MAX_SIDE} pixels a side or {MAX_PIXELS} pixels"
            ),
            Self::TooManyBytes { bytes, max } => write!(
                f,
                "the image is {bytes} bytes as PNG, over the limit of {max} bytes"
            ),
            Self::Unsupported(what) => write!(f, "unsupported image: {what}"),
            Self::Malformed(what) => write!(f, "malformed image: {what}"),
        }
    }
}
impl std::error::Error for ImageError {}
impl ImageError {
    /// Over a limit, of pixels or of bytes, rather than broken.
    pub fn too_large(&self) -> bool {
        matches!(self, Self::TooLarge | Self::TooManyBytes { .. })
    }
}

/// An image as 8-bit RGBA with straight alpha, top row first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
impl Image {
    /// Whether any pixel is less than opaque.
    pub fn has_alpha(&self) -> bool {
        self.rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[3] != 255)
    }
}

/// The RGBA buffer size of a `width` x `height` image within the limits.
fn pixel_bytes(width: u32, height: u32) -> Result<usize, ImageError> {
    if width == 0 || height == 0 {
        return Err(ImageError::Malformed("the image has no pixels".into()));
    }
    if width > MAX_SIDE || height > MAX_SIDE || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(ImageError::TooLarge);
    }
    usize::try_from(u64::from(width) * u64::from(height) * 4).map_err(|_| ImageError::TooLarge)
}

/// Decodes a PNG: its first frame, any color type and depth, as RGBA.
pub fn decode_png(bytes: &[u8]) -> Result<Image, ImageError> {
    if !bytes.starts_with(PNG_SIGNATURE) {
        return Err(ImageError::NotPng);
    }
    let malformed = |error: png::DecodingError| match error {
        png::DecodingError::LimitsExceeded => ImageError::TooLarge,
        error => ImageError::Malformed(error.to_string()),
    };
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(bytes),
        png::Limits {
            bytes: DECODER_BYTES,
        },
    );
    // Palettes, low bit depths and transparency chunks expand, 16 bits strip
    // to 8: what is left is 8-bit gray, gray and alpha, RGB or RGBA.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(malformed)?;
    let (width, height) = (reader.info().width, reader.info().height);
    let rgba_bytes = pixel_bytes(width, height)?;
    let size = reader
        .output_buffer_size()
        .filter(|size| *size <= rgba_bytes)
        .ok_or(ImageError::TooLarge)?;
    // One buffer of the RGBA size: the frame is decoded into its start and
    // spread to RGBA in place, so an image never takes twice its pixels.
    let mut rgba = vec![0; rgba_bytes];
    let frame = reader.next_frame(&mut rgba[..size]).map_err(malformed)?;
    if frame.bit_depth != png::BitDepth::Eight {
        return Err(ImageError::Unsupported(format!(
            "{:?} bits per channel",
            frame.bit_depth
        )));
    }
    let channels = match frame.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => {
            return Err(ImageError::Unsupported("an unexpanded palette".into()));
        }
    };
    let row = width as usize * channels;
    if frame.line_size < row || size < frame.line_size * height as usize {
        return Err(ImageError::Malformed(
            "the frame is shorter than its size".into(),
        ));
    }
    spread_to_rgba(&mut rgba, frame.line_size, channels, width as usize)?;
    Ok(Image {
        width,
        height,
        rgba,
    })
}

/// Spreads the rows of `line_size` bytes of `channels`-channel pixels at the
/// start of `buffer`, which is exactly the RGBA size of rows of `width`
/// pixels, to RGBA in place. From the last pixel back, each lands at or past
/// where it was read, so none is overwritten before it is read.
fn spread_to_rgba(
    buffer: &mut [u8],
    line_size: usize,
    channels: usize,
    width: usize,
) -> Result<(), ImageError> {
    let rgba_row = width * 4;
    if !(1..=4).contains(&channels)
        || line_size < width * channels
        || line_size > rgba_row
        || rgba_row == 0
        || !buffer.len().is_multiple_of(rgba_row)
    {
        return Err(ImageError::Malformed(
            "the frame does not fit its size".into(),
        ));
    }
    if channels == 4 && line_size == rgba_row {
        return Ok(());
    }
    let height = buffer.len() / rgba_row;
    for y in (0..height).rev() {
        for x in (0..width).rev() {
            let from = y * line_size + x * channels;
            let pixel = match buffer[from..from + channels] {
                [gray] => [gray, gray, gray, 255],
                [gray, alpha] => [gray, gray, gray, alpha],
                [r, g, b] => [r, g, b, 255],
                [r, g, b, a] => [r, g, b, a],
                _ => unreachable!("1 to 4 channels"),
            };
            let to = y * rgba_row + x * 4;
            buffer[to..to + 4].copy_from_slice(&pixel);
        }
    }
    Ok(())
}

/// The length of the PNG `bytes` start with, through its `IEND` chunk, or
/// `None` when they aren't one or it is cut short. Clipboard memory is
/// rounded up, so a PNG there can be followed by anything.
pub fn png_extent(bytes: &[u8]) -> Option<usize> {
    if !bytes.starts_with(PNG_SIGNATURE) {
        return None;
    }
    let mut at = PNG_SIGNATURE.len();
    loop {
        let length = u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize;
        let kind = bytes.get(at + 4..at + 8)?;
        // Length, type, data and CRC.
        let end = at.checked_add(12)?.checked_add(length)?;
        if end > bytes.len() {
            return None;
        }
        if kind == b"IEND" {
            return Some(end);
        }
        at = end;
    }
}

/// Encodes `image` as an 8-bit PNG: RGB when it is opaque, else RGBA.
pub fn encode_png(image: &Image) -> Result<Vec<u8>, ImageError> {
    let expected = pixel_bytes(image.width, image.height)?;
    if image.rgba.len() != expected {
        return Err(ImageError::Malformed(
            "the pixels don't match the size".into(),
        ));
    }
    let failed = |error: png::EncodingError| ImageError::Malformed(error.to_string());
    let alpha = image.has_alpha();
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, image.width, image.height);
    encoder.set_color(if alpha {
        png::ColorType::Rgba
    } else {
        png::ColorType::Rgb
    });
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(failed)?;
    if alpha {
        writer.write_image_data(&image.rgba).map_err(failed)?;
    } else {
        let rgb: Vec<u8> = image
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[r, g, b, _]| [r, g, b])
            .collect();
        writer.write_image_data(&rgb).map_err(failed)?;
    }
    writer.finish().map_err(failed)?;
    Ok(out)
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// One channel of a `BI_BITFIELDS` pixel: where its bits are and how many.
#[derive(Clone, Copy)]
struct Channel {
    mask: u32,
    shift: u32,
    max: u32,
}
impl Channel {
    /// A contiguous mask, or none (0); others are refused.
    fn of(mask: u32) -> Result<Option<Self>, ImageError> {
        if mask == 0 {
            return Ok(None);
        }
        let shift = mask.trailing_zeros();
        let max = mask >> shift;
        if max & max.wrapping_add(1) != 0 {
            return Err(ImageError::Unsupported(format!(
                "the channel mask {mask:#010x} is not contiguous"
            )));
        }
        Ok(Some(Self { mask, shift, max }))
    }
    fn read(self, pixel: u32) -> u8 {
        let value = (pixel & self.mask) >> self.shift;
        (u64::from(value) * 255 / u64::from(self.max)) as u8
    }
}

/// Reads a packed DIB (`CF_DIB` or `CF_DIBV5`: the header, any masks and
/// color table, then the pixels) of 24 or 32 bits a pixel, uncompressed or
/// with bit fields. A 32-bit bitmap whose alpha is zero everywhere, as most
/// programs write one without alpha, is opaque.
pub fn decode_dib(bytes: &[u8]) -> Result<Image, ImageError> {
    let short = || ImageError::Malformed("the bitmap is cut short".into());
    let header = u32_at(bytes, 0).ok_or_else(short)? as usize;
    if header < INFO_HEADER || header > bytes.len() {
        return Err(ImageError::Malformed(format!("a header of {header} bytes")));
    }
    let width = u32_at(bytes, 4).ok_or_else(short)? as i32;
    let height = u32_at(bytes, 8).ok_or_else(short)? as i32;
    let planes = u16::from_le_bytes([bytes[12], bytes[13]]);
    let bits = u16::from_le_bytes([bytes[14], bytes[15]]);
    let compression = u32_at(bytes, 16).ok_or_else(short)?;
    let colors_used = u32_at(bytes, 32).ok_or_else(short)?;
    if width <= 0 || height == 0 || height == i32::MIN || planes != 1 {
        return Err(ImageError::Malformed(format!(
            "{width} x {height} pixels in {planes} planes"
        )));
    }
    let (width, top_down) = (width as u32, height < 0);
    let height = height.unsigned_abs();
    let rgba_bytes = pixel_bytes(width, height)?;
    if !matches!(bits, 24 | 32) {
        return Err(ImageError::Unsupported(format!("{bits} bits a pixel")));
    }
    let mut offset = header;
    let masks = match compression {
        BI_RGB => None,
        BI_BITFIELDS | BI_ALPHABITFIELDS if bits == 32 => {
            // Right after the info header: a bare one is followed by its
            // masks, later ones hold them there.
            if header == INFO_HEADER {
                offset += if compression == BI_ALPHABITFIELDS {
                    16
                } else {
                    12
                };
            }
            let mask = |n: usize| u32_at(bytes, INFO_HEADER + 4 * n).ok_or_else(short);
            let alpha = if header >= 56 || compression == BI_ALPHABITFIELDS {
                mask(3)?
            } else {
                0
            };
            Some([mask(0)?, mask(1)?, mask(2)?, alpha])
        }
        other => {
            return Err(ImageError::Unsupported(format!(
                "compression {other} at {bits} bits a pixel"
            )));
        }
    };
    // A color table is optional at these depths and never used.
    offset = (colors_used as usize)
        .checked_mul(4)
        .and_then(|table| offset.checked_add(table))
        .ok_or_else(short)?;
    let stride = (width as usize * bits as usize).div_ceil(32) * 4;
    let image_bytes = stride
        .checked_mul(height as usize)
        .ok_or(ImageError::TooLarge)?;
    // Some programs write the three masks after a V4 or V5 header too.
    if masks.is_some()
        && header > INFO_HEADER
        && bytes.len().checked_sub(offset) == Some(image_bytes + 12)
    {
        offset += 12;
    }
    let pixels = offset
        .checked_add(image_bytes)
        .and_then(|end| bytes.get(offset..end))
        .ok_or_else(short)?;
    let (channels, alpha_channel) = match masks {
        Some([r, g, b, a]) => {
            let color = |mask, name| {
                Channel::of(mask)?.ok_or_else(|| ImageError::Unsupported(format!("no {name} mask")))
            };
            (
                Some([color(r, "red")?, color(g, "green")?, color(b, "blue")?]),
                Channel::of(a)?,
            )
        }
        None => (None, None),
    };
    let mut rgba = vec![0u8; rgba_bytes];
    let row_bytes = width as usize * 4;
    for (y, line) in pixels.chunks_exact(stride).enumerate() {
        let target = if top_down { y } else { height as usize - 1 - y };
        let out = rgba[target * row_bytes..][..row_bytes]
            .as_chunks_mut::<4>()
            .0;
        match (&channels, bits) {
            (Some([r, g, b]), _) => {
                for (pixel, out) in line.as_chunks::<4>().0.iter().zip(out) {
                    let value = u32::from_le_bytes(*pixel);
                    let alpha = alpha_channel.map_or(255, |a| a.read(value));
                    *out = [r.read(value), g.read(value), b.read(value), alpha];
                }
            }
            (None, 24) => {
                for (&[b, g, r], out) in line.as_chunks::<3>().0.iter().zip(out) {
                    *out = [r, g, b, 255];
                }
            }
            (None, _) => {
                for (&[b, g, r, a], out) in line.as_chunks::<4>().0.iter().zip(out) {
                    *out = [r, g, b, a];
                }
            }
        }
    }
    // Even with an alpha mask: some programs write one and no alpha.
    let pixels = rgba.as_chunks_mut::<4>().0;
    if bits == 32 && pixels.iter().all(|pixel| pixel[3] == 0) {
        for pixel in pixels {
            pixel[3] = 255;
        }
    }
    Ok(Image {
        width,
        height,
        rgba,
    })
}

/// `image` as a packed `CF_DIBV5`: a V5 header with BGRA bit fields and
/// straight alpha in sRGB, bottom row first as most programs expect.
pub fn encode_dibv5(image: &Image) -> Result<Vec<u8>, ImageError> {
    let pixels = pixel_bytes(image.width, image.height)?;
    if image.rgba.len() != pixels {
        return Err(ImageError::Malformed(
            "the pixels don't match the size".into(),
        ));
    }
    let mut out = Vec::with_capacity(V5_HEADER + pixels);
    let put = |out: &mut Vec<u8>, value: u32| out.extend_from_slice(&value.to_le_bytes());
    put(&mut out, V5_HEADER as u32);
    put(&mut out, image.width);
    put(&mut out, image.height);
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    put(&mut out, BI_BITFIELDS);
    put(&mut out, pixels as u32);
    // Resolution, colors used and important: none.
    out.resize(out.len() + 16, 0);
    for mask in [0x00ff_0000, 0x0000_ff00, 0x0000_00ff, 0xff00_0000, LCS_SRGB] {
        put(&mut out, mask);
    }
    // Endpoints (36 bytes) and gamma (12 bytes): unused for sRGB.
    out.resize(out.len() + 48, 0);
    put(&mut out, LCS_GM_IMAGES);
    // Profile data, profile size and reserved.
    out.resize(V5_HEADER, 0);
    let row = image.width as usize * 4;
    for line in image.rgba.chunks_exact(row).rev() {
        for &[r, g, b, a] in line.as_chunks::<4>().0 {
            out.extend_from_slice(&[b, g, r, a]);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32) -> Image {
        let rgba = (0..width * height)
            .flat_map(|n| {
                [
                    n as u8,
                    (n >> 8) as u8,
                    7,
                    if n % 3 == 0 { 128 } else { 255 },
                ]
            })
            .collect();
        Image {
            width,
            height,
            rgba,
        }
    }

    fn opaque(mut image: Image) -> Image {
        image
            .rgba
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .for_each(|pixel| pixel[3] = 255);
        image
    }

    /// A packed DIB with a bare info header, as `CF_DIB` holds it.
    fn info_dib(
        width: i32,
        height: i32,
        bits: u16,
        compression: u32,
        extra: &[u8],
        pixels: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&width.to_le_bytes());
        out.extend_from_slice(&height.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(&compression.to_le_bytes());
        out.resize(40, 0);
        out.extend_from_slice(extra);
        out.extend_from_slice(pixels);
        out
    }

    #[test]
    fn png_round_trips_with_and_without_alpha() {
        for picture in [image(17, 9), opaque(image(17, 9))] {
            let png = encode_png(&picture).unwrap();
            assert!(png.starts_with(PNG_SIGNATURE));
            assert_eq!(decode_png(&png).unwrap(), picture);
        }
    }

    #[test]
    fn every_png_color_type_decodes_in_its_one_rgba_buffer() {
        // A 3 x 2 image of each color type, its samples numbered.
        let encode = |color, depth, samples: &[u8], palette: Option<&[u8]>| {
            let mut out = Vec::new();
            let mut encoder = png::Encoder::new(&mut out, 3, 2);
            encoder.set_color(color);
            encoder.set_depth(depth);
            if let Some(palette) = palette {
                encoder.set_palette(palette.to_vec());
            }
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(samples).unwrap();
            writer.finish().unwrap();
            out
        };
        let eight = png::BitDepth::Eight;
        let samples = |channels: u8| (1..=6 * channels).collect::<Vec<u8>>();
        let gray = encode(png::ColorType::Grayscale, eight, &samples(1), None);
        assert_eq!(
            decode_png(&gray).unwrap().rgba,
            (1..=6).flat_map(|v| [v, v, v, 255]).collect::<Vec<_>>()
        );
        let gray_alpha = encode(png::ColorType::GrayscaleAlpha, eight, &samples(2), None);
        assert_eq!(
            decode_png(&gray_alpha).unwrap().rgba,
            (0..6)
                .flat_map(|n| [2 * n + 1, 2 * n + 1, 2 * n + 1, 2 * n + 2])
                .collect::<Vec<_>>()
        );
        let rgb = encode(png::ColorType::Rgb, eight, &samples(3), None);
        assert_eq!(
            decode_png(&rgb).unwrap().rgba,
            (0..6)
                .flat_map(|n| [3 * n + 1, 3 * n + 2, 3 * n + 3, 255])
                .collect::<Vec<_>>()
        );
        let rgba = encode(png::ColorType::Rgba, eight, &samples(4), None);
        assert_eq!(decode_png(&rgba).unwrap().rgba, samples(4));
        // A palette expands, 16 bits keep their high byte.
        let palette = [10, 20, 30, 40, 50, 60];
        let indexed = encode(
            png::ColorType::Indexed,
            eight,
            &[0, 1, 0, 1, 1, 0],
            Some(&palette),
        );
        assert_eq!(
            &decode_png(&indexed).unwrap().rgba[..8],
            &[10, 20, 30, 255, 40, 50, 60, 255]
        );
        let wide = encode(
            png::ColorType::Grayscale,
            png::BitDepth::Sixteen,
            &[7; 12],
            None,
        );
        assert_eq!(decode_png(&wide).unwrap().rgba, [7, 7, 7, 255].repeat(6));
    }

    #[test]
    fn rows_spread_to_rgba_in_place_past_their_padding() {
        // Two rows of two RGB pixels, each row padded by a byte.
        let mut buffer = vec![1, 2, 3, 4, 5, 6, 0, 7, 8, 9, 10, 11, 12, 0];
        buffer.resize(16, 0xee);
        spread_to_rgba(&mut buffer, 7, 3, 2).unwrap();
        assert_eq!(
            buffer,
            [1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]
        );
        // A row longer than its RGBA would overwrite pixels not yet read.
        assert!(spread_to_rgba(&mut [0; 16], 9, 3, 2).is_err());
        assert!(spread_to_rgba(&mut [0; 15], 6, 3, 2).is_err());
    }

    #[test]
    fn a_png_ends_with_its_iend_chunk() {
        let png = encode_png(&image(3, 3)).unwrap();
        assert_eq!(png_extent(&png), Some(png.len()));
        let mut padded = png.clone();
        padded.extend_from_slice(&[0; 100]);
        assert_eq!(png_extent(&padded), Some(png.len()));
        for end in 0..png.len() {
            assert_eq!(png_extent(&png[..end]), None, "{end}");
        }
        let mut huge = png.clone();
        huge[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(png_extent(&huge), None);
    }

    #[test]
    fn dibv5_round_trips_bottom_up() {
        let picture = image(5, 3);
        let dib = encode_dibv5(&picture).unwrap();
        assert_eq!(dib.len(), 124 + 5 * 3 * 4);
        // The bottom row comes first.
        assert_eq!(&dib[124..128], &[7, 0, 10, 255][..]);
        assert_eq!(decode_dib(&dib).unwrap(), picture);
    }

    #[test]
    fn a_24_bit_dib_pads_its_rows_and_reads_top_down_too() {
        // 3 pixels of 3 bytes: 9 bytes padded to 12 a row.
        let rows = [
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 0, 0],
            [10, 11, 12, 13, 14, 15, 16, 17, 18, 0, 0, 0],
        ];
        let bottom_up = info_dib(3, 2, 24, BI_RGB, &[], &rows.concat());
        let image = decode_dib(&bottom_up).unwrap();
        assert_eq!((image.width, image.height), (3, 2));
        assert_eq!(&image.rgba[..4], &[12, 11, 10, 255]);
        assert_eq!(&image.rgba[12..16], &[3, 2, 1, 255]);
        let top_down = info_dib(3, -2, 24, BI_RGB, &[], &rows.concat());
        assert_eq!(&decode_dib(&top_down).unwrap().rgba[..4], &[3, 2, 1, 255]);
    }

    #[test]
    fn a_32_bit_dib_without_alpha_is_opaque_and_bit_fields_follow_their_masks() {
        let zero_alpha = info_dib(1, 1, 32, BI_RGB, &[], &[1, 2, 3, 0]);
        assert_eq!(decode_dib(&zero_alpha).unwrap().rgba, [3, 2, 1, 255]);
        let with_alpha = info_dib(1, 1, 32, BI_RGB, &[], &[1, 2, 3, 4]);
        assert_eq!(decode_dib(&with_alpha).unwrap().rgba, [3, 2, 1, 4]);
        // 5-6-5-style masks in 32 bits, scaled to 8 bits a channel.
        let masks: Vec<u8> = [0xf800u32, 0x07e0, 0x001f]
            .iter()
            .flat_map(|m| m.to_le_bytes())
            .collect();
        let fields = info_dib(1, 1, 32, BI_BITFIELDS, &masks, &0xffffu32.to_le_bytes());
        assert_eq!(decode_dib(&fields).unwrap().rgba, [255, 255, 255, 255]);
        let fields = info_dib(1, 1, 32, BI_BITFIELDS, &masks, &0x0800u32.to_le_bytes());
        assert_eq!(decode_dib(&fields).unwrap().rgba, [8, 0, 0, 255]);
    }

    #[test]
    fn a_v5_header_with_masks_written_after_it_too_is_read() {
        let picture = image(2, 2);
        let mut dib = encode_dibv5(&picture).unwrap();
        let masks: Vec<u8> = [0x00ff_0000u32, 0x0000_ff00, 0x0000_00ff]
            .iter()
            .flat_map(|m| m.to_le_bytes())
            .collect();
        dib.splice(124..124, masks);
        assert_eq!(decode_dib(&dib).unwrap(), picture);
    }

    #[test]
    fn oversized_images_are_refused_before_they_are_allocated() {
        let wide = info_dib(MAX_SIDE as i32 + 1, 1, 32, BI_RGB, &[], &[]);
        assert_eq!(decode_dib(&wide), Err(ImageError::TooLarge));
        let many = info_dib(8192, 8192, 32, BI_RGB, &[], &[]);
        assert_eq!(decode_dib(&many), Err(ImageError::TooLarge));
        // A PNG header claiming 16384 x 16384 with no data behind it.
        let mut png = encode_png(&image(1, 1)).unwrap();
        png[16..20].copy_from_slice(&MAX_SIDE.to_be_bytes());
        png[20..24].copy_from_slice(&MAX_SIDE.to_be_bytes());
        let crc = crc32fast::hash(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        assert_eq!(decode_png(&png), Err(ImageError::TooLarge));
    }

    #[test]
    fn each_limit_is_named_by_what_went_over_it() {
        let pixels = ImageError::TooLarge;
        assert!(pixels.too_large());
        assert!(
            pixels.to_string().contains("16384 pixels a side"),
            "{pixels}"
        );
        let bytes = ImageError::TooManyBytes {
            bytes: 20_000_000,
            max: 16 << 20,
        };
        assert!(bytes.too_large());
        assert_eq!(
            bytes.to_string(),
            "the image is 20000000 bytes as PNG, over the limit of 16777216 bytes"
        );
        assert!(!ImageError::NotPng.too_large());
        assert!(!ImageError::Malformed("cut short".into()).too_large());
    }

    #[test]
    fn a_decompression_bomb_is_refused_by_the_size_it_claims() {
        // 8192 x 8192 pixels compress to well under 1 MiB but would decode
        // to twice the limit. The decoder never writes more than the header
        // claims, so the size check bounds the decompression too.
        let encoder_bytes = {
            let mut out = Vec::new();
            let mut encoder = png::Encoder::new(&mut out, 8192, 8192);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&vec![0; 8192 * 8192]).unwrap();
            writer.finish().unwrap();
            out
        };
        assert!(encoder_bytes.len() < 1 << 20);
        assert_eq!(decode_png(&encoder_bytes), Err(ImageError::TooLarge));
    }

    #[test]
    fn bitmaps_this_cannot_read_are_named() {
        let palette = info_dib(1, 1, 8, BI_RGB, &[0; 4], &[0; 4]);
        assert!(matches!(
            decode_dib(&palette),
            Err(ImageError::Unsupported(_))
        ));
        let rle = info_dib(1, 1, 32, 1, &[], &[0; 4]);
        assert!(matches!(decode_dib(&rle), Err(ImageError::Unsupported(_))));
        let gaps: Vec<u8> = [0x00f0_0f00u32, 0xff, 0xff00]
            .iter()
            .flat_map(|m| m.to_le_bytes())
            .collect();
        let gaps = info_dib(1, 1, 32, BI_BITFIELDS, &gaps, &[0; 4]);
        assert!(matches!(decode_dib(&gaps), Err(ImageError::Unsupported(_))));
        assert_eq!(decode_png(b"GIF89a"), Err(ImageError::NotPng));
    }

    #[test]
    fn cut_short_or_corrupt_images_fail_without_panicking() {
        let dib = encode_dibv5(&image(4, 4)).unwrap();
        for end in 0..dib.len() {
            assert!(decode_dib(&dib[..end]).is_err(), "{end}");
        }
        let header = info_dib(1, 1, 32, BI_RGB, &[], &[0; 4]);
        for (at, value) in [
            (0, 0xffu8),
            (4, 0x80),
            (8, 0x80),
            (12, 0),
            (14, 0),
            (32, 0xff),
            (35, 0xff),
        ] {
            let mut bad = header.clone();
            bad[at] = value;
            if at == 35 {
                bad[32] = 0xff;
            }
            assert!(decode_dib(&bad).is_err(), "{at}");
        }
        let png = encode_png(&image(9, 7)).unwrap();
        let data = png.windows(4).position(|w| w == b"IDAT").unwrap() + 4;
        for end in 0..png.len() {
            // Without image data it fails; cut later, it must not panic.
            let decoded = decode_png(&png[..end]);
            assert!(end > data || decoded.is_err(), "{end}");
        }
        for at in 8..png.len() {
            let mut bad = png.clone();
            bad[at] ^= 0x5a;
            // Corruption is refused or, past a checksum the decoder skips, decodes.
            let _ = decode_png(&bad);
        }
    }
}
