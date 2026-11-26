//! The setup dialog dump: shows setup's real task dialogs one by one, captures each
//! with PrintWindow and closes it. `RUBYLIGHT_SETUP_DUMP_DIR=<folder>` turns it on;
//! it needs an interactive desktop.
use super::*;
use windows::Win32::{
    Foundation::RECT,
    Graphics::{
        Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
        Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC,
            DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject,
        },
    },
    Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow},
    UI::WindowsAndMessaging::{GetWindowRect, PostMessageW, WM_CLOSE},
};

/// Where the dialog on screen now goes.
static TARGET: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

/// The window's visible frame as top-down RGBA rows.
unsafe fn grab(window: HWND) -> Option<(u32, u32, Vec<u8>)> {
    // SAFETY: the dialog is alive until WM_CLOSE is posted after this returns; every DC
    // and bitmap made here is released once, and GetDIBits writes within `pixels`.
    unsafe {
        let mut outer = RECT::default();
        GetWindowRect(window, &mut outer).ok()?;
        let mut frame = outer;
        let _ = DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut frame as *mut RECT).cast(),
            size_of::<RECT>() as u32,
        );
        let (width, height) = (outer.right - outer.left, outer.bottom - outer.top);
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let old = SelectObject(dc, bitmap.into());
        // PW_RENDERFULLCONTENT: the title bar and DWM-drawn content too.
        let printed = PrintWindow(window, dc, PRINT_WINDOW_FLAGS(2)).as_bool();
        SelectObject(dc, old);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let lines = GetDIBits(
            dc,
            bitmap,
            0,
            height as u32,
            Some(pixels.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        if !printed || lines != height {
            return None;
        }
        // Crop the invisible resize borders around the visible frame.
        let (left, top) = (frame.left - outer.left, frame.top - outer.top);
        let (w, h) = (frame.right - frame.left, frame.bottom - frame.top);
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in top..top + h {
            for x in left..left + w {
                let p = &pixels[((y * width + x) * 4) as usize..][..4];
                rgba.extend_from_slice(&[p[2], p[1], p[0], 0xff]);
            }
        }
        Some((w as u32, h as u32, rgba))
    }
}

fn capture(window: isize) {
    std::thread::spawn(move || {
        // Let the dialog paint, its icon and title bar included.
        std::thread::sleep(std::time::Duration::from_millis(800));
        let hwnd = HWND(window as *mut _);
        // SAFETY: the dialog stays open until the WM_CLOSE posted below.
        let grabbed = unsafe { grab(hwnd) };
        if let (Some((width, height, rgba)), Some(path)) = (grabbed, TARGET.lock().unwrap().take())
        {
            let file = std::fs::File::create(&path).unwrap();
            let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&rgba)
                .unwrap();
        }
        // SAFETY: an integer-only message to a window that may already be gone.
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    });
}

#[test]
fn setup_dialog_dump() {
    let Some(folder) = std::env::var_os("RUBYLIGHT_SETUP_DUMP_DIR") else {
        return;
    };
    let folder = std::path::PathBuf::from(folder);
    std::fs::create_dir_all(&folder).unwrap();
    crate::ui_theme::per_monitor_dpi();
    let summary = "Pair a device and add apps at https://localhost:47990.\nInstalled in C:\\Program Files\\Rubylight.";
    for (theme, dark) in [("light", false), ("dark", true)] {
        *DUMP_HOOK.lock().unwrap() = Some((dark, capture as fn(isize)));
        let shot = |name: &str| {
            *TARGET.lock().unwrap() = Some(folder.join(format!("setup-{name}-{theme}.png")));
        };
        shot("question");
        ask(
            crate::TITLE,
            "Install Rubylight 7.0.0",
            "Rubylight installs in C:\\Program Files\\Rubylight and replaces Vibepollo, keeping its settings and paired devices.",
            "Install Rubylight",
            Some("Change folder…"),
            Some(("Install the virtual gamepad driver for controllers", true)),
        );
        shot("finish");
        finished(
            crate::TITLE,
            crate::finish_qr::HEADING,
            &crate::finish_qr::text(summary, crate::finish_qr::Host::Ready),
            true,
            Some(crate::finish_qr::OPEN_CONSOLE),
        );
        shot("finish-starting");
        finished(
            crate::TITLE,
            crate::finish_qr::HEADING,
            &crate::finish_qr::text(summary, crate::finish_qr::Host::Starting),
            true,
            Some(crate::finish_qr::OPEN_CONSOLE),
        );
        shot("error");
        finished(
            crate::TITLE,
            "Rubylight could not be installed",
            "The Rubylight service did not start.\n\nSetup log: C:\\Users\\you\\AppData\\Local\\Temp\\rubylight-setup.log",
            false,
            None,
        );
    }
    *DUMP_HOOK.lock().unwrap() = None;
}
