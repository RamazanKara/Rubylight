use super::*;

#[test]
fn sets_are_handed_out_once_and_come_back_in_order() {
    let mut pool = Pool::new(3);
    let taken: Vec<_> = std::iter::from_fn(|| pool.acquire()).collect();
    assert_eq!(taken, [(0, 0), (1, 0), (2, 0)]);
    assert_eq!(pool.acquire(), None, "an empty pool hands out nothing");
    pool.release(1, None);
    pool.release(0, Some(7));
    assert_eq!(pool.free_count(), 2);
    assert_eq!(pool.acquire(), Some((1, 0)));
    assert_eq!(pool.acquire(), Some((0, 7)), "the set a draw read waits for the draw's signal");
}

#[test]
fn a_set_waits_for_the_largest_value_it_was_released_with() {
    let mut pool = Pool::new(1);
    for value in [Some(5), None, Some(3)] {
        let (set, _) = pool.acquire().unwrap();
        pool.release(set, value);
    }
    assert_eq!(pool.acquire(), Some((0, 5)), "an unshown picture or an older value never lowers the wait");
}

#[test]
fn a_pinned_set_is_not_reused_until_its_readers_let_go() {
    let mut pool = Pool::new(1);
    let (set, _) = pool.acquire().unwrap();
    pool.pin(set);
    pool.pin(set);
    pool.release(set, Some(4));
    assert_eq!(pool.acquire(), None, "released while read: not free yet");
    pool.unpin(set);
    assert_eq!(pool.acquire(), None, "one reader left");
    pool.unpin(set);
    assert_eq!(pool.acquire(), Some((0, 4)), "free when the last reader lets go, with the wait it was released with");
}

#[test]
fn a_reader_that_lets_go_of_a_held_set_does_not_free_it() {
    let mut pool = Pool::new(1);
    let (set, _) = pool.acquire().unwrap();
    pool.pin(set);
    pool.unpin(set);
    assert_eq!(pool.acquire(), None, "the lease still holds it");
    pool.release(set, None);
    assert_eq!(pool.acquire(), Some((0, 0)));
}

#[test]
fn a_slice_draw_raises_the_wait_of_the_next_decode_into_the_set() {
    let mut pool = Pool::new(1);
    let (set, _) = pool.acquire().unwrap();
    pool.read_until(set, 3);
    pool.read_until(set, 2);
    assert_eq!(pool.after(set), 3, "never lowered");
    pool.release(set, Some(1));
    assert_eq!(pool.acquire(), Some((0, 3)));
}

#[cfg(unix)]
#[test]
fn a_lease_holds_its_buffers_until_it_is_dropped_or_released() {
    let planes = unix_planes(2);
    let (first, _) = planes.acquire().unwrap();
    let (second, _) = planes.acquire().unwrap();
    assert!(planes.acquire().is_none(), "every set is held by a picture");
    first.with_buffers(|buffers| buffers[0][0] = 7).unwrap();
    assert_eq!(first.with_buffers(|buffers| buffers[0][0]), Some(7));
    assert_eq!([0, 1, 2].map(|p| first.with_buffers(|buffers| buffers[p].len())), [Some(64 * 48), Some(32 * 24), Some(32 * 24)]);
    assert!(planes.images().is_none(), "CPU buffers are not GPU images");
    let (set, _) = (first.set(), first.release());
    first.release();
    assert_eq!(planes.free_count(), 1, "releasing twice frees the set once");
    drop(second);
    assert_eq!(planes.free_count(), 2);
    drop(first);
    assert_eq!(planes.free_count(), 2, "a released lease does not free again on drop");
    let (again, _) = planes.acquire().unwrap();
    assert_eq!(again.set(), set, "sets are reused in the order they were freed");
}

/// A set the presenter copied out of on the GPU comes back with its reuse signal, which the
/// next decode into it waits for; a picture dropped unshown adds no wait.
#[cfg(unix)]
#[test]
fn a_set_released_after_a_copy_hands_its_reuse_value_to_the_next_decode() {
    let planes = unix_planes(1);
    let (lease, after) = planes.acquire().unwrap();
    assert_eq!(after, 0);
    lease.release_after(5);
    lease.release_after(9);
    drop(lease);
    let (lease, after) = planes.acquire().unwrap();
    assert_eq!(after, 5, "released once, with the first value");
    drop(lease);
    assert_eq!(planes.acquire().map(|(_, after)| after), Some(5), "an unshown picture does not lower the wait");
}

/// The GPU side of a `Handover`, as a log of what was queued, in order.
#[derive(Default)]
struct Gpu {
    log: Vec<String>,
    value: u64,
    refuse_sampled: bool,
    refuse_returned: bool,
}

impl SetGpu for Gpu {
    fn flush(&mut self) -> Result<(), String> {
        self.log.push("flush".into());
        Ok(())
    }

    fn sampled(&mut self, set: usize) -> Result<(), String> {
        if self.refuse_sampled { return Err("device lost".into()); }
        self.log.push(format!("sampled {set}"));
        Ok(())
    }

    fn returned(&mut self, set: usize) -> Result<u64, String> {
        if self.refuse_returned { return Err("device lost".into()); }
        self.value += 1;
        self.log.push(format!("returned {set} at {}", self.value));
        Ok(self.value)
    }
}

/// A set the presenter holds, over a pool shared with the "decoder".
struct Held {
    set: usize,
    pool: Arc<Mutex<Pool>>,
    freed: bool,
}

impl HeldSet for Held {
    fn set(&self) -> usize { self.set }

    fn give_back(mut self, after: u64) {
        self.freed = true;
        self.pool.lock().unwrap().release(self.set, Some(after));
    }

    fn abandon(mut self) {
        self.freed = true;
    }
}

impl Drop for Held {
    /// A lease dropped unread goes back without a wait, as `Lease`'s drop does.
    fn drop(&mut self) {
        if !self.freed { self.pool.lock().unwrap().release(self.set, None); }
    }
}

fn take(pool: &Arc<Mutex<Pool>>) -> Held {
    let (set, _) = pool.lock().unwrap().acquire().expect("a free set");
    Held { set, pool: pool.clone(), freed: false }
}

/// The set a picture is drawn from stays out of the pool while it is shown, and goes back
/// only with a value signalled after the renderer flushed the draws that read it.
#[test]
fn a_sampled_set_goes_back_only_behind_the_draws_that_read_it() {
    let pool = Arc::new(Mutex::new(Pool::new(3)));
    let (mut gpu, mut handover) = (Gpu::default(), Handover::default());
    handover.show(&mut gpu, take(&pool)).unwrap();
    assert_eq!(handover.held(), Some(0));
    assert_eq!(gpu.log, ["sampled 0"], "nothing is given back while the first picture is shown");
    assert_eq!(pool.lock().unwrap().free_count(), 2, "the shown set is not free");
    handover.show(&mut gpu, take(&pool)).unwrap();
    assert_eq!(gpu.log, ["sampled 0", "sampled 1", "flush", "returned 0 at 1"], "the old set's return is queued after the flush");
    assert_eq!(handover.held(), Some(1));
    let mut pool_now = pool.lock().unwrap();
    assert_eq!(pool_now.free_count(), 2);
    assert_eq!(pool_now.acquire(), Some((2, 0)), "the set never read comes first, without a wait");
    assert_eq!(pool_now.acquire(), Some((0, 1)), "the replaced set: its next decode waits for the value behind its last draw");
}

/// Pictures that are replaced before they are drawn (skipped), dropped before they reach the
/// presenter, and the one shown when the stream ends all come back; none is lost.
#[test]
fn replaced_dropped_and_last_sets_all_come_back() {
    let pool = Arc::new(Mutex::new(Pool::new(4)));
    let (mut gpu, mut handover) = (Gpu::default(), Handover::default());
    for _ in 0..10 {
        let dropped = take(&pool);
        handover.show(&mut gpu, take(&pool)).unwrap();
        drop(dropped);
    }
    assert_eq!(pool.lock().unwrap().free_count(), 3, "only the shown set is held");
    handover.finish(&mut gpu);
    handover.finish(&mut gpu);
    assert_eq!(handover.held(), None);
    assert_eq!(pool.lock().unwrap().free_count(), 4, "the stream's end gives the last set back, once");
    assert_eq!(gpu.log.iter().filter(|line| line.starts_with("returned")).count(), 10);
    assert_eq!(gpu.log.last().map(String::as_str), Some("returned 3 at 10"));
    assert_eq!(gpu.log[gpu.log.len() - 2], "flush", "the last return follows a flush too");
}

/// A picture whose set cannot be made readable is not shown: it goes back unread, and the
/// held set (the last picture, still on screen) stays.
#[test]
fn a_set_that_cannot_be_sampled_goes_back_unread_and_the_shown_one_stays() {
    let pool = Arc::new(Mutex::new(Pool::new(2)));
    let (mut gpu, mut handover) = (Gpu::default(), Handover::default());
    handover.show(&mut gpu, take(&pool)).unwrap();
    gpu.refuse_sampled = true;
    assert!(handover.show(&mut gpu, take(&pool)).is_err());
    assert_eq!(handover.held(), Some(0));
    assert_eq!(pool.lock().unwrap().acquire(), Some((1, 0)), "the refused set is free again, with no wait");
}

/// A set whose return cannot be queued (a lost device) never goes back: the decoder must not
/// write a set still in the presenter's layout, with draws of it possibly pending.
#[test]
fn a_set_whose_return_fails_stays_out_of_use() {
    let pool = Arc::new(Mutex::new(Pool::new(2)));
    let (mut gpu, mut handover) = (Gpu::default(), Handover::default());
    handover.show(&mut gpu, take(&pool)).unwrap();
    gpu.refuse_returned = true;
    handover.show(&mut gpu, take(&pool)).unwrap();
    handover.finish(&mut gpu);
    assert_eq!(pool.lock().unwrap().free_count(), 0, "both sets are abandoned, neither is freed unsynchronised");
}

/// The presenter takes a picture's set over: the picture's lease no longer frees it, and the
/// presenter's frees it once.
#[cfg(unix)]
#[test]
fn a_handed_over_lease_frees_its_set_once_through_the_new_owner() {
    let planes = unix_planes(2);
    let (picture, _) = planes.acquire().unwrap();
    let held = picture.hand_over().expect("not given back yet");
    assert!(picture.hand_over().is_none(), "handed over once");
    drop(picture);
    assert_eq!(planes.free_count(), 1, "the picture's lease no longer holds it, the presenter's does");
    HeldSet::give_back(held, 4);
    assert_eq!(planes.free_count(), 2);
    let (set, _) = planes.acquire().unwrap();
    let (second, after) = planes.acquire().unwrap();
    assert_eq!((set.set(), second.set(), after), (1, 0, 4), "the given-back set waits for the presenter's value");
    set.hand_over().unwrap().abandon();
    drop(set);
    assert_eq!(planes.free_count(), 0, "an abandoned set never comes back");
    drop(second);
}

#[cfg(unix)]
fn unix_planes(sets: usize) -> Arc<Planes> {
    let config = GpuConfig { width: 64, height: 48, chroma444: false, ten_bit: false, slots: sets };
    Arc::new(Planes {
        config,
        store: Store::Buffers((0..sets).map(|_| Mutex::new([0, 1, 2].map(|plane| vec![0; config.plane_bytes(plane)]))).collect()),
        pool: Mutex::new(Pool::new(sets)),
    })
}
