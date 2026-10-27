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
        // SAFETY: `userdata` is the Handed<Prepared> boxed by `hand_over`, alive until `cleanup`.
        let image = unsafe { &(*userdata.cast::<Handed<Prepared>>()).value };
        if mime.is_null() || unsafe { CStr::from_ptr(mime) } != image.mime {
            unsafe { *size = 0 };
            return std::ptr::null();
        }
        unsafe { *size = image.data.len() };
        image.data.as_ptr().cast()
    }
    let types = [image.mime.as_ptr()];
    let mut error = String::new();
    let set = hand_over(
        image,
        |userdata| {
            // SAFETY: valid callbacks and NUL-terminated types; SDL frees `userdata` through
            // `cleanup` once it has taken it.
            let set = unsafe {
                SDL_SetClipboardData(
                    Some(provide),
                    Some(cleanup::<Prepared>),
                    userdata,
                    types.as_ptr(),
                    types.len(),
                )
            };
            if !set {
                error = crate::sdl::error();
            }
            set
        },
        // SAFETY: main thread, like the set.
        || unsafe {
            SDL_ClearClipboardData();
        },
    );
    if set {
        Ok(())
    } else if error.is_empty() {
        Err("the clipboard could not be set".into())
    } else {
        Err(error)
    }
}

/// What SDL holds as clipboard userdata: the value, and a flag its drop raises.
struct Handed<T> {
    value: T,
    dropped: std::rc::Rc<std::cell::Cell<bool>>,
}

impl<T> Drop for Handed<T> {
    fn drop(&mut self) {
        self.dropped.set(true);
    }
}

unsafe extern "C" fn cleanup<T>(userdata: *mut std::ffi::c_void) {
    // SAFETY: SDL calls this once, when the data is replaced or the clipboard cleared.
    drop(unsafe { Box::from_raw(userdata.cast::<Handed<T>>()) });
}

/// Hands `value` to `set` as clipboard userdata freed through [`cleanup`]. When setting fails,
/// SDL may or may not have kept it (it keeps it when the platform refused, and returns before
/// taking it on bad arguments or without video): `clear` makes SDL let go of what it kept,
/// through `cleanup`, and what it never took is freed here. Whether `set` succeeded.
fn hand_over<T>(
    value: T,
    set: impl FnOnce(*mut std::ffi::c_void) -> bool,
    clear: impl FnOnce(),
) -> bool {
    let dropped = std::rc::Rc::new(std::cell::Cell::new(false));
    let userdata = Box::into_raw(Box::new(Handed {
        value,
        dropped: dropped.clone(),
    }));
    if set(userdata.cast()) {
        return true;
    }
    clear();
    if !dropped.get() {
        // SAFETY: SDL returned before taking `userdata`, or `clear` would have freed it: it
        // can only fail to clear without video, when it could not have taken it either.
        drop(unsafe { Box::from_raw(userdata) });
    }
    false
}

fn encode_png(image: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(png)
}

const FILE_HEADER: usize = 14;
const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;
/// A BITMAPV4HEADER, the smallest header that holds its colour masks itself.
const V4_HEADER: usize = 108;

fn u32_at(bmp: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bmp.get(at..at + 4)?.try_into().ok()?))
}

fn decode_bmp(bmp: &[u8]) -> Result<RgbaImage, String> {
    let bmp = pixels_where_they_are(bmp);
    let mut image = image::load_from_memory_with_format(&bmp, image::ImageFormat::Bmp)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    reserved_byte_alpha(&bmp, &mut image);
    // A 32-bit DIB that Windows made from an opaque bitmap (a screenshot) may say it has alpha
    // and leave every alpha byte 0; read as written, it would arrive fully transparent.
    if image.pixels().all(|p| p[3] == 0) {
        image.pixels_mut().for_each(|p| p[3] = 255);
    }
    Ok(image)
}

/// `bmp` with its file header pointing at the pixels. SDL makes a BMP file of a Windows DIB by
/// putting the pixels 12 bytes after any BI_BITFIELDS header, for the three masks that follow a
/// BITMAPINFOHEADER. A V4 or V5 header holds its masks itself, and apps differ on whether they
/// repeat them after it: when those 12 bytes are not the header's masks again, they are the
/// first pixels.
fn pixels_where_they_are(bmp: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    let (Some(offset), Some(header), Some(compression)) = (
        u32_at(bmp, 10),
        u32_at(bmp, FILE_HEADER),
        u32_at(bmp, FILE_HEADER + 16),
    ) else {
        return bmp.into();
    };
    let header_end = FILE_HEADER + header as usize;
    if compression != BI_BITFIELDS
        || (header as usize) < V4_HEADER
        || offset as usize != header_end + 12
    {
        return bmp.into();
    }
    let masks = &bmp[FILE_HEADER + 40..FILE_HEADER + 52];
    if bmp.get(header_end..header_end + 12) == Some(masks) {
        return bmp.into();
    }
    let mut moved = bmp.to_vec();
    moved[10..14].copy_from_slice(&(header_end as u32).to_le_bytes());
    moved.into()
}

/// Takes the alpha of a 32-bit BI_RGB bitmap from the fourth byte of its pixels, which Windows
/// calls reserved and the decoder drops, when any pixel has some there: an app that writes
/// alpha there (as `bmp_v5` does) means it, and one that does not leaves it 0.
fn reserved_byte_alpha(bmp: &[u8], image: &mut RgbaImage) {
    let (Some(offset), Some(height)) = (u32_at(bmp, 10), u32_at(bmp, FILE_HEADER + 8)) else {
        return;
    };
    let bits = bmp.get(FILE_HEADER + 14..FILE_HEADER + 16);
    if bits != Some(&32u16.to_le_bytes()[..]) || u32_at(bmp, FILE_HEADER + 16) != Some(BI_RGB) {
        return;
    }
    let (width, rows) = image.dimensions();
    let stride = width as usize * 4;
    // The decoder read these bytes, so they are there.
    let Some(pixels) = bmp
        .get(offset as usize..)
        .and_then(|p| p.get(..stride * rows as usize))
    else {
        return;
    };
    if stride == 0 || pixels.chunks_exact(4).all(|p| p[3] == 0) {
        return;
    }
    let top_down = (height as i32) < 0;
    for (n, row) in pixels.chunks_exact(stride).enumerate() {
        let y = if top_down { n } else { rows as usize - 1 - n };
        for (x, p) in row.chunks_exact(4).enumerate() {
            image.get_pixel_mut(x as u32, y as u32)[3] = p[3];
        }
    }
}

/// A BMP file of `image` with a BITMAPV5HEADER: 32 bits per pixel, bottom-up, sRGB, which SDL
/// hands Windows as CF_DIBV5 (Windows makes CF_DIB and CF_BITMAP of it).
///
/// BI_RGB, not BI_BITFIELDS: SDL 3.4 reads a BI_BITFIELDS DIB back (WIN_ConvertDIBtoBMP) as if
/// three masks followed every header, which a V5 header's own masks do not, so the image would
/// fail as "Invalid BMP data" or come back three pixels off. Each pixel's fourth byte carries
/// its alpha, not premultiplied, and the header's alpha mask says so; Windows calls that byte
/// reserved in BI_RGB. Apps that go by Windows paste the image opaque, transparent pixels in
/// whatever colour they hold; apps that read the byte as alpha, this one included, keep the
/// transparency.
fn bmp_v5(image: &RgbaImage) -> Vec<u8> {
    const V5_HEADER: u32 = 124;
    let file_header = FILE_HEADER as u32;
    let (width, height) = image.dimensions();
    let pixels = width * height * 4;
    let mut out = Vec::with_capacity((file_header + V5_HEADER + pixels) as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(file_header + V5_HEADER + pixels).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(file_header + V5_HEADER).to_le_bytes());
    out.extend_from_slice(&V5_HEADER.to_le_bytes());
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&(height as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&BI_RGB.to_le_bytes());
    out.extend_from_slice(&pixels.to_le_bytes());
    out.extend_from_slice(&[0; 16]); // resolution, palette
                                     // The colour masks count only with BI_BITFIELDS; the alpha mask names the fourth byte.
    for mask in [0u32, 0, 0, 0xff00_0000] {
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
        assert_eq!(
            u32_at(&bmp, 30),
            Some(BI_RGB),
            "SDL misplaces BI_BITFIELDS pixels"
        );
        assert_eq!(decode_bmp(&bmp).unwrap(), picture());
    }

    /// SDL 3.4's WIN_ConvertBMPtoDIB: the DIB it hands Windows for a BMP file.
    fn sdl_bmp_to_dib(bmp: &[u8]) -> Result<Vec<u8>, &'static str> {
        let offset = u32_at(bmp, 10).unwrap() as usize;
        let header = u32_at(bmp, 14).unwrap() as usize;
        let colours = u32_at(bmp, 14 + 32).unwrap() as usize;
        let pixels = u32_at(bmp, 14 + 20).unwrap() as usize;
        let header_size = header + colours * 4;
        if offset < 14 + header_size || offset + pixels > bmp.len() {
            return Err("Invalid BMP data");
        }
        Ok([&bmp[14..14 + header_size], &bmp[offset..offset + pixels]].concat())
    }

    /// SDL 3.4's WIN_ConvertDIBtoBMP: the BMP file it makes of `dib`, held by Windows in a block
    /// of `global_size` bytes (GlobalAlloc rounds up, and the slack holds anything).
    fn sdl_dib_to_bmp(dib: &[u8], global_size: usize) -> Result<Vec<u8>, &'static str> {
        let mut block = dib.to_vec();
        block.resize(global_size, 0xa5);
        let header = u32_at(&block, 0).unwrap() as usize;
        let bits = u16::from_le_bytes(block[14..16].try_into().unwrap());
        let colours = u32_at(&block, 32).unwrap() as usize;
        let table = match u32_at(&block, 16).unwrap() {
            BI_RGB if bits <= 8 => 4 * if colours == 0 { 1 << bits } else { colours },
            BI_RGB => 0,
            BI_BITFIELDS => 12,
            6 => 16,
            _ => 4 * colours,
        };
        if header + table + u32_at(&block, 20).unwrap() as usize > global_size {
            return Err("Invalid BMP data");
        }
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&((14 + global_size) as u32).to_le_bytes());
        bmp.extend_from_slice(&[0; 4]);
        bmp.extend_from_slice(&((14 + header + table) as u32).to_le_bytes());
        bmp.extend_from_slice(&block);
        Ok(bmp)
    }

    #[test]
    fn a_bitmap_written_for_windows_survives_sdls_trip_through_the_clipboard() {
        let dib = sdl_bmp_to_dib(&bmp_v5(&picture())).unwrap();
        assert_eq!(dib.len(), 124 + 5 * 3 * 4);
        for slack in 0..16 {
            let back = sdl_dib_to_bmp(&dib, dib.len() + slack).unwrap();
            assert_eq!(
                decode_bmp(&back).unwrap(),
                picture(),
                "{slack} bytes of slack"
            );
        }
    }

    /// A V5 DIB with BI_BITFIELDS as other apps put it on the clipboard, with or without the
    /// masks repeated after the header.
    fn bitfields_dib(image: &RgbaImage, masks_after: bool) -> Vec<u8> {
        let bmp = bmp_v5(image);
        let mut dib = bmp[14..14 + 124].to_vec();
        dib[16..20].copy_from_slice(&BI_BITFIELDS.to_le_bytes());
        let masks = [0x00ff_0000u32, 0x0000_ff00, 0x0000_00ff];
        for (n, mask) in masks.iter().enumerate() {
            dib[40 + 4 * n..44 + 4 * n].copy_from_slice(&mask.to_le_bytes());
        }
        if masks_after {
            masks
                .iter()
                .for_each(|m| dib.extend_from_slice(&m.to_le_bytes()));
        }
        dib.extend_from_slice(&bmp[14 + 124..]);
        dib
    }

    #[test]
    fn bitfields_bitmaps_from_other_apps_decode_with_or_without_masks_after_the_header() {
        let with = bitfields_dib(&picture(), true);
        let back = sdl_dib_to_bmp(&with, with.len()).unwrap();
        assert_eq!(decode_bmp(&back).unwrap(), picture());

        // SDL puts the pixels 12 bytes on, into the slack Windows' block has past them.
        let without = bitfields_dib(&picture(), false);
        let back = sdl_dib_to_bmp(&without, without.len() + 12).unwrap();
        assert_eq!(u32_at(&back, 10), Some(14 + 124 + 12));
        assert_eq!(decode_bmp(&back).unwrap(), picture());
        // Without that slack SDL refuses it, and reading the clipboard reports its error.
        assert_eq!(
            sdl_dib_to_bmp(&without, without.len()),
            Err("Invalid BMP data")
        );

        // A BI_BITFIELDS bitmap with an alpha mask and every alpha 0 still reads opaque.
        let mut clear = picture();
        clear.pixels_mut().for_each(|p| p[3] = 0);
        let dib = bitfields_dib(&clear, false);
        let read = decode_bmp(&sdl_dib_to_bmp(&dib, dib.len() + 12).unwrap()).unwrap();
        assert!(read.pixels().all(|p| p[3] == 255));
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

    /// Counts its drops.
    struct Probe(std::rc::Rc<std::cell::Cell<u32>>);

    impl Drop for Probe {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn the_image_handed_to_sdl_is_freed_once_whether_or_not_setting_works() {
        let drops = std::rc::Rc::new(std::cell::Cell::new(0));
        // SDL returns before taking it: freed here.
        let cleared = std::cell::Cell::new(false);
        assert!(!hand_over(
            Probe(drops.clone()),
            |_| false,
            || cleared.set(true)
        ));
        assert!(cleared.get(), "SDL is told to let go");
        assert_eq!(drops.get(), 1);

        // SDL keeps it and the platform refuses: clearing frees it through the cleanup.
        let kept = std::cell::Cell::new(std::ptr::null_mut());
        let set = |userdata| {
            kept.set(userdata);
            false
        };
        // SAFETY: what SDL_ClearClipboardData does with the data it holds.
        let clear = || unsafe { cleanup::<Probe>(kept.get()) };
        assert!(!hand_over(Probe(drops.clone()), set, clear));
        assert_eq!(drops.get(), 2);

        // Set: SDL frees it, when the next set or SDL_Quit cancels it.
        assert!(hand_over(
            Probe(drops.clone()),
            |userdata| {
                kept.set(userdata);
                true
            },
            || panic!("nothing to clear")
        ));
        assert_eq!(drops.get(), 2);
        unsafe { cleanup::<Probe>(kept.get()) };
        assert_eq!(drops.get(), 3);
    }

    #[test]
    fn broken_images_fail_loudly() {
        assert!(to_png(Local::Bmp(b"BM broken".to_vec())).is_err());
        assert!(from_png(b"\x89PNG\r\n\x1a\n broken").is_err());
    }
}
