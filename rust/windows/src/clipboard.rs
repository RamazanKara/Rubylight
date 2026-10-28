//! The Windows clipboard: text (`CF_UNICODETEXT`) and images, which travel
//! as PNG and are kept here as `CF_DIBV5` and the registered `PNG` format
//! (`butterpollo_core::clipboard_image`).
use anyhow::{Result, bail};
use butterpollo_core::clipboard_image::{self, ImageError};
use std::sync::{Mutex, MutexGuard, PoisonError};
use windows::{
    Win32::{
        Foundation::*,
        System::{DataExchange::*, Memory::*},
        UI::WindowsAndMessaging::*,
    },
    core::w,
};
const MAX_BYTES: usize = 1024 * 1024;
const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;
const CF_DIBV5: u32 = 17;
static LOCK: Mutex<()> = Mutex::new(());
/// Held while an image is decoded or encoded: each can take hundreds of MiB
/// at the limits (`clipboard_image::MAX_PIXELS`), so requests take turns
/// rather than adding up. Never held with the clipboard open. The host also
/// lets one image request in at a time, so their copies and bodies don't add
/// up either.
static CONVERT: Mutex<()> = Mutex::new(());
/// The lock guards no data, so one a panic poisoned still serializes.
fn lock(mutex: &Mutex<()>) -> MutexGuard<'_, ()> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
/// Runs `work`, an image conversion, after any other has finished.
fn converting<T>(work: impl FnOnce() -> T) -> T {
    let _turn = lock(&CONVERT);
    work()
}
struct Open(HWND);
impl Open {
    fn new() -> Result<Self> {
        // SAFETY: the window class and title are static literals, and the clipboard is opened only
        // for the message window created here, which is destroyed if that fails.
        unsafe {
            let owner = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("Rubylight clipboard"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )?;
            for _ in 0..10 {
                if OpenClipboard(Some(owner)).is_ok() {
                    return Ok(Self(owner));
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let _ = DestroyWindow(owner);
            bail!("clipboard is busy")
        }
    }
}
impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: this value exists only while the clipboard is open for `self.0`, which it owns.
        unsafe {
            let _ = CloseClipboard();
            let _ = DestroyWindow(self.0);
        }
    }
}
/// The clipboard's change count, which every new content raises
/// (`GetClipboardSequenceNumber`), read without opening it.
pub fn sequence() -> u32 {
    // SAFETY: takes no arguments and only reads the window station's counter.
    unsafe { GetClipboardSequenceNumber() }
}
pub fn read() -> Result<String> {
    let _lock = lock(&LOCK);
    let _open = Open::new()?;
    // SAFETY: the clipboard stays open for `_open`, so the handle stays valid, and the locked
    // memory is read only within its GlobalSize bytes before it is unlocked.
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT).is_err() {
            return Ok(String::new());
        }
        let handle = GetClipboardData(CF_UNICODETEXT)?;
        let global = HGLOBAL(handle.0);
        let size = GlobalSize(global);
        if size > MAX_BYTES || !size.is_multiple_of(2) {
            bail!("clipboard text exceeds the limit");
        }
        let pointer = GlobalLock(global).cast::<u16>();
        if pointer.is_null() {
            bail!("cannot lock clipboard text");
        }
        let text = std::slice::from_raw_parts(pointer, size / 2);
        let end = text.iter().position(|c| *c == 0).unwrap_or(text.len());
        let result = String::from_utf16(&text[..end]);
        let _ = GlobalUnlock(global);
        Ok(result?)
    }
}
pub fn write(text: &str) -> Result<()> {
    if text.len() > MAX_BYTES / 2 || text.contains('\0') {
        bail!("invalid clipboard text size or NUL character");
    }
    let text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let bytes: Vec<u8> = text.iter().flat_map(|unit| unit.to_le_bytes()).collect();
    let _lock = lock(&LOCK);
    let _open = Open::new()?;
    // SAFETY: the clipboard is open for `_open`.
    unsafe { EmptyClipboard()? };
    set(CF_UNICODETEXT, &bytes)
}

/// The registered `PNG` clipboard format, which browsers and Office read
/// and write besides the bitmap.
fn png_format() -> Result<u32> {
    // SAFETY: the name is a static NUL-terminated literal.
    match unsafe { RegisterClipboardFormatW(w!("PNG")) } {
        0 => Err(windows::core::Error::from_thread().into()),
        format => Ok(format),
    }
}

/// Puts `bytes` on the open clipboard as `format`.
fn set(format: u32, bytes: &[u8]) -> Result<()> {
    // SAFETY: the caller holds the clipboard open; `global` is allocated for
    // `bytes.len()` bytes, so the copy fits; the system owns it once
    // SetClipboardData succeeds, and it is freed here only on failure.
    unsafe {
        let global = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
        let pointer = GlobalLock(global).cast::<u8>();
        if pointer.is_null() {
            let _ = GlobalFree(Some(global));
            bail!("cannot allocate clipboard data");
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len());
        let _ = GlobalUnlock(global);
        let result = SetClipboardData(format, Some(HANDLE(global.0)));
        if result.is_err() {
            let _ = GlobalFree(Some(global));
        }
        result?;
        Ok(())
    }
}

/// Calls `read` with the bytes of `format` on the open clipboard, or
/// returns `None` while it doesn't hold that format.
fn with_data<T>(format: u32, read: impl FnOnce(&[u8]) -> Result<T>) -> Result<Option<T>> {
    // SAFETY: the caller holds the clipboard open, so the handle stays valid,
    // and the locked memory is read only within its GlobalSize bytes before
    // it is unlocked.
    unsafe {
        if IsClipboardFormatAvailable(format).is_err() {
            return Ok(None);
        }
        let global = HGLOBAL(GetClipboardData(format)?.0);
        let size = GlobalSize(global);
        let pointer = GlobalLock(global).cast::<u8>();
        if pointer.is_null() {
            bail!("cannot lock the clipboard's format {format}");
        }
        let result = read(std::slice::from_raw_parts(pointer, size));
        let _ = GlobalUnlock(global);
        result.map(Some)
    }
}

/// The clipboard's image as PNG of at most `max_bytes`, or `None` while it
/// holds none. A `PNG` another program put there is passed on as it is,
/// without anything after its end; a bitmap is converted. An image over
/// `max_bytes` fails with [`ImageError::TooManyBytes`], one over the pixel
/// limits with [`ImageError::TooLarge`], one this can't read with
/// [`ImageError::Unsupported`].
pub fn read_png(max_bytes: usize) -> Result<Option<Vec<u8>>> {
    image_png(png_format()?, max_bytes, copied)
}

/// Copies what is needed out of a format's bytes while the clipboard is open.
type Take<'a> = &'a dyn Fn(&[u8]) -> Result<Vec<u8>>;

/// The bytes `take` copies out of the clipboard's `format`, or `None` while
/// it doesn't hold that format. The clipboard is open, and [`LOCK`] held,
/// only while they are copied.
fn copied(format: u32, take: Take<'_>) -> Result<Option<Vec<u8>>> {
    let _lock = lock(&LOCK);
    let _open = Open::new()?;
    with_data(format, take)
}

/// [`read_png`] with the clipboard's formats from `copy`, as [`copied`]
/// returns them, so a bitmap is converted with the clipboard closed and
/// unlocked again.
fn image_png(
    png: u32,
    max_bytes: usize,
    mut copy: impl FnMut(u32, Take<'_>) -> Result<Option<Vec<u8>>>,
) -> Result<Option<Vec<u8>>> {
    let over = move |bytes: usize| ImageError::TooManyBytes {
        bytes,
        max: max_bytes,
    };
    let fits = move |bytes: Vec<u8>| -> Result<Vec<u8>> {
        if bytes.len() > max_bytes {
            Err(over(bytes.len()).into())
        } else {
            Ok(bytes)
        }
    };
    let passed = copy(png, &|bytes| {
        let end = clipboard_image::png_extent(bytes)
            .ok_or_else(|| ImageError::Malformed("the clipboard's PNG is cut short".into()))?;
        if end > max_bytes {
            return Err(over(end).into());
        }
        Ok(bytes[..end].to_vec())
    });
    let too_large = |error: &anyhow::Error| {
        error
            .downcast_ref::<ImageError>()
            .is_some_and(ImageError::too_large)
    };
    // What made a format unusable, failed with when no later one is there.
    let mut unusable = None;
    match passed {
        Ok(Some(bytes)) => return Ok(Some(bytes)),
        Ok(None) => {}
        // A broken PNG is no reason to drop the bitmap beside it.
        Err(error) if !too_large(&error) => {
            tracing::warn!("clipboard: the PNG format is unusable, reading the bitmap: {error:#}");
            unusable = Some(error);
        }
        Err(error) => return Err(error),
    }
    for format in [CF_DIBV5, CF_DIB] {
        // Longer than any bitmap within the limits: refused before a copy.
        let read = copy(format, &|bytes| {
            if bytes.len() > clipboard_image::MAX_DIB_BYTES {
                Err(ImageError::TooLarge.into())
            } else {
                Ok(bytes.to_vec())
            }
        })
        .and_then(|dib| {
            dib.map(|dib| {
                converting(move || {
                    let image = clipboard_image::decode_dib(&dib)?;
                    drop(dib);
                    fits(clipboard_image::encode_png(&image)?)
                })
            })
            .transpose()
        });
        match read {
            Ok(Some(png)) => return Ok(Some(png)),
            Ok(None) => {}
            // Windows makes CF_DIB from CF_DIBV5 with another header, which
            // can be one this reads.
            Err(error) if format == CF_DIBV5 && !too_large(&error) => {
                tracing::warn!("clipboard: CF_DIBV5 is unusable, reading CF_DIB: {error:#}");
                unusable = Some(error);
            }
            Err(error) => return Err(error),
        }
    }
    unusable.map_or(Ok(None), Err)
}

/// Puts the PNG `png` on the clipboard as a `CF_DIBV5` bitmap (Windows adds
/// `CF_DIB` and `CF_BITMAP` from it) and as `PNG`, re-encoded from the
/// decoded pixels, so nothing but them reaches other programs.
pub fn write_png(png: &[u8]) -> Result<()> {
    // The PNG first, while only the pixels are held besides it.
    let (png, dib) = converting(|| -> Result<_> {
        let image = clipboard_image::decode_png(png)?;
        let png = clipboard_image::encode_png(&image)?;
        Ok((png, clipboard_image::encode_dibv5(&image)?))
    })?;
    let format = png_format()?;
    let _lock = lock(&LOCK);
    let _open = Open::new()?;
    // SAFETY: the clipboard is open for `_open`.
    unsafe { EmptyClipboard()? };
    set(CF_DIBV5, &dib)?;
    set(format, &png)
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterpollo_core::clipboard_image::Image;
    use std::sync::mpsc;

    /// A clipboard of `formats`, copied out as [`copied`] would, with the
    /// formats asked for in order.
    fn fake(
        formats: Vec<(u32, Vec<u8>)>,
        asked: &mut Vec<u32>,
    ) -> impl FnMut(u32, Take<'_>) -> Result<Option<Vec<u8>>> + '_ {
        move |format, take| {
            asked.push(format);
            let _lock = lock(&LOCK);
            formats
                .iter()
                .find(|(held, _)| *held == format)
                .map(|(_, bytes)| take(bytes))
                .transpose()
        }
    }
    const PNG: u32 = 0xc000;

    fn picture() -> Image {
        Image {
            width: 3,
            height: 2,
            rgba: (0..24).map(|n| n * 10).collect(),
        }
    }

    #[test]
    fn a_bitmap_is_copied_out_and_converted_after_the_clipboard_closes() {
        let image = picture();
        let dib = clipboard_image::encode_dibv5(&image).unwrap();
        let mut asked = Vec::new();
        let png = image_png(PNG, 1 << 20, fake(vec![(CF_DIBV5, dib)], &mut asked))
            .unwrap()
            .expect("an image");
        assert_eq!(clipboard_image::decode_png(&png).unwrap(), image);
        assert_eq!(asked, [PNG, CF_DIBV5]);
        // A PNG there passes without what follows it, within the limit.
        let held = clipboard_image::encode_png(&image).unwrap();
        let held_len = held.len();
        let mut padded = held.clone();
        padded.extend_from_slice(&[0; 64]);
        let mut asked = Vec::new();
        let read = image_png(PNG, 1 << 20, fake(vec![(PNG, padded.clone())], &mut asked));
        assert_eq!(read.unwrap(), Some(held));
        // Over the byte limit, the size and the limit are named, not the
        // pixel limits.
        let too_large = image_png(PNG, 16, fake(vec![(PNG, padded)], &mut asked)).unwrap_err();
        assert_eq!(
            too_large.downcast_ref(),
            Some(&ImageError::TooManyBytes {
                bytes: held_len,
                max: 16
            })
        );
        let message = too_large.to_string();
        assert!(message.contains(&format!("{held_len} bytes")), "{message}");
        assert!(!message.contains("pixels"), "{message}");
        // So is a converted bitmap's PNG over it, without trying CF_DIB.
        let mut asked = Vec::new();
        let dib = clipboard_image::encode_dibv5(&image).unwrap();
        let converted = image_png(
            PNG,
            16,
            fake(vec![(CF_DIBV5, dib.clone()), (CF_DIB, dib)], &mut asked),
        )
        .unwrap_err();
        assert!(matches!(
            converted.downcast_ref(),
            Some(ImageError::TooManyBytes { max: 16, .. })
        ));
        assert_eq!(asked, [PNG, CF_DIBV5]);
        assert_eq!(
            image_png(PNG, 1 << 20, fake(vec![], &mut asked)).unwrap(),
            None
        );
    }

    #[test]
    fn an_unusable_dibv5_falls_through_to_cf_dib_and_is_reported_without_one() {
        let image = picture();
        let dib = clipboard_image::encode_dibv5(&image).unwrap();
        // 8 bits a pixel: a palette this doesn't read.
        let mut palette = dib.clone();
        palette[14] = 8;
        let mut asked = Vec::new();
        let png = image_png(
            PNG,
            1 << 20,
            fake(
                vec![(CF_DIBV5, palette.clone()), (CF_DIB, dib.clone())],
                &mut asked,
            ),
        )
        .unwrap()
        .expect("the CF_DIB image");
        assert_eq!(clipboard_image::decode_png(&png).unwrap(), image);
        assert_eq!(asked, [PNG, CF_DIBV5, CF_DIB]);
        // So does a CF_DIBV5 Windows fails to give.
        let png = image_png(PNG, 1 << 20, |format, take: Take<'_>| match format {
            CF_DIBV5 => bail!("GetClipboardData failed"),
            CF_DIB => take(&dib).map(Some),
            _ => Ok(None),
        })
        .unwrap();
        assert!(png.is_some());
        // With nothing after it, what was wrong is the answer, not "no image".
        let error =
            image_png(PNG, 1 << 20, fake(vec![(CF_DIBV5, palette)], &mut asked)).unwrap_err();
        assert!(matches!(
            error.downcast_ref(),
            Some(ImageError::Unsupported(_))
        ));
        let error = image_png(
            PNG,
            1 << 20,
            fake(vec![(PNG, b"\x89PNG\r\n".to_vec())], &mut asked),
        )
        .unwrap_err();
        assert!(matches!(
            error.downcast_ref(),
            Some(ImageError::Malformed(_))
        ));
    }

    #[test]
    fn a_bitmap_longer_than_any_within_the_limits_is_not_copied() {
        let huge = vec![0u8; clipboard_image::MAX_DIB_BYTES + 1];
        let copied = std::cell::Cell::new(false);
        let error = image_png(PNG, 1 << 20, |format, take: Take<'_>| {
            if format != CF_DIBV5 {
                return Ok(None);
            }
            let bytes = take(&huge)?;
            copied.set(true);
            Ok(Some(bytes))
        })
        .unwrap_err();
        assert_eq!(error.downcast_ref(), Some(&ImageError::TooLarge));
        assert!(!copied.get());
    }

    #[test]
    fn a_panic_holding_the_clipboard_lock_does_not_end_clipboard_access() {
        let panicked = std::thread::spawn(|| {
            let _lock = lock(&LOCK);
            panic!("a conversion failed");
        })
        .join();
        assert!(panicked.is_err());
        assert!(LOCK.is_poisoned());
        drop(lock(&LOCK));
        let _ = std::thread::spawn(|| converting(|| panic!("a conversion failed"))).join();
        assert_eq!(converting(|| 7), 7);
    }

    #[test]
    fn image_conversions_take_turns() {
        let order = Mutex::new(Vec::new());
        let (entered, first_inside) = mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                converting(|| {
                    entered.send(()).unwrap();
                    // Long enough for the second to reach its turn.
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    order.lock().unwrap().push("first ends");
                })
            });
            first_inside.recv().unwrap();
            scope.spawn(|| converting(|| order.lock().unwrap().push("second starts")));
        });
        assert_eq!(*order.lock().unwrap(), ["first ends", "second starts"]);
    }

    /// Replaces the clipboard of the PC it runs on (its text is put back), so
    /// it runs only on request: tools/hw/catalog.yaml `clipboard-image-desktop`.
    #[test]
    #[ignore = "uses the real clipboard"]
    fn images_round_trip_through_the_real_clipboard() {
        let saved = read().unwrap();
        let rgba = (0..48u32 * 32)
            .flat_map(|n| {
                [
                    n as u8,
                    (n >> 4) as u8,
                    200,
                    if n % 5 == 0 { 64 } else { 255 },
                ]
            })
            .collect();
        let image = Image {
            width: 48,
            height: 32,
            rgba,
        };
        let png = clipboard_image::encode_png(&image).unwrap();
        let result = (|| -> Result<()> {
            write_png(&png)?;
            let read = read_png(1 << 20)?.expect("an image");
            assert_eq!(clipboard_image::decode_png(&read)?, image);
            assert!(matches!(
                read_png(16).unwrap_err().downcast_ref::<ImageError>(),
                Some(ImageError::TooManyBytes { max: 16, .. })
            ));
            // Without the PNG format, the bitmap is converted.
            {
                let _lock = lock(&LOCK);
                let _open = Open::new()?;
                // SAFETY: the clipboard is open for `_open`.
                unsafe { EmptyClipboard()? };
                set(CF_DIBV5, &clipboard_image::encode_dibv5(&image)?)?;
            }
            let read = read_png(1 << 20)?.expect("an image");
            assert_eq!(clipboard_image::decode_png(&read)?, image);
            write("no image")?;
            assert_eq!(read_png(1 << 20)?, None);
            Ok(())
        })();
        write(&saved).unwrap();
        result.unwrap();
    }
}
