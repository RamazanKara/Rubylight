use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use super::*;
use crate::couch_menu::chord::{
    KeyChord, MODIFIER_ALT, MODIFIER_CTRL, MODIFIER_SHIFT, PAD_CHORD, PAD_HOLD_MS, PadButtons,
    TOUCH_MOVE, parse_hold,
};

const MS: Duration = Duration::from_millis(1);

/// Counts this thread's allocations, so the closed path can be shown to
/// allocate nothing while other tests run in parallel.
struct Counting;
thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}
fn count() {
    let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
}
// SAFETY: Every call forwards to the system allocator with the caller's
// arguments; counting touches only a const thread-local.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        // SAFETY: Forwarded unchanged; the caller upholds `alloc`'s contract.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        // SAFETY: As for `alloc`.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
        // SAFETY: `ptr` came from this allocator, which is the system's.
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from this allocator, which is the system's.
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static COUNTING: Counting = Counting;
fn allocations() -> usize {
    ALLOCATIONS.with(Cell::get)
}

fn pad(id: u16, buttons: u32) -> Input {
    stick(id, buttons, 0, 0)
}
fn stick(id: u16, buttons: u32, x: i16, y: i16) -> Input {
    Input::Controller {
        id,
        active: 1,
        buttons,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [x, y, 0, 0],
    }
}
fn key(key: u16, modifiers: u8, down: bool) -> Input {
    Input::Keyboard {
        key,
        modifiers,
        flags: 0,
        down,
    }
}
fn touch(event: u8, id: u32, x: f32, y: f32) -> Input {
    Input::Touch {
        event,
        id,
        x,
        y,
        pressure: 1.,
        major: 0.,
        minor: 0.,
        rotation: 0,
    }
}
fn mouse(button: u8, down: bool) -> Input {
    Input::MouseButton { button, down }
}
const CTRL_ALT: u8 = MODIFIER_CTRL | MODIFIER_ALT;

fn setup() -> (Arc<Shared>, Filter) {
    let shared = Arc::new(Shared::new());
    let filter = Filter::new(shared.clone(), OpenWith::default(), 1920, 1080);
    (shared, filter)
}
fn no_pointer() -> Option<(f32, f32)> {
    panic!("the cursor is read only for relative holds and clicks")
}
fn run(filter: &mut Filter, now: Instant, events: &[Input]) -> (Vec<Input>, Outcome) {
    let mut batch = events.to_vec();
    let out = filter.apply(now, &mut batch, no_pointer);
    (batch, out)
}

/// Opens with the pad chord at `t0`, returning the time it opened.
fn open_by_pad(filter: &mut Filter, t0: Instant) -> Instant {
    run(filter, t0, &[pad(0, PAD_CHORD)]);
    let (_, out) = run(filter, t0 + 1000 * MS, &[]);
    assert!(out.release_game);
    t0 + 1000 * MS
}

#[test]
fn couch_menu_filter_pad_chord_opens_at_1000_ms_not_999() {
    let t0 = Instant::now();
    let (shared, mut filter) = setup();
    let (passed, out) = run(&mut filter, t0, &[pad(0, crate::couch_menu::chord::LS_CLK)]);
    assert_eq!(passed.len(), 1);
    assert_eq!(out, Outcome::default());
    let (passed, _) = run(&mut filter, t0, &[pad(0, PAD_CHORD)]);
    assert_eq!(passed, [pad(0, PAD_CHORD)], "the game sees the clicks");
    assert_eq!(filter.next_due(), Some(t0 + 1000 * MS));
    let (_, out) = run(&mut filter, t0 + 999 * MS, &[]);
    assert_eq!(out, Outcome::default());
    assert!(!filter.is_open());
    let (_, out) = run(&mut filter, t0 + 1000 * MS, &[]);
    assert!(out.release_game && out.nav == [Nav::Opened]);
    assert!(filter.is_open() && Shared::is_open(shared.load()));
    assert_eq!(filter.next_due(), None);
}

#[test]
fn couch_menu_filter_extra_button_cancels_and_pads_are_separate() {
    let t0 = Instant::now();
    let (_, mut filter) = setup();
    run(&mut filter, t0, &[pad(0, PAD_CHORD), pad(3, PAD_CHORD)]);
    run(&mut filter, t0 + 500 * MS, &[pad(0, PAD_CHORD | BUTTON_A)]);
    let (_, out) = run(&mut filter, t0 + 1000 * MS, &[]);
    assert!(out.release_game, "pad 3 still held its chord");
    let (_, mut filter) = setup();
    run(&mut filter, t0, &[pad(1, PAD_CHORD)]);
    run(&mut filter, t0 + 999 * MS, &[pad(1, PAD_CHORD | DPAD_UP)]);
    let (_, out) = run(&mut filter, t0 + 1000 * MS, &[]);
    assert_eq!(out, Outcome::default());
    assert_eq!(filter.next_due(), None);
}

#[test]
fn couch_menu_filter_key_chord_with_and_without_shift() {
    let t0 = Instant::now();
    let (_, mut filter) = setup();
    let shifted = key(VK_BACK, CTRL_ALT | MODIFIER_SHIFT, true);
    let (passed, out) = run(&mut filter, t0, std::slice::from_ref(&shifted));
    assert_eq!(passed, [shifted]);
    assert!(!out.release_game && !filter.is_open());
    // Letting go of Shift while Backspace repeats is still the same press.
    let repeat = key(VK_BACK, CTRL_ALT, true);
    let up = key(VK_BACK, CTRL_ALT, false);
    let (passed, out) = run(&mut filter, t0, &[repeat.clone(), up.clone()]);
    assert_eq!(passed, [repeat, up]);
    assert!(!out.release_game && !filter.is_open());
    let before = key(0x41, 0, true);
    let after = key(0x42, 0, true);
    let chord = key(VK_BACK, CTRL_ALT, true);
    let (passed, out) = run(&mut filter, t0, &[before.clone(), chord.clone(), after]);
    assert_eq!(
        passed,
        [before],
        "the chord key and what follows it are consumed"
    );
    assert!(out.release_game && out.nav == [Nav::Opened]);
    // The same chord closes.
    let (passed, out) = run(&mut filter, t0, &[key(VK_BACK, CTRL_ALT, false), chord]);
    assert!(passed.is_empty());
    assert_eq!(out.nav, [Nav::Closed]);
    assert!(!filter.is_open());
}

#[test]
fn couch_menu_filter_corner_hold_opens_and_reads_the_cursor_only_when_due() {
    let t0 = Instant::now();
    let (_, mut filter) = setup();
    let mut batch = vec![touch(TOUCH_DOWN, 4, 0.01, 0.01)];
    filter.apply(t0, &mut batch, no_pointer);
    assert_eq!(batch.len(), 1, "the press reaches the desktop first");
    assert_eq!(filter.next_due(), Some(t0 + 1500 * MS));
    let (_, out) = run(&mut filter, t0 + 1499 * MS, &[]);
    assert!(!out.release_game);
    let (_, out) = run(&mut filter, t0 + 1500 * MS, &[]);
    assert!(out.release_game);

    // A relative mouse: one cursor read, when the hold is due.
    let (_, mut filter) = setup();
    let mut reads = 0;
    let mut batch = vec![Input::Relative { x: -3, y: 0 }, mouse(MOUSE_LEFT, true)];
    filter.apply(t0, &mut batch, || {
        reads += 1;
        None
    });
    assert_eq!((batch.len(), reads), (2, 0));
    filter.apply(t0 + 1499 * MS, &mut vec![], || {
        reads += 1;
        None
    });
    assert_eq!(reads, 0);
    let out = filter.apply(t0 + 1500 * MS, &mut vec![], || {
        reads += 1;
        Some((0.02, 0.02))
    });
    assert_eq!(reads, 1);
    assert!(out.release_game);
}

/// A small xorshift, so the property test needs no new dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    /// A stream position outside the corner box.
    fn spot(&mut self) -> f32 {
        0.1 + self.below(900) as f32 / 1000.
    }
}

/// Any input but a chord: no pad at exactly L3+R3, no Ctrl+Alt+Backspace and
/// no press in the corner box.
fn random_input(rng: &mut Rng) -> Input {
    match rng.below(13) {
        0 => Input::Relative {
            x: rng.next() as i16,
            y: rng.next() as i16,
        },
        1 => Input::Absolute {
            x: 200 + rng.below(1700) as i16,
            y: 120 + rng.below(900) as i16,
            width: 1920,
            height: 1080,
        },
        2 => mouse(1 + rng.below(5) as u8, rng.below(2) == 0),
        3 => Input::Scroll {
            amount: rng.next() as i16,
            horizontal: rng.below(2) == 0,
        },
        4 => {
            let mut k = rng.below(256) as u16;
            let modifiers = rng.next() as u8;
            if k == VK_BACK && modifiers & CTRL_ALT == CTRL_ALT {
                k = 0x41;
            }
            key(k, modifiers, rng.below(2) == 0)
        }
        5 => Input::Text("hé".into()),
        6 => {
            let mut buttons = rng.next() as u32 & 0x3f_ffff;
            if buttons == PAD_CHORD {
                buttons |= BUTTON_A;
            }
            Input::Controller {
                id: rng.below(16) as u16,
                active: rng.next() as u16,
                buttons,
                left_trigger: rng.next() as u8,
                right_trigger: rng.next() as u8,
                sticks: [
                    rng.next() as i16,
                    rng.next() as i16,
                    rng.next() as i16,
                    rng.next() as i16,
                ],
            }
        }
        7 => Input::Arrival {
            id: rng.below(16) as u8,
            kind: 1,
            capabilities: 0,
            buttons: 0,
        },
        8 => touch(
            rng.below(8) as u8,
            rng.below(4) as u32,
            rng.spot(),
            rng.spot(),
        ),
        9 => Input::Pen {
            event: rng.below(8) as u8,
            tool: 1,
            buttons: 0,
            x: rng.spot(),
            y: rng.spot(),
            pressure: 0.5,
            rotation: 0,
            tilt: 0,
        },
        10 => Input::ControllerTouch {
            id: 0,
            event: 1,
            touchpad: 0,
            pointer: 0,
            x: 0.5,
            y: 0.5,
            pressure: 1.,
        },
        11 => Input::Motion {
            id: 0,
            kind: 1,
            xyz: [0.1, 0.2, 0.3],
        },
        _ => Input::Battery {
            id: 0,
            state: 1,
            percent: 50,
        },
    }
}

#[test]
fn couch_menu_filter_closed_passes_random_streams_unchanged_and_in_order() {
    for seed in 1..=64u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let (shared, mut filter) = setup();
        let mut now = Instant::now();
        for _ in 0..40 {
            let batch: Vec<Input> = (0..rng.below(12)).map(|_| random_input(&mut rng)).collect();
            let mut events = batch.clone();
            // A relative-mouse hold may come due: the cursor is elsewhere.
            let out = filter.apply(now, &mut events, || Some((0.5, 0.5)));
            assert_eq!(events, batch, "seed {seed}");
            assert_eq!(out, Outcome::default(), "seed {seed}");
            now += rng.below(400) as u32 * MS;
        }
        assert!(!Shared::is_open(shared.load()));
    }
}

#[test]
fn couch_menu_filter_closed_does_not_allocate_and_has_nothing_due() {
    let t0 = Instant::now();
    let (_, mut filter) = setup();
    let mut rng = Rng(7);
    // No mouse button: a relative press would arm a corner hold.
    let mut events: Vec<Input> = std::iter::repeat_with(|| random_input(&mut rng))
        .filter(|e| !matches!(e, Input::MouseButton { .. } | Input::Text(_)))
        .take(256)
        .collect();
    events.push(touch(TOUCH_DOWN, 9, 0.5, 0.5));
    events.push(touch(TOUCH_UP, 9, 0.5, 0.5));
    let expected = events.clone();
    let capacity = events.capacity();
    let before = allocations();
    let out = filter.apply(t0, &mut events, no_pointer);
    let empty = filter.apply(t0 + 5000 * MS, &mut Vec::new(), no_pointer);
    assert_eq!(allocations(), before, "the closed path allocates nothing");
    assert_eq!(events.capacity(), capacity);
    assert_eq!(events, expected);
    assert_eq!(out.nav.capacity(), 0);
    assert_eq!(empty, Outcome::default());
    assert_eq!(filter.next_due(), None);
}

#[test]
fn couch_menu_filter_releases_once_per_open_and_consumes_while_open() {
    let t0 = Instant::now();
    let (shared, mut filter) = setup();
    let opened = open_by_pad(&mut filter, t0);
    let arrival = Input::Arrival {
        id: 5,
        kind: 1,
        capabilities: 0,
        buttons: 0,
    };
    let battery = Input::Battery {
        id: 0,
        state: 1,
        percent: 40,
    };
    let mut rng = Rng(99);
    let mut events: Vec<Input> = (0..200).map(|_| random_input(&mut rng)).collect();
    events.extend([arrival.clone(), battery.clone(), Input::Haptics(true)]);
    let expected: Vec<Input> = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                Input::Arrival { .. } | Input::Battery { .. } | Input::Haptics(_)
            )
        })
        .cloned()
        .collect();
    let out = filter.apply(opened, &mut events, || Some((0.5, 0.5)));
    assert_eq!(events, expected, "only Arrival, Battery and Haptics pass");
    assert!(!out.release_game, "release once per open");
    assert!(filter.is_open());
    for later in [10, 20, 30] {
        let (_, out) = run(&mut filter, opened + later * MS, &[]);
        assert!(!out.release_game);
    }
    // Close and open again through the native API: one more release.
    shared.close();
    let (_, out) = run(&mut filter, opened + 40 * MS, &[]);
    assert_eq!(out.nav, [Nav::Closed]);
    assert!(!out.release_game);
    shared.open().unwrap();
    let (_, out) = run(&mut filter, opened + 50 * MS, &[]);
    assert!(out.release_game && out.nav == [Nav::Opened]);
    let (_, out) = run(&mut filter, opened + 60 * MS, &[]);
    assert!(!out.release_game);
    // Closed and reopened between two passes: seen as a close and an open.
    shared.close();
    shared.open().unwrap();
    let (_, out) = run(&mut filter, opened + 70 * MS, &[]);
    assert!(out.release_game && out.nav == [Nav::Closed, Nav::Opened]);
}

#[test]
fn couch_menu_filter_masks_what_is_held_after_close_until_let_go() {
    let t0 = Instant::now();
    let (shared, mut filter) = setup();
    // Held from before the open: W, the left mouse button, a finger, A on pad 1.
    run(
        &mut filter,
        t0,
        &[
            key(0x57, 0, true),
            mouse(MOUSE_LEFT, true),
            touch(TOUCH_DOWN, 3, 0.5, 0.5),
            pad(1, BUTTON_A),
        ],
    );
    let opened = open_by_pad(&mut filter, t0);
    // Pressed while open: Enter, and X on pad 1.
    let (passed, _) = run(
        &mut filter,
        opened,
        &[key(VK_RETURN, 0, true), pad(1, BUTTON_A | 0x4000)],
    );
    assert!(passed.is_empty());
    shared.close();
    let (passed, out) = run(&mut filter, opened + 10 * MS, &[]);
    assert!(passed.is_empty() && out.nav == [Nav::Closed]);

    // Repeats and releases of held keys never reach the game; others do.
    let (passed, _) = run(
        &mut filter,
        opened + 20 * MS,
        &[
            key(0x57, 0, true),
            key(VK_RETURN, 0, false),
            key(0x41, 0, true),
            mouse(MOUSE_LEFT, false),
            mouse(MOUSE_RIGHT, true),
            touch(TOUCH_MOVE, 3, 0.6, 0.5),
            touch(TOUCH_UP, 3, 0.6, 0.5),
            touch(TOUCH_DOWN, 8, 0.6, 0.5),
        ],
    );
    assert_eq!(
        passed,
        [
            key(0x41, 0, true),
            mouse(MOUSE_RIGHT, true),
            touch(TOUCH_DOWN, 8, 0.6, 0.5)
        ]
    );
    // Pad buttons held at the close are masked bit by bit until each drops.
    let (passed, _) = run(
        &mut filter,
        opened + 30 * MS,
        &[pad(1, BUTTON_A | 0x4000 | DPAD_UP)],
    );
    assert_eq!(passed, [pad(1, DPAD_UP)]);
    let (passed, _) = run(&mut filter, opened + 40 * MS, &[pad(1, 0x4000)]);
    assert_eq!(passed, [pad(1, 0)], "A let go: no half press of X");
    let (passed, _) = run(
        &mut filter,
        opened + 50 * MS,
        &[pad(1, 0), pad(1, BUTTON_A)],
    );
    assert_eq!(
        passed,
        [pad(1, 0), pad(1, BUTTON_A)],
        "a new press of A passes"
    );
    // W let go: dropped (the game was released at the open), then W works
    // again. Pad 0 lets go of the chord that opened the menu.
    let (passed, _) = run(
        &mut filter,
        opened + 60 * MS,
        &[
            pad(0, 0),
            key(0x57, 0, false),
            key(0x57, 0, true),
            key(0x57, 0, false),
        ],
    );
    assert_eq!(passed, [pad(0, 0), key(0x57, 0, true), key(0x57, 0, false)]);
    assert!(!filter.masking, "everything held at the close was let go");
}

#[test]
fn couch_menu_filter_chord_keys_are_masked_after_a_keyboard_close() {
    let t0 = Instant::now();
    let (_, mut filter) = setup();
    let ctrl = key(0x11, MODIFIER_CTRL, true);
    let alt = key(0x12, CTRL_ALT, true);
    let chord = key(VK_BACK, CTRL_ALT, true);
    let (passed, _) = run(&mut filter, t0, &[ctrl.clone(), alt.clone(), chord.clone()]);
    assert_eq!(passed, [ctrl.clone(), alt.clone()]);
    let (passed, out) = run(&mut filter, t0, &[key(VK_BACK, CTRL_ALT, false), chord]);
    assert!(passed.is_empty() && out.nav == [Nav::Closed]);
    let (passed, _) = run(
        &mut filter,
        t0,
        &[
            key(VK_BACK, CTRL_ALT, false),
            key(0x12, MODIFIER_CTRL, false),
            key(0x11, 0, false),
        ],
    );
    assert!(passed.is_empty(), "no orphan ups: {passed:?}");
    assert!(!filter.masking);
}

#[test]
fn couch_menu_filter_navigation_and_repeat_timings() {
    let t0 = Instant::now();
    let (_, mut filter) = setup();
    let open = open_by_pad(&mut filter, t0);
    // The opening pad lets go of the chord.
    run(&mut filter, open, &[pad(0, 0)]);
    let (_, out) = run(&mut filter, open, &[pad(0, DPAD_DOWN)]);
    assert_eq!(out.nav, [Nav::Down]);
    assert_eq!(filter.next_due(), Some(open + 400 * MS));
    let (_, out) = run(&mut filter, open + 399 * MS, &[]);
    assert!(out.nav.is_empty());
    let (_, out) = run(&mut filter, open + 400 * MS, &[]);
    assert_eq!(out.nav, [Nav::Down]);
    assert_eq!(filter.next_due(), Some(open + 520 * MS));
    let (_, out) = run(&mut filter, open + 519 * MS, &[]);
    assert!(out.nav.is_empty());
    let (_, out) = run(&mut filter, open + 520 * MS, &[]);
    assert_eq!(out.nav, [Nav::Down]);
    // Same buttons again (a stick move elsewhere) neither repeats nor resets.
    let (_, out) = run(&mut filter, open + 600 * MS, &[stick(0, DPAD_DOWN, 0, 0)]);
    assert!(out.nav.is_empty());
    assert_eq!(filter.next_due(), Some(open + 640 * MS));
    // A late wake gives one step, not a burst.
    let (_, out) = run(&mut filter, open + 2000 * MS, &[]);
    assert_eq!(out.nav, [Nav::Down]);
    assert_eq!(filter.next_due(), Some(open + 2120 * MS));
    let (_, out) = run(&mut filter, open + 2050 * MS, &[pad(0, 0)]);
    assert!(out.nav.is_empty());
    assert_eq!(filter.next_due(), None);

    // The stick: over 50% starts, under 30% ends.
    let (_, out) = run(&mut filter, open, &[stick(0, 0, 0, 16_000)]);
    assert!(out.nav.is_empty());
    let (_, out) = run(&mut filter, open, &[stick(0, 0, 0, 20_000)]);
    assert_eq!(out.nav, [Nav::Up]);
    let (_, out) = run(&mut filter, open, &[stick(0, 0, 0, 10_000)]);
    assert!(out.nav.is_empty(), "still latched above 30%");
    let (_, out) = run(
        &mut filter,
        open,
        &[stick(0, 0, 0, 9_000), stick(0, 0, -30_000, 0)],
    );
    assert_eq!(out.nav, [Nav::Left]);
    let (_, out) = run(
        &mut filter,
        open,
        &[stick(0, 0, 0, 0), stick(0, 0, 0, -20_000)],
    );
    assert_eq!(out.nav, [Nav::Down]);

    // Buttons, keys, wheel, clicks and taps.
    let (_, out) = run(
        &mut filter,
        open,
        &[
            stick(0, BUTTON_A, 0, 0),
            pad(0, BUTTON_A | BUTTON_B),
            pad(0, 0),
            key(VK_UP, 0, true),
            key(VK_UP, 0, false),
            key(VK_RIGHT, 0, true),
            key(VK_SPACE, 0, true),
            key(VK_ESCAPE, 0, true),
            key(VK_BACK, 0, true),
            Input::Scroll {
                amount: -120,
                horizontal: false,
            },
            Input::Absolute {
                x: 960,
                y: 270,
                width: 1920,
                height: 1080,
            },
            mouse(MOUSE_LEFT, true),
            mouse(MOUSE_RIGHT, true),
            touch(TOUCH_DOWN, 2, 0.25, 0.75),
        ],
    );
    assert_eq!(
        out.nav,
        [
            Nav::Accept,
            Nav::Back,
            Nav::Up,
            Nav::Right,
            Nav::Accept,
            Nav::Back,
            Nav::Back,
            Nav::Down,
            Nav::Point { x: 0.5, y: 0.25 },
            Nav::Back,
            Nav::Point { x: 0.25, y: 0.75 },
        ]
    );
    // A relative mouse clicks at the host cursor.
    let mut batch = vec![Input::Relative { x: 1, y: 1 }, mouse(MOUSE_LEFT, true)];
    let out = filter.apply(open, &mut batch, || Some((0.1, 0.2)));
    assert_eq!(out.nav, [Nav::Point { x: 0.1, y: 0.2 }]);
}

#[test]
fn couch_menu_filter_refusal_is_reported_once_and_the_key_passes() {
    let t0 = Instant::now();
    let (shared, mut filter) = setup();
    shared.refuse(Some("couch menu: needs wgc_direct_encoder_input".into()));
    let chord = key(VK_BACK, CTRL_ALT, true);
    let (passed, out) = run(&mut filter, t0, std::slice::from_ref(&chord));
    assert_eq!(passed, std::slice::from_ref(&chord));
    assert_eq!(
        out.refused.as_deref(),
        Some("couch menu: needs wgc_direct_encoder_input")
    );
    assert!(!out.release_game);
    let (_, out) = run(&mut filter, t0, &[chord]);
    assert_eq!(out.refused, None, "once per session");
    run(&mut filter, t0, &[pad(0, PAD_CHORD)]);
    let (_, out) = run(&mut filter, t0 + 1000 * MS, &[]);
    assert_eq!(out, Outcome::default());
}

#[test]
fn couch_menu_filter_open_with_turns_ways_off() {
    let t0 = Instant::now();
    let shared = Arc::new(Shared::new());
    let none = OpenWith::parse("").unwrap();
    let mut filter = Filter::new(shared.clone(), none, 1920, 1080);
    let events = [
        key(VK_BACK, CTRL_ALT, true),
        pad(0, PAD_CHORD),
        touch(TOUCH_DOWN, 1, 0.01, 0.01),
    ];
    let (passed, _) = run(&mut filter, t0, &events);
    assert_eq!(passed, events);
    assert_eq!(filter.next_due(), None);
    let (_, out) = run(&mut filter, t0 + 2000 * MS, &[]);
    assert_eq!(out, Outcome::default());
    // Opened through the native API, Back still closes it.
    shared.open().unwrap();
    let (_, out) = run(&mut filter, t0, &[key(VK_ESCAPE, 0, true)]);
    assert_eq!(out.nav, [Nav::Opened, Nav::Back]);
}

#[test]
fn couch_menu_filter_a_second_pad_holding_the_chord_waits_for_a_change() {
    let t0 = Instant::now();
    let (_, mut filter) = setup();
    run(&mut filter, t0, &[pad(0, PAD_CHORD)]);
    run(&mut filter, t0 + 500 * MS, &[pad(1, PAD_CHORD)]);
    let (_, out) = run(&mut filter, t0 + 1000 * MS, &[]);
    assert!(out.release_game && filter.is_open(), "pad 0 opened it");
    // Pad 1 still reports L3+R3: the open cancelled its hold, and it must
    // not arm again and close the menu a second later.
    for ms in [1100, 1600, 2100, 2600, 3100] {
        let (_, out) = run(&mut filter, t0 + ms * MS, &[pad(1, PAD_CHORD)]);
        assert!(!out.nav.contains(&Nav::Closed), "at {ms} ms");
        assert!(filter.is_open(), "at {ms} ms");
    }
    // Let go and hold again: a new hold, which closes it.
    run(&mut filter, t0 + 3200 * MS, &[pad(0, 0), pad(1, 0)]);
    run(&mut filter, t0 + 3300 * MS, &[pad(1, PAD_CHORD)]);
    run(&mut filter, t0 + 4300 * MS, &[]);
    assert!(!filter.is_open());
}

fn custom(controller: &str, hold_ms: &str, keyboard: &str) -> Filter {
    let open_with = OpenWith {
        pad_chord: PadButtons::parse(controller).unwrap(),
        pad_hold: parse_hold("couch_menu_hold_ms", hold_ms, PAD_HOLD_MS).unwrap(),
        key_chord: KeyChord::parse(keyboard).unwrap(),
        ..OpenWith::default()
    };
    Filter::new(Arc::new(Shared::new()), open_with, 1920, 1080)
}

#[test]
fn couch_menu_filter_custom_single_button_hold_opens_at_its_time_and_closes() {
    const SELECT: u32 = 0x0020;
    let t0 = Instant::now();
    let mut filter = custom("select", "300", "ctrl+alt+backspace");
    // L3+R3 is an ordinary press now.
    run(&mut filter, t0, &[pad(0, PAD_CHORD)]);
    assert_eq!(filter.next_due(), None);
    let (passed, _) = run(&mut filter, t0, &[pad(0, SELECT)]);
    assert_eq!(passed, [pad(0, SELECT)], "the game sees the press");
    assert_eq!(filter.next_due(), Some(t0 + 300 * MS));
    let (_, out) = run(&mut filter, t0 + 299 * MS, &[]);
    assert_eq!(out, Outcome::default());
    let (_, out) = run(&mut filter, t0 + 300 * MS, &[]);
    assert!(out.release_game && filter.is_open());
    // Still held: no close until it is let go and held again.
    run(&mut filter, t0 + 900 * MS, &[pad(0, SELECT)]);
    assert!(filter.is_open());
    run(&mut filter, t0 + 1000 * MS, &[pad(0, 0)]);
    run(&mut filter, t0 + 1100 * MS, &[pad(0, SELECT)]);
    run(&mut filter, t0 + 1399 * MS, &[]);
    assert!(filter.is_open());
    let (_, out) = run(&mut filter, t0 + 1400 * MS, &[]);
    assert_eq!(out.nav, [Nav::Closed]);
    // An extra button cancels a hold.
    run(&mut filter, t0 + 2000 * MS, &[pad(0, 0), pad(0, SELECT)]);
    run(&mut filter, t0 + 2100 * MS, &[pad(0, SELECT | BUTTON_A)]);
    let (_, out) = run(&mut filter, t0 + 3000 * MS, &[]);
    assert_eq!(out, Outcome::default());
    assert!(!filter.is_open());
}

#[test]
fn couch_menu_filter_custom_key_chord_replaces_the_default() {
    const VK_HOME: u16 = 0x24;
    let t0 = Instant::now();
    let mut filter = custom("l3+r3", "1000", "ctrl+alt+home");
    let old = key(VK_BACK, CTRL_ALT, true);
    let (passed, out) = run(&mut filter, t0, std::slice::from_ref(&old));
    assert_eq!(passed, [old], "Ctrl+Alt+Backspace goes to the game");
    assert!(!out.release_game && !filter.is_open());
    let shifted = key(VK_HOME, CTRL_ALT | MODIFIER_SHIFT, true);
    let (passed, _) = run(&mut filter, t0, std::slice::from_ref(&shifted));
    assert_eq!(passed, [shifted]);
    let (passed, out) = run(
        &mut filter,
        t0,
        &[
            key(VK_HOME, CTRL_ALT | MODIFIER_SHIFT, false),
            key(VK_HOME, CTRL_ALT, true),
        ],
    );
    assert_eq!(passed, [key(VK_HOME, CTRL_ALT | MODIFIER_SHIFT, false)]);
    assert!(out.release_game && filter.is_open());
    // In the open menu Backspace is Back, and the custom chord closes.
    let (_, out) = run(&mut filter, t0, &[key(VK_BACK, CTRL_ALT, true)]);
    assert_eq!(out.nav, [Nav::Back]);
    let (_, out) = run(
        &mut filter,
        t0,
        &[key(VK_HOME, CTRL_ALT, false), key(VK_HOME, CTRL_ALT, true)],
    );
    assert_eq!(out.nav, [Nav::Closed]);
}

#[test]
fn couch_menu_filter_key_chord_repeats_do_not_toggle_again() {
    let t0 = Instant::now();
    let (shared, mut filter) = setup();
    let chord = key(VK_BACK, CTRL_ALT, true);
    let (passed, out) = run(&mut filter, t0, &[chord.clone(), chord.clone()]);
    assert!(passed.is_empty());
    assert_eq!(
        out.nav,
        [Nav::Opened],
        "the repeat neither closes nor goes Back"
    );
    // Auto-repeat keeps sending key-downs while the key is held.
    for ms in [500, 533, 566] {
        let (passed, out) = run(&mut filter, t0 + ms * MS, std::slice::from_ref(&chord));
        assert!(passed.is_empty() && out.nav.is_empty(), "at {ms} ms");
        assert!(
            filter.is_open() && Shared::is_open(shared.load()),
            "at {ms} ms"
        );
    }
    // Let go and press again: that closes, and its repeats do not reopen.
    let (_, out) = run(
        &mut filter,
        t0 + 600 * MS,
        &[key(VK_BACK, CTRL_ALT, false), chord.clone(), chord.clone()],
    );
    assert_eq!(out.nav, [Nav::Closed]);
    let (passed, out) = run(&mut filter, t0 + 700 * MS, std::slice::from_ref(&chord));
    assert!(passed.is_empty(), "the held key stays masked");
    assert_eq!(out, Outcome::default());
    assert!(!filter.is_open());
}

#[test]
fn couch_menu_filter_custom_corner_hold() {
    let t0 = Instant::now();
    let open_with = OpenWith {
        corner_hold: 500 * MS,
        ..OpenWith::default()
    };
    let mut filter = Filter::new(Arc::new(Shared::new()), open_with, 1920, 1080);
    run(&mut filter, t0, &[touch(TOUCH_DOWN, 1, 0.01, 0.01)]);
    assert_eq!(filter.next_due(), Some(t0 + 500 * MS));
    let (_, out) = run(&mut filter, t0 + 499 * MS, &[]);
    assert_eq!(out, Outcome::default());
    let (_, out) = run(&mut filter, t0 + 500 * MS, &[]);
    assert!(out.release_game && filter.is_open());
}
