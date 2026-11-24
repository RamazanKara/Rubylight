//! Unit tests for the in-place VA-API sampling state machine (`Sampled`), copy-slot `Held`
//! tracking, single-layer 4:4:4 DMA-BUF import validation, and route eligibility policy.

use std::cell::RefCell;
use std::rc::Rc;

use super::*;

/// A picture token that records its id when dropped.
#[derive(Debug)]
struct Tracked(u32, Rc<RefCell<Vec<u32>>>);
impl Drop for Tracked {
    fn drop(&mut self) {
        self.1.borrow_mut().push(self.0);
    }
}

/// Records the queue's calls; work is pending on a slot from its release until `complete`.
#[derive(Default)]
struct Mock {
    log: Vec<String>,
    pending: Vec<usize>,
    refuse_release: bool,
    combine_swap: bool,
}

impl Mock {
    fn complete(&mut self, slot: usize) {
        self.pending.retain(|&pending| pending != slot);
    }
}

impl SampleGpu for Mock {
    fn flush(&mut self) -> Result<(), String> {
        self.log.push("flush".into());
        Ok(())
    }
    fn wait(&mut self, slot: usize) -> Result<(), String> {
        self.log.push(format!("wait {slot}"));
        self.complete(slot);
        Ok(())
    }
    fn finished(&self, slot: usize) -> bool {
        !self.pending.contains(&slot)
    }
    fn acquire(&mut self, slot: usize) -> Result<(), String> {
        self.log.push(format!("acquire {slot}"));
        Ok(())
    }
    fn release(&mut self, slot: usize) -> Result<(), String> {
        if self.refuse_release {
            return Err("the device is lost".into());
        }
        self.log.push(format!("release {slot}"));
        self.pending.push(slot);
        Ok(())
    }
    fn swap(&mut self, acquire_slot: usize, release_slot: usize) -> Result<(), String> {
        if !self.combine_swap {
            self.acquire(acquire_slot)?;
            return self.release(release_slot);
        }
        if self.refuse_release {
            return Err("the device is lost".into());
        }
        self.log.push(format!("swap {acquire_slot} {release_slot}"));
        self.pending.push(release_slot);
        Ok(())
    }
}

#[test]
fn a_picture_is_released_only_after_the_next_draw_is_flushed() {
    let mut gpu = Mock::default();
    let mut sampled = Sampled::new(3);
    sampled.show(&mut gpu, 0u32).unwrap();
    assert_eq!(gpu.log, ["flush", "wait 0", "acquire 0"], "nothing is released with one picture");
    sampled.show(&mut gpu, 1).unwrap();
    assert_eq!(gpu.log[3..], ["flush", "wait 1", "acquire 1", "release 0"], "picture 0's release follows the flush that submits its draws");
    assert_eq!(sampled.held(), 2);
}

#[test]
fn replacing_a_shown_picture_uses_combined_swap() {
    let mut gpu = Mock { combine_swap: true, ..Mock::default() };
    let mut sampled = Sampled::new(3);
    sampled.show(&mut gpu, 0u32).unwrap();
    assert_eq!(gpu.log, ["flush", "wait 0", "acquire 0"]);
    sampled.show(&mut gpu, 1).unwrap();
    assert_eq!(gpu.log[3..], ["flush", "wait 1", "swap 1 0"]);
    assert_eq!(sampled.held(), 2);
}

#[test]
fn a_released_picture_waits_for_its_fence_before_the_decoder_gets_it_back() {
    let dropped = Rc::new(RefCell::new(Vec::new()));
    let mut gpu = Mock::default();
    let mut sampled = Sampled::new(3);
    sampled.show(&mut gpu, Tracked(0, dropped.clone())).unwrap();
    sampled.show(&mut gpu, Tracked(1, dropped.clone())).unwrap();
    // The GPU has not finished picture 0's release: it is not handed back.
    sampled.show(&mut gpu, Tracked(2, dropped.clone())).unwrap();
    assert!(dropped.borrow().is_empty());
    assert_eq!(sampled.held(), 3, "shown picture plus two not yet returned");
    gpu.complete(0);
    // Slot 0's next use waits for it, and then picture 0 is dropped in place.
    sampled.show(&mut gpu, Tracked(3, dropped.clone())).unwrap();
    assert_eq!(*dropped.borrow(), vec![0]);
    assert_eq!(sampled.held(), 3);
}

#[test]
fn held_pictures_stay_within_the_slots() {
    let mut gpu = Mock::default();
    let mut sampled = Sampled::new(3);
    for picture in 0..10u32 {
        sampled.show(&mut gpu, picture).unwrap();
        assert!(sampled.held() <= 3, "held {} after picture {picture}", sampled.held());
    }
}

#[test]
fn a_picture_that_cannot_be_released_comes_back_with_the_error() {
    let mut gpu = Mock::default();
    let mut sampled = Sampled::new(3);
    sampled.show(&mut gpu, 0u32).unwrap();
    gpu.refuse_release = true;
    let (picture, error) = sampled.show(&mut gpu, 1).unwrap_err();
    assert_eq!(picture, 1, "the new picture is handed back unshown");
    assert!(error.contains("lost"));
}

#[test]
fn a_flush_failure_hands_the_picture_back_unshown() {
    struct Refusing;
    impl SampleGpu for Refusing {
        fn flush(&mut self) -> Result<(), String> { Err("no flush".into()) }
        fn wait(&mut self, _: usize) -> Result<(), String> { Ok(()) }
        fn finished(&self, _: usize) -> bool { true }
        fn acquire(&mut self, _: usize) -> Result<(), String> { Ok(()) }
        fn release(&mut self, _: usize) -> Result<(), String> { Ok(()) }
    }
    let mut sampled = Sampled::new(3);
    let (picture, error) = sampled.show(&mut Refusing, 7u32).unwrap_err();
    assert_eq!((picture, error.as_str()), (7, "no flush"));
    assert_eq!(sampled.held(), 0);
}

#[test]
fn finishing_gives_every_picture_back_after_its_release() {
    let dropped = Rc::new(RefCell::new(Vec::new()));
    let mut gpu = Mock::default();
    let mut sampled = Sampled::new(3);
    sampled.show(&mut gpu, Tracked(0, dropped.clone())).unwrap();
    sampled.show(&mut gpu, Tracked(1, dropped.clone())).unwrap();
    sampled.finish(&mut gpu).unwrap();
    assert_eq!(*dropped.borrow(), vec![0, 1]);
    assert_eq!(gpu.log.last().map(String::as_str), Some("wait 1"));
    assert_eq!(sampled.held(), 0);
    assert!(gpu.log.contains(&"release 1".to_string()), "the shown picture is released, not dropped");
}

#[test]
fn held_drop_finished_drops_only_done_slots_in_place() {
    let dropped = Rc::new(RefCell::new(Vec::new()));
    let mut held = Held::new();
    held.hold(0, Tracked(10, dropped.clone()));
    held.hold(1, Tracked(11, dropped.clone()));
    held.hold(2, Tracked(12, dropped.clone()));
    held.drop_finished(|slot| slot != 1);
    let mut got = dropped.borrow().clone();
    got.sort_unstable();
    assert_eq!(got, vec![10, 12]);
    assert!(held.take(0).is_none());
    assert!(held.take(1).is_some());
}

#[test]
fn interleaved_444_single_layer_imports_accept_ayuv_xyuv_y410_xv30() {
    let make = |drm_format: u32| {
        let mut desc = crate::dmabuf::Descriptor { num_objects: 1, num_layers: 1, ..Default::default() };
        desc.layers[0].drm_format = drm_format;
        desc.layers[0].num_planes = 1;
        desc.layers[0].object_index[0] = 0;
        desc
    };
    assert!(import_interleaved_layer(&make(fourcc(b"AYUV")), false).unwrap() == vk::Format::R8G8B8A8_UNORM);
    assert!(import_interleaved_layer(&make(fourcc(b"XYUV")), false).unwrap() == vk::Format::R8G8B8A8_UNORM);
    assert!(import_interleaved_layer(&make(fourcc(b"Y410")), true).unwrap() == vk::Format::A2R10G10B10_UNORM_PACK32);
    assert!(import_interleaved_layer(&make(fourcc(b"XV30")), true).unwrap() == vk::Format::A2R10G10B10_UNORM_PACK32);
    assert!(import_interleaved_layer(&make(fourcc(b"XYUV")), true).is_err());
    assert!(import_interleaved_layer(&make(fourcc(b"XV30")), false).is_err());
}

#[test]
fn sampling_is_on_unless_the_environment_says_0() {
    assert!(sample_switched_on(None));
    assert!(sample_switched_on(Some("1".as_ref())));
    assert!(!sample_switched_on(Some("0".as_ref())));
    assert_eq!(SAMPLED_HELD, MAPPED_HELD + 1);
}

#[test]
fn the_first_reason_against_sampling_is_the_one_given() {
    let all = Eligibility { switched_on: true, sampleable: true, spare_surface: true, stable_copy: false };
    assert_eq!(route(all), Route::Sample);
    assert_eq!(route(Eligibility { switched_on: false, ..all }), Route::Copy("NIGHTJAR_VIDEO_SAMPLE_PICTURES=0"));
    assert!(matches!(route(Eligibility { stable_copy: true, sampleable: false, ..all }), Route::Copy(why) if why.contains("copy that outlives")));
    assert!(matches!(route(Eligibility { sampleable: false, spare_surface: false, ..all }), Route::Copy(why) if why.contains("DMA-BUF")));
    assert!(matches!(route(Eligibility { spare_surface: false, ..all }), Route::Copy(why) if why.contains("no surface")));
}

#[test]
fn a_pool_that_maps_keeps_a_surface_more_when_pictures_are_sampled() {
    let held = mapped_held();
    assert_eq!(held, if sample_switched_on(std::env::var_os(SAMPLE_ENVIRONMENT).as_deref()) { SAMPLED_HELD } else { MAPPED_HELD });
}

#[test]
fn the_log_says_which_path_is_taken() {
    assert!(describe_route(Route::Sample).contains("sampled in place"));
    assert!(describe_route(Route::Copy("why")).ends_with("why"));
}
