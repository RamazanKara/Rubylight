//! The tray's HICONs: the brand icon loaded at the size Windows asks for and one
//! badged copy per status, composited by [`super::status`] through GDI.
use super::status::{Status, composite};
use std::mem::size_of;
use windows::{
    Win32::{
        Foundation::{HINSTANCE, TRUE},
        Graphics::Gdi::{
            BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, DIB_RGB_COLORS,
            DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateIconIndirect, DestroyIcon, GetIconInfo, GetSystemMetrics, HICON, ICONINFO,
            IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTCOLOR, LR_LOADFROMFILE, LoadIconW, LoadImageW,
            SM_CXSMICON, SM_CYSMICON,
        },
    },
    core::PCWSTR,
};

/// The brand icon and its badged copies, one per [`Status`], owned by the tray thread.
pub(super) struct Icons {
    icons: [HICON; 4],
    /// Whether `icons[0]` was loaded by us; the shared system fallback is never destroyed.
    owned: bool,
}

impl Icons {
    /// The executable's own icon (resource 1, see host/build.rs), else the icon file,
    /// else the generic application icon, at the small-icon size Windows uses now.
    pub(super) fn load(file: &std::path::Path) -> windows::core::Result<Self> {
        // SAFETY: plain metric queries.
        let (width, height) =
            unsafe { (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON)) };
        // SAFETY: the module handle of this process stays valid while it runs.
        let module = unsafe { GetModuleHandleW(None)? };
        // SAFETY: resource 1 of this module is read synchronously at the given size.
        let own = unsafe {
            LoadImageW(
                Some(HINSTANCE(module.0)),
                PCWSTR(1 as _),
                IMAGE_ICON,
                width,
                height,
                LR_DEFAULTCOLOR,
            )
        };
        match own.or_else(|_| Self::from_file(file, width, height)) {
            Ok(handle) => Ok(Self::badged_set(HICON(handle.0), true)),
            // SAFETY: a stock system icon, shared and never destroyed.
            Err(_) => Ok(Self::badged_set(
                unsafe { LoadIconW(None, IDI_APPLICATION)? },
                false,
            )),
        }
    }

    /// The icon file at `size` px, for tests and dumps.
    #[cfg(test)]
    pub(super) fn load_file(file: &std::path::Path, size: i32) -> windows::core::Result<Self> {
        let handle = Self::from_file(file, size, size)?;
        Ok(Self::badged_set(HICON(handle.0), true))
    }

    fn from_file(
        file: &std::path::Path,
        width: i32,
        height: i32,
    ) -> windows::core::Result<windows::Win32::Foundation::HANDLE> {
        let path: Vec<u16> = file
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: `path` is NUL-terminated and outlives the synchronous call.
        unsafe {
            LoadImageW(
                None,
                PCWSTR(path.as_ptr()),
                IMAGE_ICON,
                width,
                height,
                LR_LOADFROMFILE,
            )
        }
    }

    fn badged_set(base: HICON, owned: bool) -> Self {
        let mut icons = [base; 4];
        for status in Status::ALL.into_iter().filter(|status| status.badged()) {
            // SAFETY: `base` is a live icon owned by this set or the system.
            icons[status.index()] = unsafe { badged(base, status) }.unwrap_or(base);
        }
        Self { icons, owned }
    }

    /// The icon that shows `status`.
    pub(super) fn get(&self, status: Status) -> HICON {
        self.icons[status.index()]
    }

    /// The BGRA pixels of the icon that shows `status`, as Windows holds them.
    #[cfg(test)]
    pub(super) fn pixels(&self, status: Status) -> Option<(usize, usize, Vec<u8>)> {
        // SAFETY: the icon belongs to this set and lives as long as it does.
        unsafe { pixels(self.get(status)) }
    }
}

impl Drop for Icons {
    fn drop(&mut self) {
        let base = self.icons[0];
        // SAFETY: every badged copy was created by this set and is destroyed once; the base
        // only when this set loaded it. Shell_NotifyIconW copies the icon it shows.
        unsafe {
            for icon in &self.icons[1..] {
                if *icon != base {
                    let _ = DestroyIcon(*icon);
                }
            }
            if self.owned {
                let _ = DestroyIcon(base);
            }
        }
    }
}

/// The colour image of `icon` as top-down BGRA rows with straight alpha.
unsafe fn pixels(icon: HICON) -> Option<(usize, usize, Vec<u8>)> {
    // SAFETY: GetIconInfo returns owned bitmaps that are deleted once below; checked
    // dimensions bound the 32-bit pixel buffer GetDIBits fills synchronously.
    unsafe {
        let mut info = ICONINFO::default();
        GetIconInfo(icon, &mut info).ok()?;
        let read = (|| {
            if info.hbmColor.is_invalid() {
                return None;
            }
            let mut bitmap = BITMAP::default();
            if GetObjectW(
                info.hbmColor.into(),
                size_of::<BITMAP>() as i32,
                Some((&mut bitmap as *mut BITMAP).cast()),
            ) == 0
            {
                return None;
            }
            let (width, height) = (bitmap.bmWidth, bitmap.bmHeight);
            if width <= 0 || height <= 0 || width > 256 || height > 256 {
                return None;
            }
            let mut header = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    // Top-down rows, as CreateBitmap takes them.
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = vec![0u8; width as usize * height as usize * 4];
            let screen = GetDC(None);
            let lines = GetDIBits(
                screen,
                info.hbmColor,
                0,
                height as u32,
                Some(pixels.as_mut_ptr().cast()),
                &mut header,
                DIB_RGB_COLORS,
            );
            ReleaseDC(None, screen);
            (lines == height).then_some((width as usize, height as usize, pixels))
        })();
        let _ = DeleteObject(info.hbmColor.into());
        let _ = DeleteObject(info.hbmMask.into());
        read
    }
}

/// `base` with the status's badge in its corner; None for Idle or an icon
/// without alpha, which then shows as is.
unsafe fn badged(base: HICON, status: Status) -> Option<HICON> {
    // SAFETY: the caller keeps `base` alive; the bitmaps made here are deleted once after
    // CreateIconIndirect has copied them.
    unsafe {
        let (width, height, mut pixels) = pixels(base)?;
        if !composite(&mut pixels, width, height, status) {
            return None;
        }
        let (width, height) = (width as i32, height as i32);
        let color = CreateBitmap(width, height, 1, 32, Some(pixels.as_ptr().cast()));
        // An all-zero mask: the colour image's alpha decides.
        let mask_bytes = vec![0u8; (width as usize).div_ceil(16) * 2 * height as usize];
        let mask = CreateBitmap(width, height, 1, 1, Some(mask_bytes.as_ptr().cast()));
        let icon = (!color.is_invalid() && !mask.is_invalid())
            .then(|| {
                CreateIconIndirect(&ICONINFO {
                    fIcon: TRUE,
                    xHotspot: 0,
                    yHotspot: 0,
                    hbmMask: mask,
                    hbmColor: color,
                })
                .ok()
            })
            .flatten();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}
