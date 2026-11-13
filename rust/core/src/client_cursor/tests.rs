use super::*;

fn request(enabled: bool, max_edge: u16) -> [u8; 4] {
    CursorShapeRequest { enabled, max_edge }.encode()
}

fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Arc<[u8]> {
    rgba.repeat((width * height) as usize).into()
}

fn pointer(pixels: &Arc<[u8]>, width: u32, height: u32) -> Pointer<'_> {
    Pointer {
        width,
        height,
        hotspot: [1, 2],
        logic: false,
        pixels,
    }
}

fn scale(percent: u32) -> impl FnOnce(&str) -> Option<anyhow::Result<u32>> {
    move |_| Some(Ok(percent))
}

fn decoded(message: Option<Vec<u8>>) -> CursorShape {
    CursorShape::decode(&message.expect("a message")).expect("a valid message")
}

/// Without an offered enable nothing is noted, sent or left out of the video; a disable
/// ends the session.
#[test]
fn only_an_offered_enable_starts_a_session() {
    let cursor = ClientCursor::default();
    let arrow = solid(4, 4, [10, 20, 30, 255]);
    let now = Instant::now();
    assert_eq!(cursor.on_request(&request(true, 96), false, 1), None);
    assert_eq!(cursor.on_request(&[1, 2, 96, 0], true, 1), None);
    assert_eq!(cursor.on_request(&request(true, 97)[..], true, 1), None);
    assert!(!cursor.active());
    assert!(!cursor.note(Some(pointer(&arrow, 4, 4)), "D1"));
    assert_eq!(cursor.take(1, now, scale(100)), None);
    assert_eq!(cursor.on_request(&request(true, 96), true, 1), Some(true));
    assert!(cursor.active());
    assert!(cursor.note(Some(pointer(&arrow, 4, 4)), "D1"));
    // Another control peer on the session gets nothing.
    assert_eq!(cursor.take(2, now, scale(100)), None);
    assert!(matches!(
        decoded(cursor.take(1, now, scale(100))),
        CursorShape::Image { .. }
    ));
    // A disable is taken after the offer went too.
    assert_eq!(
        cursor.on_request(&request(false, 96), false, 1),
        Some(false)
    );
    assert!(!cursor.active());
    assert!(!cursor.note(None, "D1"));
    assert_eq!(cursor.take(1, now + INTERVAL, scale(100)), None);
    // Only the peer that enabled it ends the session, by a disable or when it goes.
    cursor.on_request(&request(true, 96), true, 1);
    assert_eq!(cursor.on_request(&request(false, 96), true, 2), Some(false));
    cursor.end(2);
    assert!(cursor.active());
    cursor.end(1);
    assert!(!cursor.active());
}

/// A shape is sent once a session and shown again by id; it is sent again after it
/// dropped out of the device's 32 most recent, and after a fresh enable.
#[test]
fn shapes_are_cached_by_id() {
    let cursor = ClientCursor::default();
    let mut now = Instant::now();
    cursor.on_request(&request(true, 96), true, 1);
    let shapes: Vec<Arc<[u8]>> = (0..=CACHE_ENTRIES as u8)
        .map(|n| solid(2, 3, [n, 0, 0, 255]))
        .collect();
    let show = |pixels: &Arc<[u8]>, now: Instant| {
        assert!(cursor.note(Some(pointer(pixels, 2, 3)), "D1"));
        decoded(cursor.take(1, now, scale(150)))
    };
    let CursorShape::Image { id: first, image } = show(&shapes[0], now) else {
        panic!("first shape not defined");
    };
    assert_eq!(
        (
            image.width,
            image.height,
            image.hotspot_x,
            image.hotspot_y,
            image.scale
        ),
        (2, 3, 1, 2, 150)
    );
    assert_eq!(image.pixels, shapes[0].to_vec());
    now += INTERVAL;
    assert!(matches!(show(&shapes[1], now), CursorShape::Image { .. }));
    now += INTERVAL;
    // A copy of the first shape in another allocation is the same image.
    let copy: Arc<[u8]> = shapes[0].to_vec().into();
    assert_eq!(show(&copy, now), CursorShape::Show { id: first });
    // 32 other shapes push the first out of the device's cache.
    for shape in &shapes[1..] {
        now += INTERVAL;
        show(shape, now);
    }
    now += INTERVAL;
    assert!(matches!(show(&shapes[0], now), CursorShape::Image { id, .. } if id == first));
    now += INTERVAL;
    assert!(matches!(
        show(&shapes[CACHE_ENTRIES], now),
        CursorShape::Show { .. }
    ));
    // A fresh enable forgets what the device cached.
    cursor.on_request(&request(true, 96), true, 1);
    now += INTERVAL;
    assert!(matches!(
        show(&shapes[CACHE_ENTRIES], now),
        CursorShape::Image { .. }
    ));
}

/// The first change is sent at once, then the newest at most every interval; the same
/// pointer noted again or a change back to what the device shows sends nothing.
#[test]
fn changes_are_coalesced() {
    let cursor = ClientCursor::default();
    let start = Instant::now();
    cursor.on_request(&request(true, 96), true, 1);
    assert_eq!(cursor.next_due(), None);
    let arrow = solid(4, 4, [1, 1, 1, 255]);
    let beam = solid(4, 4, [2, 2, 2, 255]);
    let hand = solid(4, 4, [3, 3, 3, 255]);
    assert!(cursor.note(None, "D1"));
    assert_eq!(
        decoded(cursor.take(1, start, scale(100))),
        CursorShape::Hide
    );
    assert!(!cursor.note(None, "D1"));
    assert!(cursor.note(Some(pointer(&arrow, 4, 4)), "D1"));
    assert!(!cursor.note(Some(pointer(&arrow, 4, 4)), "D1"));
    assert_eq!(cursor.next_due(), Some(start + INTERVAL));
    assert_eq!(cursor.take(1, start + INTERVAL / 2, scale(100)), None);
    assert!(cursor.note(Some(pointer(&beam, 4, 4)), "D1"));
    assert!(cursor.note(Some(pointer(&hand, 4, 4)), "D1"));
    let CursorShape::Image { image, .. } = decoded(cursor.take(1, start + INTERVAL, scale(100)))
    else {
        panic!("newest shape not sent");
    };
    assert_eq!(image.pixels, hand.to_vec());
    assert_eq!(cursor.next_due(), None);
    assert_eq!(cursor.take(1, start + INTERVAL * 3, scale(100)), None);
    // Hidden and back within an interval: the device already shows the hand.
    assert!(cursor.note(None, "D1"));
    assert!(cursor.note(Some(pointer(&hand, 4, 4)), "D1"));
    assert_eq!(cursor.take(1, start + INTERVAL * 4, scale(100)), None);
}

/// The scale is read once a display, and a failed read sends 100%.
#[test]
fn the_scale_is_read_per_display() {
    let cursor = ClientCursor::default();
    let mut now = Instant::now();
    cursor.on_request(&request(true, 96), true, 1);
    let a = solid(4, 4, [1, 1, 1, 255]);
    let b = solid(4, 4, [2, 2, 2, 255]);
    let scale_of = |shape: CursorShape| match shape {
        CursorShape::Image { image, .. } => image.scale,
        other => panic!("{other:?}"),
    };
    cursor.note(Some(pointer(&a, 4, 4)), "D1");
    assert_eq!(scale_of(decoded(cursor.take(1, now, scale(250)))), 250);
    cursor.note(Some(pointer(&b, 4, 4)), "D1");
    now += INTERVAL;
    assert_eq!(
        scale_of(decoded(cursor.take(1, now, |_| panic!("read again")))),
        250
    );
    cursor.note(Some(pointer(&a, 4, 4)), "D2");
    now += INTERVAL;
    assert_eq!(
        scale_of(decoded(
            cursor.take(1, now, |_| Some(Err(anyhow::anyhow!("gone"))))
        )),
        100
    );
    cursor.note(Some(pointer(&b, 4, 4)), "D3");
    now += INTERVAL;
    assert_eq!(
        scale_of(decoded(cursor.take(1, now, scale(900)))),
        MAX_SCALE
    );
}

#[test]
fn a_shape_waits_while_its_display_scale_is_read_elsewhere() {
    let cursor = ClientCursor::default();
    let now = Instant::now();
    cursor.on_request(&request(true, 96), true, 1);
    let a = solid(4, 4, [1, 1, 1, 255]);
    cursor.note(Some(pointer(&a, 4, 4)), "D1");
    // The control thread never waits for the lookup: the shape stays pending,
    // with no deadline that would spin the loop meanwhile.
    assert_eq!(cursor.take(1, now, |_| None), None);
    assert_eq!(cursor.next_due(), None);
    assert_eq!(cursor.take(1, now, |_| None), None);
    match decoded(cursor.take(1, now, scale(150))) {
        CursorShape::Image { image, .. } => assert_eq!(image.scale, 150),
        other => panic!("{other:?}"),
    }
    // Hiding the pointer needs no scale.
    cursor.note(None, "D2");
    assert_eq!(
        decoded(cursor.take(1, now + INTERVAL, |_| panic!("no scale needed"))),
        CursorShape::Hide
    );
    // A display change after a sent shape: its deadline has passed, yet while the
    // new display's scale is read the loop gets no deadline to spin on.
    cursor.note(Some(pointer(&a, 4, 4)), "D3");
    assert_eq!(cursor.next_due(), Some(now + INTERVAL * 2));
    assert_eq!(cursor.take(1, now + INTERVAL * 3, |_| None), None);
    assert_eq!(cursor.next_due(), None);
    // The next take after the reader's wake sends it.
    assert!(matches!(
        decoded(cursor.take(1, now + INTERVAL * 3, scale(125))),
        CursorShape::Image { .. } | CursorShape::Show { .. }
    ));
    assert_eq!(cursor.next_due(), None);
}

fn converted(noted: Noted, max_edge: u16, scale: u16) -> CursorImage {
    match image(&noted, max_edge, scale) {
        Converted::Image(image) => image,
        Converted::Hidden => panic!("hidden"),
        Converted::Unusable(why) => panic!("{why}"),
    }
}

fn noted(width: u32, height: u32, logic: bool, pixels: Vec<u8>) -> Noted {
    Noted {
        width,
        height,
        hotspot: [0, 0],
        logic,
        pixels: pixels.into(),
        output: "D1".into(),
    }
}

/// Inverting pixels become black with a white outline, a transparent XOR is left out, and
/// opaque logic pixels keep their color.
#[test]
fn inverting_pixels_become_black_with_a_white_outline() {
    // A 3x3 monochrome shape: one inverting pixel in the middle, a no-op XOR at the top
    // left, an opaque red one at the bottom right.
    let mut pixels = vec![0u8; 9 * 4];
    pixels[4 * 4..4 * 4 + 4].copy_from_slice(&[255, 255, 255, 128]);
    pixels[..4].copy_from_slice(&[0, 0, 0, 128]);
    pixels[8 * 4..].copy_from_slice(&[255, 0, 0, 255]);
    let image = converted(noted(3, 3, true, pixels), 96, 100);
    let at = |n: usize| &image.pixels[n * 4..n * 4 + 4];
    assert_eq!(at(4), [0, 0, 0, 255]);
    for outline in [0, 1, 2, 3, 5, 6, 7] {
        assert_eq!(at(outline), [255, 255, 255, 255], "{outline}");
    }
    assert_eq!(at(8), [255, 0, 0, 255]);
    // A color shape keeps its straight alpha.
    let glass = converted(noted(1, 1, false, vec![9, 8, 7, 100]), 96, 100);
    assert_eq!(glass.pixels, [9, 8, 7, 100]);
}

/// A shape past the device's edge is shrunk with its hotspot and scale; an invisible one
/// hides the pointer and a malformed one is not sent.
#[test]
fn large_and_unusable_shapes() {
    let mut big = noted(128, 64, false, [0, 0, 255, 255].repeat(128 * 64));
    big.hotspot = [127, 32];
    let image = converted(big.clone(), 96, 200);
    assert_eq!((image.width, image.height), (96, 48));
    assert_eq!((image.hotspot_x, image.hotspot_y), (95, 24));
    assert_eq!(image.scale, 150);
    assert!(
        image
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [0, 0, 255, 255])
    );
    // Shrunk below 100% the scale stays at the least the protocol takes.
    let image = converted(big, 32, 100);
    assert_eq!((image.width, image.height, image.scale), (32, 16, 100));
    // Half transparent: the color is weighted by alpha.
    let mut pixels = [0u8; 8];
    pixels[..4].copy_from_slice(&[200, 100, 50, 255]);
    let image = converted(noted(2, 1, false, pixels.to_vec()), 1, 100);
    assert_eq!(image.pixels, [200, 100, 50, 128]);
    assert!(matches!(
        image_of(noted(2, 2, false, vec![0; 16])),
        Converted::Hidden
    ));
    assert!(matches!(
        image_of(noted(2, 2, true, [0, 0, 0, 128].repeat(4))),
        Converted::Hidden
    ));
    assert!(matches!(
        image_of(noted(2, 2, false, vec![255; 12])),
        Converted::Unusable(_)
    ));
    assert!(matches!(
        image_of(noted(0, 2, false, Vec::new())),
        Converted::Unusable(_)
    ));
    // Hidden shapes are sent as HIDE, malformed ones as DEFAULT.
    let cursor = ClientCursor::default();
    cursor.on_request(&request(true, 96), true, 1);
    let clear: Arc<[u8]> = vec![0; 16].into();
    cursor.note(Some(pointer(&clear, 2, 2)), "D1");
    let now = Instant::now();
    assert_eq!(decoded(cursor.take(1, now, scale(100))), CursorShape::Hide);
    let short: Arc<[u8]> = vec![255; 12].into();
    cursor.note(Some(pointer(&short, 2, 2)), "D1");
    assert_eq!(
        decoded(cursor.take(1, now + INTERVAL, scale(100))),
        CursorShape::Default
    );
}

fn image_of(noted: Noted) -> Converted {
    image(&noted, 96, 100)
}
