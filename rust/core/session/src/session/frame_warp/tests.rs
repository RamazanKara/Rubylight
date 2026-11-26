//! A stream's Frame Warp without waits: requests, input, tags, metas and the worker.

use super::*;
use butterpollo_media_policy::frame_warp::{
    FRAME_WARP_REQUEST_MESSAGE_TYPE, FrameWarpRequest, FrameWarpV2Meta, Model, Tags, Thumbnail,
    meta_flags, thumbnail_height,
};
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

fn scene(width: usize, height: usize, ox: f32, oy: f32, at_us: u64) -> Thumbnail {
    let noise = |u: f32, v: f32, cell: f32, seed: u32| -> f32 {
        let lattice = |x: i32, y: i32| {
            let mut h = (x as u32).wrapping_mul(0x8da6_b343)
                ^ (y as u32).wrapping_mul(0xd816_3841)
                ^ seed.wrapping_mul(0xcb1a_b31f);
            h ^= h >> 13;
            h = h.wrapping_mul(0x5bd1_e995);
            h ^= h >> 15;
            (h & 0xffff) as f32 / 65535.0
        };
        let (fu, fv) = (u / cell, v / cell);
        let (x0, y0) = (fu.floor(), fv.floor());
        let (tx, ty) = (fu - x0, fv - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let top = lattice(x0, y0) * (1.0 - tx) + lattice(x0 + 1, y0) * tx;
        let bottom = lattice(x0, y0 + 1) * (1.0 - tx) + lattice(x0 + 1, y0 + 1) * tx;
        top * (1.0 - ty) + bottom * ty
    };
    let world = |u: f32, v: f32| -> u8 {
        let value = 128.0
            + 70.0 * (noise(u, v, 4.0, 1) - 0.5)
            + 60.0 * (noise(u, v, 9.0, 2) - 0.5)
            + 50.0 * (noise(u, v, 23.0, 3) - 0.5);
        value.clamp(0.0, 255.0) as u8
    };
    let mut luma = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            luma.push(world(x as f32 + ox, y as f32 + oy));
        }
    }
    Thumbnail {
        width,
        height,
        luma,
        at_us,
    }
}

/// A version 2 request, as the host takes no other.
fn request(enabled: bool, controller: Option<u8>, mask_max_width: u16) -> [u8; 12] {
    RequestV2::new(FrameWarpRequest {
        enabled,
        controller,
        mask_max_width,
        lens: true,
    })
    .encode()
}

/// An older Nightjar's request: version 1, eight bytes, on, no pad, the lens, no mask.
const VERSION_1: [u8; 8] = [1, 1, 0xFF, 1, 0, 0, 0, 0];

fn state() -> (Instant, FrameWarpState) {
    let epoch = Instant::now();
    (epoch, FrameWarpState::new((1920, 1080), epoch))
}

#[test]
fn nothing_happens_until_a_client_asks() {
    let (epoch, warp) = state();
    assert_eq!(FRAME_WARP_REQUEST_MESSAGE_TYPE, 0x5543);
    assert!(!warp.enabled());
    warp.on_mouse(5, 5, epoch);
    warp.tag(1, Some(epoch));
    assert!(warp.take_outgoing().is_empty());
    assert!(lock(&warp.inbox).is_empty());
    assert_eq!(warp.on_request(&[1, 2]), None);
    assert!(!warp.enabled());
}

#[test]
fn a_request_turns_it_on_and_input_reaches_the_tags() {
    let (epoch, warp) = state();
    let wakes = Arc::new(AtomicUsize::new(0));
    let counter = wakes.clone();
    warp.set_wake(Arc::new(move || {
        counter.fetch_add(1, Ordering::Relaxed);
    }));
    assert_eq!(warp.on_request(&request(true, Some(0), 1024)), Some(true));
    assert!(warp.enabled());
    // The client counts from its request: it hears a model at once.
    let first = warp.take_outgoing();
    assert!(first.model.is_some());
    assert!(wakes.load(Ordering::Relaxed) >= 1);
    warp.on_mouse(10, -3, epoch + Duration::from_millis(1));
    warp.on_mouse(5, 1, epoch + Duration::from_millis(2));
    warp.on_controller(0, (1000, 0), epoch + Duration::from_millis(3));
    // Captured well after the input and the default lag.
    warp.tag(7, Some(epoch + Duration::from_millis(200)));
    let sent = warp.take_outgoing();
    let [tags] = sent.tags.as_slice() else {
        panic!("one tags message, not {}", sent.tags.len());
    };
    let tags = Tags::decode(tags).unwrap();
    let tag = tags.tags.last().unwrap();
    assert_eq!((tag.frame, tag.mouse_x, tag.mouse_y), (7, 15, -2));
    assert_eq!(tag.controller_seq, 1);
    // A repeated picture shows the last capture's input.
    warp.on_mouse(100, 0, epoch + Duration::from_millis(300));
    warp.tag(8, None);
    let sent = warp.take_outgoing();
    let tags = Tags::decode(sent.tags.last().unwrap()).unwrap();
    let tag = tags.tags.last().unwrap();
    assert_eq!((tag.frame, tag.mouse_x), (8, 15));
    // The picture halved until it fits the client's 1024.
    assert_eq!(warp.params().mask_width, 960);
}

#[test]
fn events_wait_for_a_busy_host_and_keep_their_order() {
    let (epoch, warp) = state();
    warp.on_request(&request(true, None, 0));
    let _ = warp.take_outgoing();
    // While the worker holds the host, input and tags only queue.
    warp.with(|_| {
        warp.on_mouse(4, 0, epoch + Duration::from_millis(1));
        warp.tag(3, Some(epoch + Duration::from_millis(500)));
        assert_eq!(lock(&warp.inbox).len(), 2);
    });
    assert!(lock(&warp.inbox).is_empty());
    let sent = warp.take_outgoing();
    let tags = Tags::decode(sent.tags.last().unwrap()).unwrap();
    let tag = tags.tags.last().unwrap();
    assert_eq!((tag.frame, tag.mouse_x), (3, 4));
}

#[test]
fn turning_it_off_drops_what_was_waiting() {
    let (epoch, warp) = state();
    warp.on_request(&request(true, None, 0));
    warp.tag(1, Some(epoch));
    assert_eq!(warp.on_request(&request(false, None, 0)), Some(false));
    assert!(!warp.enabled());
    assert!(warp.take_outgoing().is_empty());
    warp.on_mouse(1, 1, epoch);
    warp.tag(2, Some(epoch));
    assert!(warp.take_outgoing().is_empty());
    assert!(!warp.status().enabled);
}

#[test]
fn tags_only_queue_and_wake_the_control_loop_once_until_it_takes_them() {
    let (epoch, warp) = state();
    let wakes = Arc::new(AtomicUsize::new(0));
    let counter = wakes.clone();
    warp.set_wake(Arc::new(move || {
        counter.fetch_add(1, Ordering::Relaxed);
    }));
    warp.on_request(&request(true, None, 0));
    let _ = warp.take_outgoing();
    let woken = || wakes.load(Ordering::Relaxed);
    let before = woken();
    warp.tag(1, Some(epoch + Duration::from_millis(1)));
    warp.tag(2, Some(epoch + Duration::from_millis(2)));
    // Queued, not made: the frame path does not take the host.
    assert_eq!(lock(&warp.inbox).len(), 2);
    assert_eq!(woken() - before, 1);
    let sent = warp.take_outgoing();
    assert_eq!(sent.tags.len(), 2);
    let tags = Tags::decode(&sent.tags[1]).unwrap();
    assert_eq!(
        tags.tags.iter().map(|t| t.frame).collect::<Vec<_>>(),
        [1, 2]
    );
    assert!(lock(&warp.inbox).is_empty());
    // Taken: the next tag wakes the control loop again.
    warp.tag(3, Some(epoch + Duration::from_millis(3)));
    assert_eq!(woken() - before, 2);
}

#[test]
fn the_worker_learns_from_thumbnails_and_the_model_goes_out() {
    let (epoch, warp) = state();
    warp.on_request(&request(true, Some(0), 480));
    let _ = warp.take_outgoing();
    let mut measurer = Measurer::default();
    let h = thumbnail_height((1920, 1080));
    // The camera pans 2 thumbnail pixels per 8 ms frame, for 40 ms of mouse input each.
    let (mut t, mut sum, mut learned) = (0u64, 0, Learned::default());
    while t < 3_000_000 {
        warp.on_mouse(16, 0, epoch + Duration::from_micros(t));
        sum += 16;
        let offset = (sum as f32 - 16.0 * 5.0).max(0.0) * 0.125;
        learned = warp.on_thumbnail(&mut measurer, scene(256, h, offset, 0.0, t), t);
        t += 8_000;
    }
    let model = learned.model.expect("a model");
    assert!(model.mouse_locked, "{model:?}");
    assert!((model.mouse_gain[0] + 0.9375).abs() < 0.08, "{model:?}");
    assert!(learned.prior.is_some_and(|p| p.base.mouse_gain.is_some()));
    let sent = warp
        .take_outgoing()
        .model
        .map(|m| Model::decode(&m).unwrap());
    assert_eq!(sent, Some(model));
    assert_eq!(warp.status().model, Some(model));
}

#[test]
fn the_prior_is_claimed_once() {
    let (_, warp) = state();
    assert!(warp.claim_prior());
    assert!(!warp.claim_prior());
}

#[test]
fn the_clock_counts_from_the_epoch() {
    let (epoch, warp) = state();
    assert_eq!(warp.us(epoch), 0);
    assert_eq!(warp.us(epoch + Duration::from_micros(1500)), 1500);
    if let Some(before) = epoch.checked_sub(Duration::from_millis(1)) {
        assert_eq!(warp.us(before), 0);
    }
}

#[test]
fn mouse_and_controller_input_never_self_wake_the_control_loop() {
    let (epoch, warp) = state();
    let wakes = Arc::new(AtomicUsize::new(0));
    let counter = wakes.clone();
    warp.set_wake(Arc::new(move || {
        counter.fetch_add(1, Ordering::Relaxed);
    }));
    warp.on_request(&request(true, Some(0), 0));
    let _ = warp.take_outgoing();
    let before = wakes.load(Ordering::Relaxed);

    // Even with a queued tag ready to collect into the outbox, control-thread
    // input applies quietly without sending a wake to itself.
    warp.queue(Event::Tag {
        frame: 1,
        capture_us: 1000,
    });
    warp.on_mouse(8, -4, epoch + Duration::from_millis(1));
    warp.on_controller(0, (1000, -500), epoch + Duration::from_millis(2));
    assert_eq!(wakes.load(Ordering::Relaxed), before);
    assert!(lock(&warp.inbox).is_empty());
    assert!(!warp.take_outgoing().tags.is_empty());
}

#[test]
fn an_older_nightjars_request_changes_nothing() {
    let (epoch, warp) = state();
    assert_eq!(warp.on_request(&VERSION_1), None);
    assert!(!warp.enabled());
    assert!(warp.first_ignored());
    assert!(!warp.first_ignored());
    warp.on_mouse(5, 5, epoch);
    warp.on_motion(0, 2, [0.0, 90.0, 0.0], epoch);
    warp.tag(1, Some(epoch));
    assert!(warp.take_outgoing().is_empty());
    assert!(lock(&warp.inbox).is_empty());
    // A version 2 client's session goes on through one.
    warp.on_request(&request(true, None, 0));
    let _ = warp.take_outgoing();
    assert_eq!(warp.on_request(&VERSION_1), None);
    assert!(warp.enabled());
    warp.tag(2, Some(epoch + Duration::from_millis(100)));
    let sent = warp.take_outgoing();
    assert!(sent.model.is_none());
    assert_eq!((sent.tags.len(), sent.metas.len()), (1, 1));
}

#[test]
fn only_the_requested_pads_gyro_reaches_the_host_and_every_tag_has_a_meta() {
    let (epoch, warp) = state();
    let gyro = RequestV2 {
        gyro: true,
        gyro_controller: Some(1),
        ..RequestV2::new(FrameWarpRequest {
            enabled: true,
            controller: Some(0),
            mask_max_width: 0,
            lens: true,
        })
    };
    assert_eq!(warp.on_request(&gyro.encode()), Some(true));
    let _ = warp.take_outgoing();
    for ms in 1..=3 {
        let at = epoch + Duration::from_millis(ms);
        warp.on_motion(1, 2, [0.0, 30.0, 0.0], at);
        // Another pad's gyro, and the accelerometer, are not the camera's.
        warp.on_motion(0, 2, [0.0, 30.0, 0.0], at);
        warp.on_motion(1, 1, [0.0, 9.8, 0.0], at);
    }
    assert!(lock(&warp.inbox).is_empty());
    warp.tag(7, Some(epoch + Duration::from_millis(200)));
    let sent = warp.take_outgoing();
    assert_eq!(sent.tags.len(), 1);
    let [meta] = sent.metas.as_slice() else {
        panic!("one meta, not {}", sent.metas.len());
    };
    let meta = FrameWarpV2Meta::decode(meta).unwrap();
    assert_eq!((meta.frame, meta.gyro_seq), (7, 3));
    // A stop ends the filter: nothing more is queued.
    warp.on_request(&request(false, None, 0));
    warp.on_motion(1, 2, [0.0, 30.0, 0.0], epoch);
    assert!(lock(&warp.inbox).is_empty());
}

#[test]
fn the_pointer_is_queued_only_when_it_changes() {
    let (epoch, warp) = state();
    warp.on_cursor(true);
    assert!(lock(&warp.inbox).is_empty());
    warp.on_request(&request(true, None, 0));
    let _ = warp.take_outgoing();
    warp.with(|_| {
        warp.on_cursor(true);
        warp.on_cursor(true);
        assert_eq!(lock(&warp.inbox).len(), 1);
    });
    warp.tag(1, Some(epoch + Duration::from_millis(50)));
    let sent = warp.take_outgoing();
    let meta = FrameWarpV2Meta::decode(&sent.metas[0]).unwrap();
    assert!(meta.has(meta_flags::CURSOR_VISIBLE_PAUSE), "{meta:?}");
    // A request may start the host's state over: the pointer is told again.
    warp.on_request(&request(true, None, 0));
    warp.with(|_| {
        warp.on_cursor(true);
        assert_eq!(lock(&warp.inbox).len(), 1);
    });
}
