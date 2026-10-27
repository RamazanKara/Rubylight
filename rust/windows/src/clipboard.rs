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
/// rather than adding up. Never held with the clipboard open.
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
pub fn read() -> Result<String> {
    let _lock = LOCK.lock().unwrap();
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
    let _lock = LOCK.lock().unwrap();
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
/// `max_bytes` fails with [`ImageError::TooLarge`], one this can't read with
/// [`ImageError::Unsupported`].
pub fn read_png(max_bytes: usize) -> Result<Option<Vec<u8>>> {
    let png = png_format()?;
    let _lock = LOCK.lock().unwrap();
    let _open = Open::new()?;
    let fits = |bytes: Vec<u8>| -> Result<Vec<u8>> {
        if bytes.len() > max_bytes {
            Err(ImageError::TooLarge.into())
        } else {
            Ok(bytes)
        }
    };
    let passed = with_data(png, |bytes| {
        let end = clipboard_image::png_extent(bytes)
            .ok_or_else(|| ImageError::Malformed("the clipboard's PNG is cut short".into()))?;
        fits(bytes[..end].to_vec())
    });
    match passed {
        Ok(Some(bytes)) => return Ok(Some(bytes)),
        Ok(None) => {}
        // A broken PNG is no reason to drop the bitmap beside it.
        Err(error) if error.downcast_ref::<ImageError>() != Some(&ImageError::TooLarge) => {
            tracing::warn!("clipboard: the PNG format is unusable, reading the bitmap: {error:#}");
        }
        Err(error) => return Err(error),
    }
    for format in [CF_DIBV5, CF_DIB] {
        let converted = with_data(format, |bytes| {
            let image = clipboard_image::decode_dib(bytes)?;
            fits(clipboard_image::encode_png(&image)?)
        })?;
        if converted.is_some() {
            return Ok(converted);
        }
    }
    Ok(None)
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
    let _lock = LOCK.lock().unwrap();
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
                Some(ImageError::TooLarge)
            ));
            // Without the PNG format, the bitmap is converted.
            {
                let _lock = LOCK.lock().unwrap();
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
