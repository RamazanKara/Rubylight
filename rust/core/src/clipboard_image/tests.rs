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
