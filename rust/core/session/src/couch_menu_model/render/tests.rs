//! Tests for the 7.0 couch menu panel: golden hashes of settled pages, the
//! colours of each row state and control, opacity and size per layout, and
//! the review dump (`dump`).
mod dump;

use std::time::Instant;

use super::super::model::tests::{snapshot, strings};
use super::super::model::{Snapshot, Style};
use super::super::theme_70::*;
use super::*;
use butterpollo_input_policy::couch_menu::Nav;

fn fnv1a(frame: &Frame) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let header = [frame.origin, frame.size, [frame.scale, 0]];
    for byte in header
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .chain(frame.rgba.iter().copied())
    {
        hash = (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// A menu opened on `snap`, driven by the helpers below.
pub(super) struct Driver {
    pub menu: Menu,
    pub snap: Snapshot,
    pub layout: Layout,
    now: Instant,
}

impl Driver {
    pub fn new(menu: Menu, snap: Snapshot, layout: Layout) -> Self {
        let mut driver = Self {
            menu,
            snap,
            layout,
            now: Instant::now(),
        };
        driver.nav(Nav::Opened);
        driver
    }
    pub fn nav(&mut self, nav: Nav) -> Option<super::super::Action> {
        self.menu.nav(nav, self.now, &self.snap, &self.layout)
    }
    /// Moves the focus down to the row whose text starts with `label`.
    pub fn focus(&mut self, label: &str) {
        for _ in 0..32 {
            let lines = self.menu.lines(&self.snap);
            let focused = lines
                .iter()
                .find(|l| matches!(l.style, Style::Row { focus: true, .. }));
            if focused.is_some_and(|l| l.text.starts_with(label)) {
                return;
            }
            self.nav(Nav::Down);
        }
        panic!("no row {label}");
    }
    /// The line index of the row whose text starts with `label`.
    pub fn line(&self, label: &str) -> u32 {
        let lines = self.menu.lines(&self.snap);
        lines
            .iter()
            .position(|l| l.text.starts_with(label))
            .unwrap() as u32
    }
    pub fn frame(&self) -> Frame {
        render(&self.menu, &self.snap, &self.layout)
    }
}

/// The main page and the Radeon page, as the tests' snapshot shows them.
fn pages(layout: &Layout) -> [Frame; 2] {
    let mut run = Driver::new(Menu::new(strings(false)), snapshot(), *layout);
    let main = run.frame();
    run.nav(Nav::Down);
    run.nav(Nav::Accept);
    run.nav(Nav::Down);
    [main, run.frame()]
}

fn colour(frame: &Frame, x: u32, y: u32) -> Rgb {
    let width = frame.size[0] / frame.scale;
    let at = ((y * width + x) * 4) as usize;
    frame.rgba[at..at + 3].try_into().unwrap()
}

#[test]
fn couch_menu_render_golden_hash_per_page() {
    // Changing a page's look changes these: check the dump by eye, then
    // update them.
    let golden = [
        ([1280, 720], [0xe966_9736_4a17_d115, 0x54b5_790e_09e0_54a6]),
        ([3840, 2160], [0x80bb_68d4_ebbc_1a18, 0xda93_83af_c15a_e507]),
    ];
    let mut got = Vec::new();
    for (target, _) in golden {
        let layout = Layout::for_stream(target[0], target[1]).unwrap();
        got.push((target, pages(&layout).map(|page| fnv1a(&page))));
    }
    let want: Vec<_> = golden.to_vec();
    assert_eq!(got, want, "golden hashes");
}

#[test]
fn couch_menu_render_is_opaque_hairlined_and_sized_for_the_layout() {
    for [w, h] in [
        [640, 360],
        [1280, 720],
        [1920, 1080],
        [3840, 2160],
        [1080, 2400],
    ] {
        let layout = Layout::for_stream(w, h).unwrap();
        for frame in pages(&layout) {
            let [fw, fh] = frame.size.map(|v| v / frame.scale);
            assert_eq!(frame.size, [fw, fh].map(|v| v * layout.scale));
            assert_eq!(frame.rgba.len(), (fw * fh * 4) as usize);
            assert!(
                frame.rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 0xff),
                "alpha 255"
            );
            assert!(frame.origin.iter().chain(&frame.size).all(|v| v % 16 == 0));
            for (x, y) in [
                (0, 0),
                (fw - 1, fh - 1),
                (fw - 1, 7),
                (5, fh - 1),
                (0, fh / 2),
            ] {
                assert_eq!(colour(&frame, x, y), HAIRLINE, "border at {x},{y}");
            }
            for (x, y) in [(1, 1), (fw - 2, fh - 2), (2, fh / 2)] {
                assert_eq!(colour(&frame, x, y), STAGE, "stage at {x},{y}");
            }
        }
    }
}

/// The row states and controls at 1920x1080 (16x32 cells at 1x, so the
/// widgets' unit is 1): sampled where each colour must be exact.
#[test]
fn couch_menu_render_draws_cards_switches_sliders_and_markers() {
    let layout = Layout::for_stream(1920, 1080).unwrap();
    let mut run = Driver::new(Menu::new(strings(false)), snapshot(), layout);
    let [pad_x, pad_y] = layout.padding();
    let right = pad_x + COLUMNS * 16;
    let top = |line: u32| pad_y + line * 32;
    let mid = |line: u32| top(line) + 16;
    let frame = run.frame();
    let width = frame.size[0];
    // The focused first row is a ruby card, the others are cards, the
    // gaps between them stage.
    let back = run.line("Back to game");
    assert_eq!(colour(&frame, width / 2, top(back) + 2), FOCUS);
    assert_eq!(colour(&frame, width / 2, mid(back)), FOCUS);
    let end = run.line("End session");
    assert_eq!(colour(&frame, width / 2, mid(end)), CARD);
    assert_eq!(colour(&frame, width / 2, top(end)), STAGE);
    assert_eq!(colour(&frame, width / 2, top(end) + 31), STAGE);
    // The card's rounded corner blends from stage to card.
    let corner = colour(&frame, pad_x / 2 + 1, top(end) + 3);
    assert!(corner != STAGE && corner != CARD, "{corner:?}");
    // The header badge: ruby with a white mark, under it the rule.
    assert_eq!(colour(&frame, pad_x + 12, pad_y + 4 + 12), FOCUS_INK);
    assert_eq!(colour(&frame, pad_x + 12, pad_y + 4 + 1), BRAND);
    assert_eq!(colour(&frame, width / 2, pad_y + 31), HAIRLINE);
    // Night mode, off: a track pill with a muted knob on the left.
    let night = run.line("Night mode");
    assert_eq!(colour(&frame, right - 28, mid(night)), MUTED);
    assert_eq!(colour(&frame, right - 6, mid(night)), TRACK);
    // Focused, off: a strong ruby pill with a white knob.
    run.focus("Night mode");
    let frame = run.frame();
    assert_eq!(colour(&frame, right - 28, mid(night)), FOCUS_INK);
    assert_eq!(colour(&frame, right - 6, mid(night)), FOCUS_STRONG);

    // The Radeon page: Anti-Lag on, an OK pill with a stage knob on the right.
    run.focus("Radeon settings");
    run.nav(Nav::Accept);
    let frame = run.frame();
    let anti_lag = run.line("Anti-Lag");
    assert_eq!(colour(&frame, right - 12, mid(anti_lag)), STAGE);
    assert_eq!(colour(&frame, right - 34, mid(anti_lag)), OK);
    // Image sharpening at 80 of 0..100: rail of 5 columns before "80 %".
    let rail = pad_x + (COLUMNS - 4 - 1 - 5) * 16;
    let sharpening = run.line("Image sharpening");
    assert_eq!(colour(&frame, rail + 4, mid(sharpening)), INK_2, "fill");
    assert_eq!(colour(&frame, rail + 76, mid(sharpening)), TRACK, "rail");
    assert_eq!(colour(&frame, rail + 8 + 51, mid(sharpening)), INK, "knob");
    run.focus("Image sharpening");
    let frame = run.frame();
    assert_eq!(colour(&frame, rail + 4, mid(sharpening)), FOCUS_INK);
    assert_eq!(colour(&frame, rail + 76, mid(sharpening)), FOCUS_STRONG);
    assert_eq!(colour(&frame, rail + 8 + 51, mid(sharpening)), FOCUS_INK);
    // The locked Radeon Chill keeps a card and takes a ruby marker.
    run.focus("Radeon Chill");
    let frame = run.frame();
    let chill = run.line("Radeon Chill");
    assert_eq!(colour(&frame, width / 2, mid(chill)), CARD);
    assert_eq!(colour(&frame, pad_x / 2 + 4, mid(chill)), FOCUS, "marker");
    assert_eq!(colour(&frame, width / 2, top(chill) + 2), FOCUS, "outline");
    assert_eq!(colour(&frame, width / 2, top(chill) + 4), CARD);
    assert_eq!(
        colour(&frame, right - 28, mid(chill)),
        MUTED,
        "locked switch"
    );
}

#[test]
fn couch_menu_render_text_uses_the_theme_inks() {
    let layout = Layout::for_stream(1920, 1080).unwrap();
    let run = Driver::new(Menu::new(strings(false)), snapshot(), layout);
    let frame = run.frame();
    let used: std::collections::HashSet<Rgb> = frame
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| [p[0], p[1], p[2]])
        .collect();
    for ink in [
        INK, INK_2, MUTED, FOCUS_INK, FOCUS, CARD, STAGE, HAIRLINE, BRAND,
    ] {
        assert!(used.contains(&ink), "{ink:?}");
    }
    // No colour of the 6.x panel is left.
    for old in [[0x17, 0x17, 0x1c], [0xe0, 0x11, 0x5f]] {
        assert!(!used.contains(&old), "{old:?}");
    }
}
