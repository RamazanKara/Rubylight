//! Tests for the tray's status colours, badge compositing, contrast and menu,
//! and the badge dump (`RUBYLIGHT_TRAY_DUMP_DIR`).
use super::*;

const SIZES: [usize; 4] = [16, 24, 32, 48];
const TASKBAR_LIGHT: [u8; 3] = [0xf3, 0xf3, 0xf3];
const TASKBAR_DARK: [u8; 3] = [0x1c, 0x1c, 0x1c];

/// The first value of `--name` in the 7.0 tokens.
fn token(css: &str, name: &str) -> [u8; 3] {
    let start = css.find(&format!("{name}:")).expect(name) + name.len() + 1;
    let value = css[start..].split(';').next().unwrap().trim();
    let hex = value.strip_prefix('#').expect(value);
    assert_eq!(hex.len(), 6, "{name}: {value}");
    [0, 2, 4].map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
}

#[test]
fn the_status_colours_are_the_70_tokens() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/src/styles/tokens.css");
    let css = std::fs::read_to_string(&path).expect("rust/web/src/styles/tokens.css");
    assert_eq!(Status::Idle.color(), token(&css, "--brand"));
    assert_eq!(Status::Streaming.color(), token(&css, "--media-ok"));
    assert_eq!(Status::Attention.color(), token(&css, "--media-warn"));
    assert_eq!(Status::Error.color(), token(&css, "--media-danger"));
    assert_eq!(BRAND, [0xc4, 0x12, 0x42]);
}

fn luminance(rgb: [u8; 3]) -> f64 {
    let channel = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(rgb[0]) + 0.7152 * channel(rgb[1]) + 0.0722 * channel(rgb[2])
}

fn contrast(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[test]
fn the_two_tone_ring_reads_on_light_and_dark_taskbars() {
    // One of the two ring tones stands out against each taskbar (WCAG non-text 3:1,
    // held to 4.5:1 here), and every fill stands out against the dark ring beside it.
    for taskbar in [TASKBAR_LIGHT, TASKBAR_DARK] {
        let best = contrast(RING_DARK, taskbar).max(contrast(RING_LIGHT, taskbar));
        assert!(best >= 4.5, "ring on {taskbar:02x?}: {best:.2}");
    }
    assert!(contrast(RING_DARK, TASKBAR_LIGHT) >= 4.5);
    assert!(contrast(RING_LIGHT, TASKBAR_DARK) >= 4.5);
    for status in [Status::Streaming, Status::Attention, Status::Error] {
        let fill = contrast(status.color(), RING_DARK);
        assert!(fill >= 3.0, "{status:?} against the ring: {fill:.2}");
    }
}

/// A square brand-like icon: opaque ruby with a transparent margin.
fn icon(size: usize) -> Vec<u8> {
    let mut pixels = vec![0u8; size * size * 4];
    for y in 1..size - 1 {
        for x in 1..size - 1 {
            pixels[(y * size + x) * 4..][..4].copy_from_slice(&[0x42, 0x12, 0xc4, 0xff]);
        }
    }
    pixels
}

fn rgb_at(pixels: &[u8], size: usize, x: usize, y: usize) -> [u8; 3] {
    let p = &pixels[(y * size + x) * 4..][..4];
    [p[2], p[1], p[0]]
}

#[test]
fn every_status_badges_the_bottom_right_corner_at_every_size() {
    for size in SIZES {
        let (diameter, _, _) = badge_geometry(size);
        let ratio = diameter / size as f32;
        assert!((0.4..=0.5).contains(&ratio), "{size}px badge is {ratio:.2}");
        for status in [Status::Streaming, Status::Attention, Status::Error] {
            let mut pixels = icon(size);
            assert!(composite(&mut pixels, size, size, status));
            assert_eq!(pixels.len(), size * size * 4);
            let centre = (size as f32 - diameter / 2.0) as usize;
            assert_eq!(
                rgb_at(&pixels, size, centre, centre),
                status.color(),
                "{status:?} at {size}px"
            );
            // The corner pixel of the badge's bounding box stays outside the circle.
            assert_eq!(rgb_at(&pixels, size, 1, 1), BRAND);
            // The ring: a dark pixel and a near-white pixel inside the badge.
            let badge: Vec<[u8; 3]> = (size - diameter as usize..size)
                .flat_map(|y| (size - diameter as usize..size).map(move |x| (x, y)))
                .map(|(x, y)| rgb_at(&pixels, size, x, y))
                .collect();
            let near = |a: [u8; 3], b: [u8; 3]| (0..3).all(|c| a[c].abs_diff(b[c]) <= 24);
            assert!(
                badge.iter().any(|p| near(*p, RING_DARK)),
                "{size}px dark ring"
            );
            assert!(
                badge.iter().any(|p| p.iter().all(|c| *c >= 0xd0)),
                "{size}px light hairline"
            );
            // The badge is (nearly) opaque where it covers the transparent margin.
            assert!(pixels[(centre * size + size - 1) * 4 + 3] >= 0xf0);
        }
    }
}

#[test]
fn idle_keeps_the_plain_brand_icon_and_flat_images_stay_as_they_are() {
    let mut pixels = icon(16);
    let before = pixels.clone();
    assert!(!composite(&mut pixels, 16, 16, Status::Idle));
    assert_eq!(pixels, before);
    let mut flat = vec![0u8; 16 * 16 * 4];
    assert!(!composite(&mut flat, 16, 16, Status::Streaming));
    assert!(flat.iter().all(|b| *b == 0));
    assert!(!composite(&mut [0u8; 8], 16, 16, Status::Streaming));
}

#[test]
fn the_status_follows_the_running_app() {
    assert_eq!(Status::from_icon(Icon::Idle, false), Status::Idle);
    assert_eq!(Status::from_icon(Icon::Idle, true), Status::Idle);
    assert_eq!(Status::from_icon(Icon::Streaming, false), Status::Streaming);
    assert_eq!(Status::from_icon(Icon::Paused, false), Status::Attention);
    assert_eq!(Status::from_icon(Icon::Streaming, true), Status::Error);
    for (at, status) in Status::ALL.iter().enumerate() {
        assert_eq!(status.index(), at);
    }
}

#[test]
fn the_header_says_what_is_happening() {
    assert_eq!(header(Status::Idle, None), "Ready");
    assert_eq!(
        header(Status::Streaming, Some("Portal 2")),
        "Streaming Portal 2"
    );
    assert_eq!(
        header(Status::Attention, Some("Portal 2")),
        "Needs attention: Portal 2 is paused"
    );
    assert_eq!(
        header(Status::Error, Some("Portal 2")),
        "Needs attention: Portal 2 closed right after it started"
    );
    assert_eq!(menu_label("Tom & Jerry"), "Tom && Jerry");
}

fn actions(items: &[Item]) -> Vec<(Action, bool)> {
    items
        .iter()
        .filter_map(|item| match item {
            Item::Entry {
                action, enabled, ..
            } => Some((*action, *enabled)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_menu_leads_with_the_status_and_keeps_every_entry() {
    let items = menu(Status::Streaming, Some("Portal 2"), false);
    assert_eq!(items[0], Item::Header("Streaming Portal 2".into()));
    assert_eq!(items[1], Item::Separator);
    assert_eq!(
        actions(&items),
        [
            (Action::Open, true),
            (Action::PairDevice, true),
            (Action::EnterPin, true),
            (Action::StopSessions, true),
            (Action::QuitApp, true),
            (Action::CheckUpdates, true),
            (Action::BackupSettings, true),
            (Action::RestoreSettings, true),
            (Action::Restart, true),
            (Action::Quit, true),
        ]
    );
    assert!(items.iter().any(|item| matches!(
        item,
        Item::Entry { action: Action::QuitApp, label, .. } if label == "Quit Portal 2"
    )));
    // Never two separators in a row, nor one at the end.
    assert!(
        items
            .windows(2)
            .all(|pair| pair != [Item::Separator, Item::Separator])
    );
    assert_ne!(items.last(), Some(&Item::Separator));

    let idle = menu(Status::Idle, None, false);
    assert!(actions(&idle).contains(&(Action::QuitApp, false)));
    let limited = menu(Status::Idle, None, true);
    assert_eq!(
        actions(&limited),
        [
            (Action::Open, true),
            (Action::PairDevice, true),
            (Action::EnterPin, true),
            (Action::CheckUpdates, true),
        ]
    );
}

/// Each status at 16/24/32/48 px through the real GDI path, on a light and a dark
/// taskbar strip, magnified 8x: `RUBYLIGHT_TRAY_DUMP_DIR=<folder>` turns it on.
#[test]
fn tray_badge_dump() {
    let Some(folder) = std::env::var_os("RUBYLIGHT_TRAY_DUMP_DIR") else {
        return;
    };
    let folder = std::path::PathBuf::from(folder);
    std::fs::create_dir_all(&folder).unwrap();
    let ico = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/butterpollo.ico");
    const SCALE: usize = 8;
    for (theme, background) in [("light", TASKBAR_LIGHT), ("dark", TASKBAR_DARK)] {
        for size in SIZES {
            let gap = size / 2;
            let (width, height) = (gap + Status::ALL.len() * (size + gap), size + 2 * gap);
            let mut strip: Vec<u8> = background
                .iter()
                .copied()
                .chain([0xff])
                .cycle()
                .take(width * height * 4)
                .collect();
            let icons =
                super::super::icon::Icons::load_file(&ico, size as i32).expect("the brand icon");
            for (n, status) in Status::ALL.into_iter().enumerate() {
                let (w, h, bgra) = icons.pixels(status).expect("icon pixels");
                assert_eq!((w, h), (size, size), "{status:?} at {size}px");
                let left = gap + n * (size + gap);
                for y in 0..size {
                    for x in 0..size {
                        let p = &bgra[(y * size + x) * 4..][..4];
                        let a = u32::from(p[3]);
                        let at = ((gap + y) * width + left + x) * 4;
                        for (c, bgr) in [2usize, 1, 0].into_iter().enumerate() {
                            let under = u32::from(strip[at + c]);
                            strip[at + c] =
                                ((u32::from(p[bgr]) * a + under * (255 - a) + 127) / 255) as u8;
                        }
                    }
                }
            }
            let (big_w, big_h) = (width * SCALE, height * SCALE);
            let mut big = vec![0u8; big_w * big_h * 4];
            for y in 0..big_h {
                for x in 0..big_w {
                    let from = ((y / SCALE) * width + x / SCALE) * 4;
                    big[(y * big_w + x) * 4..][..4].copy_from_slice(&strip[from..from + 4]);
                }
            }
            let file =
                std::fs::File::create(folder.join(format!("tray-{theme}-{size}px.png"))).unwrap();
            let mut encoder =
                png::Encoder::new(std::io::BufWriter::new(file), big_w as u32, big_h as u32);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&big)
                .unwrap();
        }
    }
}
