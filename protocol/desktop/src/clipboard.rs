//! Images on this device's clipboard, as the PC's image clipboard takes them: PNG both ways.
//!
//! Reading takes `image/png` when the clipboard has one and a bitmap (`image/bmp`, Windows'
//! CF_DIB and CF_DIBV5) otherwise, which is converted to PNG. Writing puts a PNG from the PC on
//! the clipboard as Windows' apps read images: SDL sets only the first image type it is given,
//! so Windows gets a 32-bit CF_DIBV5 (Windows makes CF_DIB and CF_BITMAP of it) and the other
//! systems the PNG itself. The SDL calls run on the main thread; the conversions are for a
//! worker, since a large image takes a while.

use std::ffi::CStr;

use image::RgbaImage;
use sdl3_sys::everything::*;

/// What the clipboard here holds, as it came.
#[derive(Debug, PartialEq, Eq)]
pub enum Local {
    Png(Vec<u8>),
    /// A BMP file: SDL puts a file header before Windows' DIB.
    Bmp(Vec<u8>),
}

/// An image ready for the clipboard here: its type and bytes.
#[derive(Debug)]
pub struct Prepared {
    mime: &'static CStr,
    data: Vec<u8>,
}

#[cfg(windows)]
const WRITE_TYPE: &CStr = c"image/bmp";
#[cfg(not(windows))]
const WRITE_TYPE: &CStr = c"image/png";

/// The image on the clipboard, or `None` while it holds none. Main thread.
pub fn read() -> Result<Option<Local>, String> {
    for (mime, png) in [(c"image/png", true), (c"image/bmp", false)] {
        // SAFETY: NUL-terminated MIME types; SDL returns a buffer of `size` bytes we free.
        unsafe {
            if !SDL_HasClipboardData(mime.as_ptr()) {
                continue;
            }
            SDL_ClearError();
            let mut size = 0;
            let data = SDL_GetClipboardData(mime.as_ptr(), &mut size);
            if data.is_null() {
                let error = crate::sdl::error();
                return Err(if error.is_empty() {
                    "the clipboard could not be opened".into()
                } else {
                    error
                });
            }
            let bytes = std::slice::from_raw_parts(data.cast::<u8>(), size).to_vec();
            SDL_free(data);
            if bytes.is_empty() {
                return Err(format!(
                    "the clipboard's {} is empty",
                    mime.to_string_lossy()
                ));
            }
            return Ok(Some(if png {
                Local::Png(bytes)
            } else {
                Local::Bmp(bytes)
            }));
        }
    }
    Ok(None)
}

/// The PNG to send for what the clipboard held: a PNG as it is (the PC checks it), a bitmap
/// converted.
pub fn to_png(local: Local) -> Result<Vec<u8>, String> {
    match local {
        Local::Png(png) => Ok(png),
        Local::Bmp(bmp) => encode_png(&decode_bmp(&bmp)?),
    }
}

/// `png` from the PC, as the clipboard here takes it.
pub fn from_png(png: &[u8]) -> Result<Prepared, String> {
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    if cfg!(windows) {
        Ok(Prepared {
            mime: WRITE_TYPE,
            data: bmp_v5(&image.to_rgba8()),
        })
    } else {
        Ok(Prepared {
            mime: WRITE_TYPE,
            data: png.to_vec(),
        })
    }
}

/// Puts `image` on the clipboard. Main thread.
pub fn write(image: Prepared) -> Result<(), String> {
    unsafe extern "C" fn provide(
        userdata: *mut std::ffi::c_void,
        mime: *const std::ffi::c_char,
        size: *mut usize,
    ) -> *const std::ffi::c_void {
        // SAFETY: `userdata` is the Prepared boxed below, alive until `cleanup`.
        let image = unsafe { &*userdata.cast::<Prepared>() };
        if mime.is_null() || unsafe { CStr::from_ptr(mime) } != image.mime {
            unsafe { *size = 0 };
            return std::ptr::null();
        }
        unsafe { *size = image.data.len() };
        image.data.as_ptr().cast()
    }
    unsafe extern "C" fn cleanup(userdata: *mut std::ffi::c_void) {
        // SAFETY: SDL calls this once, when the data is replaced or the clipboard cleared.
        drop(unsafe { Box::from_raw(userdata.cast::<Prepared>()) });
    }
    let types = [image.mime.as_ptr()];
    let userdata = Box::into_raw(Box::new(image));
    // SAFETY: SDL owns `userdata` from here on and frees it through `cleanup`, also when
    // setting fails (the next set or SDL_Quit cancels it).
    if unsafe {
        SDL_SetClipboardData(
            Some(provide),
            Some(cleanup),
            userdata.cast(),
            types.as_ptr(),
            types.len(),
        )
    } {
        Ok(())
    } else {
        Err(crate::sdl::error())
    }
}

fn encode_png(image: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(png)
}

fn decode_bmp(bmp: &[u8]) -> Result<RgbaImage, String> {
    let mut image = image::load_from_memory_with_format(bmp, image::ImageFormat::Bmp)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    // A 32-bit DIB that Windows made from an opaque bitmap (a screenshot) may say it has alpha
    // and leave every alpha byte 0; read as written, it would arrive fully transparent.
    if image.pixels().all(|p| p[3] == 0) {
        image.pixels_mut().for_each(|p| p[3] = 255);
    }
    Ok(image)
}

/// A BMP file of `image` with a BITMAPV5HEADER: 32 bits per pixel, bottom-up, sRGB, with an
/// alpha mask, which SDL hands Windows as CF_DIBV5.
fn bmp_v5(image: &RgbaImage) -> Vec<u8> {
    const FILE_HEADER: u32 = 14;
    const V5_HEADER: u32 = 124;
    let (width, height) = image.dimensions();
    let pixels = width * height * 4;
    let mut out = Vec::with_capacity((FILE_HEADER + V5_HEADER + pixels) as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(FILE_HEADER + V5_HEADER + pixels).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(FILE_HEADER + V5_HEADER).to_le_bytes());
    out.extend_from_slice(&V5_HEADER.to_le_bytes());
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&(height as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&3u32.to_le_bytes()); // BI_BITFIELDS
    out.extend_from_slice(&pixels.to_le_bytes());
    out.extend_from_slice(&[0; 16]); // resolution, palette
    for mask in [0x00ff_0000u32, 0x0000_ff00, 0x0000_00ff, 0xff00_0000] {
        out.extend_from_slice(&mask.to_le_bytes());
    }
    out.extend_from_slice(b"BGRs"); // LCS_sRGB, little-endian
    out.extend_from_slice(&[0; 36 + 12]); // endpoints, gamma
    out.extend_from_slice(&4u32.to_le_bytes()); // LCS_GM_IMAGES
    out.extend_from_slice(&[0; 12]); // profile, reserved
    for row in image.rows().rev() {
        for p in row {
            out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture() -> RgbaImage {
        RgbaImage::from_fn(5, 3, |x, y| {
            image::Rgba([
                x as u8 * 50,
                y as u8 * 80,
                7,
                if x == 0 { 0 } else { 128 + y as u8 },
            ])
        })
    }

    #[test]
    fn a_bitmap_written_for_windows_reads_back_pixel_exact() {
        let bmp = bmp_v5(&picture());
        // What SDL checks before handing the DIB to Windows, and what makes it CF_DIBV5.
        assert_eq!(&bmp[..2], b"BM");
        assert_eq!(
            u32::from_le_bytes(bmp[2..6].try_into().unwrap()) as usize,
            bmp.len()
        );
        assert_eq!(
            u32::from_le_bytes(bmp[10..14].try_into().unwrap()),
            14 + 124
        );
        assert_eq!(u32::from_le_bytes(bmp[14..18].try_into().unwrap()), 124);
        assert_eq!(
            u32::from_le_bytes(bmp[34..38].try_into().unwrap()),
            5 * 3 * 4
        );
        assert_eq!(decode_bmp(&bmp).unwrap(), picture());
    }

    #[test]
    fn a_bitmap_on_the_clipboard_goes_to_the_pc_as_the_same_pixels_in_a_png() {
        let png = to_png(Local::Bmp(bmp_v5(&picture()))).unwrap();
        assert!(png.starts_with(nightjar_core::http::PNG_SIGNATURE));
        assert_eq!(image::load_from_memory(&png).unwrap().to_rgba8(), picture());
        // A PNG goes as it is; the PC, not this side, says whether it is one.
        assert_eq!(
            to_png(Local::Png(b"not checked here".to_vec())).unwrap(),
            b"not checked here"
        );
    }

    #[test]
    fn an_opaque_bitmap_with_every_alpha_zero_stays_opaque() {
        let mut clear = picture();
        clear.pixels_mut().for_each(|p| p[3] = 0);
        let read = decode_bmp(&bmp_v5(&clear)).unwrap();
        assert!(read.pixels().all(|p| p[3] == 255));
        assert_eq!(
            read.pixels()
                .map(|p| [p[0], p[1], p[2]])
                .collect::<Vec<_>>(),
            clear
                .pixels()
                .map(|p| [p[0], p[1], p[2]])
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_png_from_the_pc_becomes_what_this_systems_clipboard_takes() {
        let png = encode_png(&picture()).unwrap();
        let prepared = from_png(&png).unwrap();
        assert_eq!(prepared.mime, WRITE_TYPE);
        if cfg!(windows) {
            assert_eq!(decode_bmp(&prepared.data).unwrap(), picture());
        } else {
            assert_eq!(prepared.data, png);
        }
    }

    #[test]
    fn broken_images_fail_loudly() {
        assert!(to_png(Local::Bmp(b"BM broken".to_vec())).is_err());
        assert!(from_png(b"\x89PNG\r\n\x1a\n broken").is_err());
    }
}
