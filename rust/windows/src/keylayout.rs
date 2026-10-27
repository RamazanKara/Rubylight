// Fixed US scan codes from the retained Moonlight Windows input backend.
pub const SCANCODES: [u8; 256] = [
    0, 0, 0, 70, 0, 0, 0, 0, 14, 15, 0, 0, 76, 28, 0, 0, 42, 29, 56, 0, 58, 0, 0, 0, 0, 0, 0, 1, 0,
    0, 0, 0, 57, 73, 81, 79, 71, 75, 72, 77, 80, 0, 0, 0, 84, 82, 83, 99, 11, 2, 3, 4, 5, 6, 7, 8,
    9, 10, 0, 0, 0, 0, 0, 0, 0, 30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16,
    19, 31, 20, 22, 47, 17, 45, 21, 44, 91, 92, 93, 0, 95, 82, 79, 80, 81, 75, 76, 77, 71, 72, 73,
    55, 78, 0, 74, 83, 53, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 87, 88, 100, 101, 102, 103, 104,
    105, 106, 107, 108, 109, 110, 118, 0, 0, 0, 0, 0, 0, 0, 0, 69, 70, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 42, 54, 29, 29, 56, 56, 106, 105, 103, 104, 101, 102, 50, 32, 46, 48, 25, 16,
    36, 34, 108, 109, 107, 33, 0, 0, 39, 13, 51, 12, 52, 53, 41, 115, 126, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 26, 43, 27, 40, 0, 0, 0, 86, 0, 0, 0, 0, 0, 0,
    113, 92, 123, 0, 111, 90, 0, 0, 91, 0, 95, 0, 94, 0, 0, 0, 93, 0, 98, 0, 0, 0, 0,
];

/// The scan code of each virtual key on one Windows keyboard layout, read from the
/// layout's DLL. Reading the table loads no layout into Windows: the user's input
/// switcher shows nothing new during a stream, and nothing is left to unload after it.
#[derive(Clone, PartialEq, Eq)]
pub struct Layout {
    klid: u32,
    scancodes: [u8; 256],
}

impl std::fmt::Debug for Layout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Layout({:08X})", self.klid)
    }
}

/// The start of `KBDTABLES` in kbd.h, up to the scan code table.
#[repr(C)]
struct KbdTables {
    char_modifiers: *const std::ffi::c_void,
    vk_to_wchar: *const std::ffi::c_void,
    dead_keys: *const std::ffi::c_void,
    key_names: *const std::ffi::c_void,
    key_names_ext: *const std::ffi::c_void,
    key_names_dead: *const std::ffi::c_void,
    vsc_to_vk: *const u16,
    max_vsc_to_vk: u8,
}

impl Layout {
    /// The layout with this id (KLID), from the layout file Windows lists for it.
    pub fn load(klid: u32) -> anyhow::Result<Self> {
        use anyhow::{Context, bail};
        use windows::Win32::{
            Foundation::FreeLibrary,
            System::{
                LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW},
                ProcessStatus::{GetModuleInformation, MODULEINFO},
                Threading::GetCurrentProcess,
            },
        };
        let file = layout_file(klid)?;
        if !file.to_ascii_lowercase().ends_with(".dll")
            || !file
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        {
            bail!("keyboard layout {klid:08X} names the layout file {file:?}");
        }
        let mut folder = [0u16; 260];
        // SAFETY: The buffer is live and writable for its whole length.
        let length = unsafe {
            windows::Win32::System::SystemInformation::GetSystemDirectoryW(Some(&mut folder))
        } as usize;
        if length == 0 || length >= folder.len() {
            bail!(
                "the system folder is unknown: {}",
                std::io::Error::last_os_error()
            );
        }
        let path: Vec<u16> = folder[..length]
            .iter()
            .copied()
            .chain(format!("\\{file}").encode_utf16())
            .chain([0])
            .collect();
        // SAFETY: The path is terminated. A layout file is a system DLL that Windows
        // itself loads into every desktop session; it and what it imports come from the
        // system folder only. Loading it as an image still runs its DllMain, as Windows'
        // own loading does; mapping it as a data file would mean relocating the tables'
        // pointers by hand.
        let module = unsafe {
            LoadLibraryExW(
                windows::core::PCWSTR(path.as_ptr()),
                None,
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        }
        .with_context(|| format!("loading {file} for keyboard layout {klid:08X}"))?;
        // SAFETY: The module stays loaded until FreeLibrary below and the name is
        // terminated. KbdLayerDescriptor takes nothing and returns the layout's static
        // KBDTABLES, which KbdTables is the start of; its scan code table has
        // max_vsc_to_vk entries. Both are read only once they lie inside the loaded
        // image, aligned, and the table is copied before the module is freed.
        let table = unsafe {
            let mut info = MODULEINFO::default();
            let image = GetModuleInformation(
                GetCurrentProcess(),
                module,
                &mut info,
                std::mem::size_of::<MODULEINFO>() as u32,
            )
            .map(|()| (info.lpBaseOfDll as usize, info.SizeOfImage as usize));
            let table = match image {
                Err(error) => Err(format!("has no image information ({error})")),
                Ok((base, size)) => {
                    match GetProcAddress(module, windows::core::s!("KbdLayerDescriptor")) {
                        None => Err("has no keyboard layout tables".to_owned()),
                        Some(descriptor) => {
                            let descriptor: unsafe extern "system" fn() -> *const KbdTables =
                                std::mem::transmute(descriptor);
                            let tables = descriptor();
                            if tables.is_null() {
                                Err("has no keyboard layout tables".to_owned())
                            } else if !in_image(base, size, tables as usize, size_of::<KbdTables>())
                                || !tables.is_aligned()
                            {
                                Err(OUTSIDE.to_owned())
                            } else {
                                let tables = &*tables;
                                let length = usize::from(tables.max_vsc_to_vk);
                                if tables.vsc_to_vk.is_null()
                                    || !tables.vsc_to_vk.is_aligned()
                                    || !in_image(
                                        base,
                                        size,
                                        tables.vsc_to_vk as usize,
                                        length * size_of::<u16>(),
                                    )
                                {
                                    Err(OUTSIDE.to_owned())
                                } else {
                                    Ok(std::slice::from_raw_parts(tables.vsc_to_vk, length)
                                        .to_vec())
                                }
                            }
                        }
                    }
                }
            };
            let _ = FreeLibrary(module);
            table
        };
        let table = table.map_err(|reason| anyhow::anyhow!("{file} {reason}"))?;
        let layout = Self::from_table(klid, &table);
        if layout.scancodes.iter().all(|&scan| scan == 0) {
            bail!("{file} maps no keys");
        }
        Ok(layout)
    }

    /// The layout of a scan code table: each entry's low byte is the virtual key of the
    /// scan code it is indexed by, the high byte flags. A key found twice keeps its lowest
    /// scan code, as MapVirtualKeyEx does.
    fn from_table(klid: u32, table: &[u16]) -> Self {
        let mut scancodes = [0; 256];
        for (scan, entry) in table.iter().enumerate().take(256).skip(1) {
            let vk = usize::from(entry & 0xff);
            if vk != 0 && vk != 0xff && scancodes[vk] == 0 {
                scancodes[vk] = scan as u8;
            }
        }
        Self { klid, scancodes }
    }

    pub fn klid(&self) -> u32 {
        self.klid
    }

    /// The scan code of a virtual key, or None if the layout has no such key outside the
    /// E0 and E1 ones (AltGr, the right Ctrl, the keys between the main block and the
    /// numpad), which every layout puts in the same place.
    pub fn scancode(&self, vk: u16) -> Option<u8> {
        self.scancodes
            .get(usize::from(vk))
            .copied()
            .filter(|&scan| scan != 0)
    }
}

/// The most layout ids one [`Layouts`] reads.
pub const MAX_LAYOUTS: usize = 16;

/// The layouts read for one peer by id, read or failed: each id's file is loaded at most
/// once however often a device reports it, and at most [`MAX_LAYOUTS`] ids are.
pub struct Layouts {
    load: fn(u32) -> anyhow::Result<Layout>,
    read: std::collections::HashMap<u32, Result<Layout, String>>,
    refused: bool,
}

impl Default for Layouts {
    fn default() -> Self {
        Self::with_loader(Layout::load)
    }
}

impl Layouts {
    /// Reads layouts with `load` instead of [`Layout::load`], for tests.
    pub fn with_loader(load: fn(u32) -> anyhow::Result<Layout>) -> Self {
        Self {
            load,
            read: Default::default(),
            refused: false,
        }
    }

    /// The layout with this id, or why it cannot be read, and whether that is new: the
    /// first time this id was asked for, or the first refusal past [`MAX_LAYOUTS`].
    pub fn get(&mut self, klid: u32) -> (Result<Layout, String>, bool) {
        if let Some(read) = self.read.get(&klid) {
            return (read.clone(), false);
        }
        if self.read.len() >= MAX_LAYOUTS {
            let first = !std::mem::replace(&mut self.refused, true);
            return (
                Err(format!(
                    "keyboard layout {klid:08X} is one more than the {MAX_LAYOUTS} this stream reads"
                )),
                first,
            );
        }
        let read = (self.load)(klid).map_err(|error| format!("{error:#}"));
        self.read.insert(klid, read.clone());
        (read, true)
    }
}

/// Why tables are refused that point out of the layout file's image.
const OUTSIDE: &str = "has keyboard layout tables outside its image";

/// Whether `len` bytes from `start` lie inside the image of `size` bytes at `base`.
fn in_image(base: usize, size: usize, start: usize, len: usize) -> bool {
    start >= base && len <= size && start - base <= size - len
}

/// The layout DLL Windows lists for a layout id.
fn layout_file(klid: u32) -> anyhow::Result<String> {
    use windows::Win32::System::Registry::*;
    let key: Vec<u16> = format!("SYSTEM\\CurrentControlSet\\Control\\Keyboard Layouts\\{klid:08X}")
        .encode_utf16()
        .chain([0])
        .collect();
    let mut text = [0u16; 64];
    let mut size = (text.len() * 2) as u32;
    // SAFETY: The names are terminated and the output size matches the live UTF-16 buffer.
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            windows::core::PCWSTR(key.as_ptr()),
            windows::core::w!("Layout File"),
            RRF_RT_REG_SZ,
            None,
            Some(text.as_mut_ptr().cast()),
            Some(&mut size),
        )
    }
    .ok()
    .map_err(|error| {
        anyhow::anyhow!("keyboard layout {klid:08X} is unknown to this PC ({error})")
    })?;
    let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
    Ok(String::from_utf16_lossy(&text[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const VK_OEM_1: u16 = 0xba;
    const VK_OEM_PLUS: u16 = 0xbb;
    const VK_OEM_COMMA: u16 = 0xbc;
    const VK_OEM_MINUS: u16 = 0xbd;
    const VK_OEM_PERIOD: u16 = 0xbe;
    const VK_OEM_2: u16 = 0xbf;
    const VK_OEM_3: u16 = 0xc0;
    const VK_OEM_4: u16 = 0xdb;
    const VK_OEM_5: u16 = 0xdc;
    const VK_OEM_6: u16 = 0xdd;
    const VK_OEM_7: u16 = 0xde;
    const VK_OEM_8: u16 = 0xdf;
    const VK_OEM_102: u16 = 0xe2;

    fn scancodes(klid: u32, keys: &[u16]) -> Vec<Option<u8>> {
        let layout = Layout::load(klid).unwrap();
        assert_eq!(layout.klid(), klid);
        keys.iter().map(|&vk| layout.scancode(vk)).collect()
    }

    /// German swaps Z and Y and puts the umlauts on the US punctuation keys.
    #[test]
    fn german_keys_have_their_physical_scan_codes() {
        let keys = [
            u16::from(b'Z'),
            u16::from(b'Y'),
            u16::from(b'Q'),
            u16::from(b'M'),
            VK_OEM_1,
            VK_OEM_PLUS,
            VK_OEM_3,
            VK_OEM_7,
            VK_OEM_2,
            VK_OEM_4,
            VK_OEM_6,
            VK_OEM_5,
            VK_OEM_102,
            VK_OEM_MINUS,
            VK_OEM_COMMA,
            VK_OEM_PERIOD,
        ];
        let expected = [
            0x15, 0x2c, 0x10, 0x32, 0x1a, 0x1b, 0x27, 0x28, 0x2b, 0x0c, 0x0d, 0x29, 0x56, 0x35,
            0x33, 0x34,
        ];
        assert_eq!(scancodes(0x0407, &keys), expected.map(Some));
    }

    /// French swaps A and Q, Z and W, and moves M beside L.
    #[test]
    fn french_keys_have_their_physical_scan_codes() {
        let keys = [
            u16::from(b'A'),
            u16::from(b'Q'),
            u16::from(b'Z'),
            u16::from(b'W'),
            u16::from(b'M'),
            u16::from(b'0'),
            VK_OEM_COMMA,
            VK_OEM_PERIOD,
            VK_OEM_2,
            VK_OEM_8,
            VK_OEM_7,
            VK_OEM_4,
            VK_OEM_PLUS,
            VK_OEM_6,
            VK_OEM_1,
            VK_OEM_3,
            VK_OEM_5,
            VK_OEM_102,
        ];
        let expected = [
            0x10, 0x1e, 0x11, 0x2c, 0x27, 0x0b, 0x32, 0x33, 0x34, 0x35, 0x29, 0x0c, 0x0d, 0x1a,
            0x1b, 0x28, 0x2b, 0x56,
        ];
        assert_eq!(scancodes(0x040c, &keys), expected.map(Some));
    }

    /// UK English moves only the keys around Enter and the extra key beside left Shift.
    #[test]
    fn uk_keys_have_their_physical_scan_codes() {
        let keys = [
            u16::from(b'Z'),
            u16::from(b'A'),
            u16::from(b'M'),
            VK_OEM_3,
            VK_OEM_8,
            VK_OEM_7,
            VK_OEM_5,
            VK_OEM_1,
            VK_OEM_2,
        ];
        let expected = [0x2c, 0x1e, 0x32, 0x28, 0x29, 0x2b, 0x56, 0x27, 0x35];
        assert_eq!(scancodes(0x0809, &keys), expected.map(Some));
    }

    /// The US layout file agrees with the fixed table normalized keys use, for the
    /// letters, digits and punctuation that layouts move.
    #[test]
    fn us_layout_matches_the_fixed_table() {
        let layout = Layout::load(0x0409).unwrap();
        for vk in (u16::from(b'0')..=u16::from(b'9'))
            .chain(u16::from(b'A')..=u16::from(b'Z'))
            .chain(VK_OEM_1..=VK_OEM_3)
            .chain(VK_OEM_4..=VK_OEM_7)
        {
            assert_eq!(
                layout.scancode(vk),
                Some(SCANCODES[usize::from(vk)]),
                "{vk:#x}"
            );
        }
    }

    /// AltGr, the right Ctrl and the numpad's digits are E0 or numpad keys, the same on
    /// every layout: the layout does not name them, and the caller keeps the host's
    /// mapping.
    #[test]
    fn extended_keys_are_left_to_the_host() {
        let layout = Layout::load(0x0407).unwrap();
        assert_eq!(layout.scancode(0xa5), None);
        assert_eq!(layout.scancode(0xa3), None);
        assert_eq!(layout.scancode(0x67), None);
        assert_eq!(layout.scancode(0x1ff), None);
    }

    #[test]
    fn unknown_layouts_fail_naming_the_id() {
        let error = format!("{:#}", Layout::load(0x0bad_0407).unwrap_err());
        assert!(error.contains("0BAD0407"), "{error}");
    }

    /// Layout tables are read only inside the image they came from, the end included.
    #[test]
    fn tables_outside_the_image_are_refused() {
        let (base, size) = (0x1000_0000, 0x4000);
        assert!(in_image(base, size, base, size));
        assert!(in_image(base, size, base + 0x100, 0x200));
        assert!(in_image(base, size, base + size - 2, 2));
        assert!(!in_image(base, size, base + size - 1, 2));
        assert!(!in_image(base, size, base - 2, 2));
        assert!(!in_image(base, size, base + size, 0x10));
        assert!(!in_image(base, size, base, size + 1));
        assert!(!in_image(base, size, 0, 0));
        assert!(!in_image(base, size, usize::MAX - 1, 4));
    }

    /// Every layout this PC lists reads from inside its own image.
    #[test]
    fn every_installed_layout_loads() {
        use windows::Win32::System::Registry::*;
        let mut key = HKEY::default();
        // SAFETY: The name is terminated and `key` is closed below.
        unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                windows::core::w!("SYSTEM\\CurrentControlSet\\Control\\Keyboard Layouts"),
                None,
                KEY_READ,
                &mut key,
            )
        }
        .ok()
        .unwrap();
        let mut loaded = 0;
        for index in 0.. {
            let mut name = [0u16; 16];
            let mut length = name.len() as u32;
            // SAFETY: `name` is live and writable for `length` characters.
            let result = unsafe {
                RegEnumKeyExW(
                    key,
                    index,
                    Some(windows::core::PWSTR(name.as_mut_ptr())),
                    &mut length,
                    None,
                    None,
                    None,
                    None,
                )
            };
            if result.is_err() {
                break;
            }
            let name = String::from_utf16_lossy(&name[..length as usize]);
            let Ok(klid) = u32::from_str_radix(&name, 16) else {
                continue;
            };
            // A layout whose file Windows no longer ships fails on its own, naming it.
            match Layout::load(klid) {
                Ok(layout) => {
                    assert_eq!(layout.klid(), klid);
                    loaded += 1;
                }
                Err(error) => {
                    let error = format!("{error:#}");
                    assert!(!error.contains(OUTSIDE), "{name}: {error}");
                }
            }
        }
        // SAFETY: Opened above.
        let _ = unsafe { RegCloseKey(key) };
        assert!(loaded > 50, "{loaded} layouts loaded");
    }

    /// Each id is loaded once, a failed one too, and past the cap nothing more is.
    #[test]
    fn layouts_load_each_id_once_up_to_the_cap() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static LOADS: AtomicUsize = AtomicUsize::new(0);
        fn load(klid: u32) -> anyhow::Result<Layout> {
            LOADS.fetch_add(1, Ordering::SeqCst);
            if klid == 0x0bad {
                anyhow::bail!("no {klid:08X}");
            }
            Ok(Layout::from_table(klid, &[0, 0, u16::from(b'1')]))
        }
        let mut layouts = Layouts::with_loader(load);
        let (first, new) = layouts.get(0x0407);
        assert_eq!(first.unwrap().klid(), 0x0407);
        assert!(new);
        let (again, new) = layouts.get(0x0407);
        assert_eq!(again.unwrap().klid(), 0x0407);
        assert!(!new);
        assert!(layouts.get(0x0bad).1);
        let (failed, new) = layouts.get(0x0bad);
        assert!(failed.unwrap_err().contains("00000BAD"));
        assert!(!new);
        assert_eq!(LOADS.load(Ordering::SeqCst), 2);
        for klid in 0..MAX_LAYOUTS as u32 - 2 {
            layouts.get(0x1_0000 + klid).0.unwrap();
        }
        let (refused, new) = layouts.get(0x0409);
        assert!(refused.unwrap_err().contains("00000409"));
        assert!(new);
        assert!(!layouts.get(0x0809).1);
        assert_eq!(LOADS.load(Ordering::SeqCst), MAX_LAYOUTS);
        // Ids read before still come from the cache.
        assert!(layouts.get(0x0407).0.is_ok());
    }

    /// The first of two scan codes for one key wins; flags in the high byte are ignored.
    #[test]
    fn a_table_keeps_the_lowest_scan_code_of_a_key() {
        let mut table = [0u16; 0x60];
        table[0x10] = u16::from(b'Q');
        table[0x47] = 0x0824; // Home, flagged as a numpad key.
        table[0x50] = u16::from(b'Q');
        table[0x51] = 0xff;
        let layout = Layout::from_table(0x0407, &table);
        assert_eq!(layout.scancode(u16::from(b'Q')), Some(0x10));
        assert_eq!(layout.scancode(0x24), Some(0x47));
        assert_eq!(layout.scancode(0xff), None);
    }
}
