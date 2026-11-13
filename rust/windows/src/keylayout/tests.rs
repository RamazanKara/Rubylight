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
        0x15, 0x2c, 0x10, 0x32, 0x1a, 0x1b, 0x27, 0x28, 0x2b, 0x0c, 0x0d, 0x29, 0x56, 0x35, 0x33,
        0x34,
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
        0x10, 0x1e, 0x11, 0x2c, 0x27, 0x0b, 0x32, 0x33, 0x34, 0x35, 0x29, 0x0c, 0x0d, 0x1a, 0x1b,
        0x28, 0x2b, 0x56,
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
