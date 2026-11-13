use super::*;
use std::vec::Vec;

fn words() -> Vec<AtomicU64> {
    (0..SECTION_BYTES / 8).map(|_| AtomicU64::new(0)).collect()
}

#[test]
fn formats_round_trip_through_the_wire() {
    for format in [Format::Bgra8, Format::Rgba16F] {
        assert_eq!(Format::from_wire(format.wire()), Some(format));
        assert!(supported_format(format.wire()));
    }
    assert_eq!(Format::from_wire(24), None, "R10G10B10A2 is never pushed");
    assert_eq!(Format::Rgba16F.bytes(), 8);
}

#[test]
fn tokens_pack_into_their_fields() {
    let t = Token {
        generation: 0x00AB_CDEF,
        seq: 0xDEAD_BEEF,
        slot: 3,
    };
    assert_eq!(t.pack(), 0xABCD_EFDE_ADBE_EF03);
    assert_eq!(Token::unpack(t.pack()), t);
    // Generation keeps 24 bits; a wider one cannot leak into the sequence.
    let wide = Token {
        generation: 0x0100_0001,
        ..t
    };
    assert_eq!(Token::unpack(wide.pack()).generation, 1);
    assert_eq!(Token::unpack(wide.pack()).seq, 0xDEAD_BEEF);
}

#[test]
fn the_header_fits_its_page_and_initialises() {
    assert_eq!(HEADER_BYTES, 640);
    let w = words();
    assert!(Header::new(&w[..HEADER_WORDS - 1]).is_none());
    let h = Header::new(&w).unwrap();
    assert_eq!(h.layout(), None, "uninitialised");
    let layout = Layout {
        kind: Kind::Fence,
        slot_count: 4,
        width: 3840,
        height: 2160,
        format: FORMAT_RGBA16F,
        generation: 7,
    };
    w[W_LATEST].store(123, Ordering::Relaxed);
    h.init(layout);
    assert_eq!(h.layout(), Some(layout));
    assert_eq!(h.latest(), None, "init clears a stale publish");
    assert_eq!(
        w[W_MAGIC].load(Ordering::Relaxed).to_le_bytes(),
        *b"RLVDRING"
    );
    w[W_LAYOUT].store(pair(RING_VERSION, 4), Ordering::Relaxed);
    assert_eq!(
        h.layout(),
        Some(Layout {
            kind: Kind::KeyedMutex,
            ..layout
        }),
        "a keyed-mutex ring"
    );
    w[W_LAYOUT].store(pair(3, 4), Ordering::Relaxed);
    assert_eq!(h.layout(), None, "another ring version");
}

#[test]
fn driver_writes_reach_the_host_view() {
    let w = words();
    let driver = Header::new(&w).unwrap();
    let host = Header::new(&w).unwrap();
    host.init(Layout {
        kind: Kind::KeyedMutex,
        slot_count: 3,
        width: 1920,
        height: 1080,
        format: FORMAT_BGRA8,
        generation: 1,
    });
    let meta = SlotMeta {
        seq: 5,
        present_qpc: 1_000_000,
        acquire_qpc: 1_000_100,
        publish_qpc: 1_000_200,
        frame_number: 77,
    };
    driver.write_slot(2, meta);
    driver.publish(Token {
        generation: 1,
        seq: 5,
        slot: 2,
    });
    driver.set_status(STATUS_STREAMING, 0);
    driver.set_surface(1920, 1080, FORMAT_BGRA8);
    driver.count_acquired();
    driver.count_dropped();
    driver.beat(42);
    assert_eq!(host.slot(2), meta);
    assert_eq!(host.slot(1), SlotMeta::default());
    assert_eq!(
        host.latest(),
        Some(Token {
            generation: 1,
            seq: 5,
            slot: 2
        })
    );
    assert_eq!(host.status(), (STATUS_STREAMING, 0));
    assert_eq!(host.surface(), (1920, 1080, FORMAT_BGRA8));
    assert_eq!(host.heartbeat(), 42);
    assert_eq!(
        host.counters(),
        Counters {
            acquired: 1,
            pushed: 1,
            dropped: 1,
            overwritten: 0
        }
    );
    // An out-of-range slot index is clamped, never an out-of-bounds read.
    let _ = host.slot(usize::MAX);
}

/// A model of the textures' keyed mutexes: `None` = held, `Some(key)` = released with key.
struct Mutexes(Vec<Option<u64>>);

impl Mutexes {
    fn new(n: usize) -> Self {
        Self(std::vec![Some(KEY_DRIVER); n])
    }
    fn take(&mut self, slot: usize, key: u64) -> bool {
        if self.0[slot] == Some(key) {
            self.0[slot] = None;
            true
        } else {
            false
        }
    }
    fn release(&mut self, slot: usize, key: u64) {
        assert_eq!(self.0[slot], None, "release of a free mutex");
        self.0[slot] = Some(key);
    }
}

/// The driver's whole publish step against the model.
fn push(picker: &mut Picker, m: &mut Mutexes) -> Option<Claim> {
    let claim = picker.claim(|s, key| m.take(s, key))?;
    let seq = picker.next_seq();
    m.release(claim.slot(), KEY_HOST);
    picker.published(claim.slot(), seq);
    Some(claim)
}

#[test]
fn the_driver_fills_free_slots_before_reclaiming() {
    let mut m = Mutexes::new(3);
    let mut p = Picker::new(3);
    assert_eq!(push(&mut p, &mut m), Some(Claim::Free(0)));
    assert_eq!(push(&mut p, &mut m), Some(Claim::Free(1)));
    assert_eq!(push(&mut p, &mut m), Some(Claim::Free(2)));
    // The host took none: the oldest unconsumed frame (slot 0) goes first.
    assert_eq!(push(&mut p, &mut m), Some(Claim::Reclaimed(0)));
    assert_eq!(push(&mut p, &mut m), Some(Claim::Reclaimed(1)));
    assert_eq!(p.next_seq(), 6);
}

#[test]
fn the_newest_publish_is_the_seed_for_a_replacement_ring() {
    let mut m = Mutexes::new(3);
    let mut p = Picker::new(3);
    assert_eq!(p.newest(), None, "nothing published yet");
    for _ in 0..4 {
        push(&mut p, &mut m);
    }
    // Slots 0..2 carried 1..3, then slot 0 was reclaimed for 4.
    assert_eq!(p.newest(), Some(0));
    push(&mut p, &mut m);
    assert_eq!(p.newest(), Some(1));
}

#[test]
fn a_slot_the_host_returned_is_free_again() {
    let mut m = Mutexes::new(2);
    let mut p = Picker::new(2);
    push(&mut p, &mut m);
    push(&mut p, &mut m);
    // The host consumes slot 1 (newest) and hands it back.
    assert!(m.take(1, KEY_HOST));
    m.release(1, KEY_DRIVER);
    // Slot 1 is free; slot 0 is published but older. Free wins even though slot 0 is
    // older, because a reclaim loses a frame the host could still take.
    assert_eq!(push(&mut p, &mut m), Some(Claim::Free(1)));
}

#[test]
fn the_driver_drops_rather_than_waits_when_the_host_holds_everything() {
    let mut m = Mutexes::new(2);
    let mut p = Picker::new(2);
    push(&mut p, &mut m);
    push(&mut p, &mut m);
    // The host is reading both.
    assert!(m.take(0, KEY_HOST));
    assert!(m.take(1, KEY_HOST));
    assert_eq!(push(&mut p, &mut m), None);
    assert_eq!(p.next_seq(), 3, "a drop consumes no sequence number");
    // One comes back: the driver resumes on it.
    m.release(0, KEY_DRIVER);
    assert_eq!(push(&mut p, &mut m), Some(Claim::Free(0)));
}

#[test]
fn a_never_written_slot_is_not_reclaimed_with_the_host_key() {
    // A texture nobody wrote is released under KEY_DRIVER; trying KEY_HOST on it would
    // fail anyway, but the picker must not even rely on that.
    let p = Picker::new(3);
    let mut tried = Vec::new();
    let claim = p.claim(|s, key| {
        tried.push((s, key));
        false
    });
    assert_eq!(claim, None);
    assert!(tried.iter().all(|&(_, key)| key == KEY_DRIVER));
}

#[test]
fn the_host_takes_only_new_frames_of_its_own_ring() {
    let mut c = Consumer::new(4, 3);
    let token = |generation, seq, slot| {
        Some(Token {
            generation,
            seq,
            slot,
        })
    };
    assert_eq!(c.wanted(None), None);
    assert_eq!(c.wanted(token(4, 1, 2)), Some(2));
    assert_eq!(c.wanted(token(3, 9, 0)), None, "an old ring's publish");
    assert_eq!(c.wanted(token(4, 9, 3)), None, "a slot outside the ring");
    c.consumed(5);
    assert_eq!(c.last(), 5);
    assert_eq!(c.wanted(token(4, 5, 1)), None, "already consumed");
    assert_eq!(c.wanted(token(4, 6, 1)), Some(1));
    // Meta read under the mutex: a reclaim may have made it newer, never older.
    let meta = |seq| SlotMeta {
        seq,
        ..Default::default()
    };
    assert!(c.fresh(&meta(6)));
    assert!(c.fresh(&meta(9)));
    assert!(!c.fresh(&meta(5)));
    assert!(!c.fresh(&meta(0)));
}

#[test]
fn a_reassigned_swap_chain_continues_the_rings_sequence() {
    // The picker belongs to the ring, so a worker started for a replacement swap chain on
    // the same ring publishes after what the host already consumed.
    let mut m = Mutexes::new(4);
    let mut picker = Picker::new(4);
    let mut consumer = Consumer::new(3, 4);
    let host_takes = |m: &mut Mutexes, consumer: &mut Consumer, token: Token| {
        let slot = consumer.wanted(Some(token))?;
        assert!(m.take(slot, KEY_HOST));
        consumer.consumed(u64::from(token.seq));
        m.release(slot, KEY_DRIVER);
        Some(slot)
    };
    let publish = |picker: &mut Picker, m: &mut Mutexes| {
        let claim = push(picker, m).expect("a slot");
        Token {
            generation: 3,
            seq: picker.next_seq() - 1,
            slot: claim.slot() as u8,
        }
    };
    for _ in 0..1000 {
        let token = publish(&mut picker, &mut m);
        assert!(host_takes(&mut m, &mut consumer, token).is_some());
    }
    // The swap chain is reassigned: the new worker reuses the ring's picker.
    let token = publish(&mut picker, &mut m);
    assert_eq!(token.seq, 1001);
    assert!(host_takes(&mut m, &mut consumer, token).is_some());
    // A picker restarted at 1 would publish frames the host rejects as consumed until
    // the sequence caught up: the freeze this ownership prevents.
    let mut restarted = Picker::new(4);
    let token = publish(&mut restarted, &mut m);
    assert_eq!(token.seq, 1);
    assert_eq!(consumer.wanted(Some(token)), None);
}

#[test]
fn end_to_end_the_host_always_sees_the_newest_frame() {
    let w = words();
    let header = Header::new(&w).unwrap();
    let layout = Layout {
        kind: Kind::KeyedMutex,
        slot_count: 3,
        width: 640,
        height: 480,
        format: FORMAT_BGRA8,
        generation: 9,
    };
    header.init(layout);
    let mut m = Mutexes::new(3);
    let mut picker = Picker::new(3);
    let mut consumer = Consumer::new(layout.generation, layout.slot_count);
    let mut seen = Vec::new();
    for frame in 1..=20u64 {
        // Driver: three frames per host poll, so the host must skip.
        let claim = picker.claim(|s, key| m.take(s, key)).expect("a slot");
        let seq = picker.next_seq();
        header.write_slot(
            claim.slot(),
            SlotMeta {
                seq: u64::from(seq),
                present_qpc: frame * 100,
                ..Default::default()
            },
        );
        m.release(claim.slot(), KEY_HOST);
        picker.published(claim.slot(), seq);
        header.publish(Token {
            generation: layout.generation,
            seq,
            slot: claim.slot() as u8,
        });
        if frame % 3 != 0 {
            continue;
        }
        // Host.
        let slot = consumer.wanted(header.latest()).expect("a new frame");
        assert!(m.take(slot, KEY_HOST));
        let meta = header.slot(slot);
        assert!(consumer.fresh(&meta));
        consumer.consumed(meta.seq);
        m.release(slot, KEY_DRIVER);
        seen.push(meta.present_qpc);
    }
    assert_eq!(seen, [300, 600, 900, 1200, 1500, 1800]);
}

#[test]
fn a_fence_sequence_retires_before_its_completed_value_can_wrap() {
    let w = words();
    let header = Header::new(&w).unwrap();
    let mut picker = Picker::new(2);
    picker.next_seq = FENCE_SEQUENCE_LIMIT - 1;
    let slot = picker.claim_fenced(&header, u64::MAX - 1).unwrap().slot();
    assert!(picker.publish_fenced(&header, slot, FENCE_SEQUENCE_LIMIT - 1));
    assert!(picker.needs_renewal());
    assert_eq!(picker.claim_fenced(&header, u64::MAX - 1), None);
    assert_eq!(picker.next_seq(), FENCE_SEQUENCE_LIMIT);
    // A new ring has new GPU fences and can start at one safely.
    assert_eq!(Picker::new(2).next_seq(), 1);
}

#[test]
fn slot_states_round_trip_and_reject_foreign_words() {
    for state in [
        SlotState::Free { release: 0 },
        SlotState::Free {
            release: SlotState::MAX_RELEASE,
        },
        SlotState::Writing { seq: u32::MAX },
        SlotState::Published { seq: 1 },
        SlotState::Reading { seq: 7 },
    ] {
        assert_eq!(SlotState::unpack(state.pack()), state);
    }
    assert_eq!(
        SlotState::unpack(0),
        SlotState::Free { release: 0 },
        "zeroed"
    );
    assert!(matches!(SlotState::unpack(9), SlotState::Invalid(9)));
    let wide = 1 | (1u64 << 40);
    assert_eq!(
        SlotState::unpack(wide),
        SlotState::Invalid(wide),
        "seq past 32 bits"
    );
    // A slot holding a word nobody writes is never taken.
    let w = words();
    let header = Header::new(&w).unwrap();
    w[W_SLOTS + S_STATE].store(9, Ordering::Relaxed);
    let picker = Picker::new(2);
    assert_eq!(picker.claim_fenced(&header, 0), Some(Claim::Free(1)));
}

#[test]
fn the_driver_skips_slots_whose_reads_the_released_fence_does_not_cover() {
    let w = words();
    let header = Header::new(&w).unwrap();
    let mut picker = Picker::new(2);
    let consumer = Consumer::new(0, 2);
    // Frame 1 into slot 0, frame 2 into slot 1; the host claims and frees frame 2 with
    // release value 5, not yet reached on the GPU.
    for slot in 0..2 {
        let claim = picker.claim_fenced(&header, 0).unwrap();
        assert_eq!(claim, Claim::Free(slot));
        let seq = picker.next_seq();
        assert!(picker.publish_fenced(&header, slot, seq));
        header.publish(Token {
            generation: 0,
            seq,
            slot: slot as u8,
        });
    }
    assert_eq!(consumer.claim(&header), Some((1, 2)));
    assert!(Consumer::release(&header, 1, 2, 5));
    assert_eq!(header.slot_state(1), SlotState::Free { release: 5 });
    // Slot 1 is busy until the fence reaches 5, so the driver overwrites frame 1 instead.
    assert_eq!(picker.claim_fenced(&header, 4), Some(Claim::Reclaimed(0)));
    picker.abandon_fenced(&header, 0, 3);
    assert_eq!(header.slot_state(0), SlotState::Free { release: 0 });
    assert_eq!(picker.newest(), Some(1), "an abandoned slot holds no frame");
    assert_eq!(picker.claim_fenced(&header, 5), Some(Claim::Free(0)));
}

#[test]
fn the_host_takes_only_the_newest_publish_and_loses_it_to_a_reclaim() {
    let w = words();
    let header = Header::new(&w).unwrap();
    let mut picker = Picker::new(2);
    let mut consumer = Consumer::new(3, 2);
    let publish = |picker: &mut Picker| {
        let slot = picker.claim_fenced(&header, 0).unwrap().slot();
        let seq = picker.next_seq();
        assert!(picker.publish_fenced(&header, slot, seq));
        header.publish(Token {
            generation: 3,
            seq,
            slot: slot as u8,
        });
        (slot, seq)
    };
    publish(&mut picker);
    let (newest, seq) = publish(&mut picker);
    // The host looks at the newest publish (slot 1, frame 2); before it swaps, the driver
    // runs out of free slots and reclaims the oldest unclaimed one, slot 0.
    let wanted = consumer.wanted(header.latest()).unwrap();
    assert_eq!((wanted, seq), (1, 2));
    assert_eq!(picker.claim_fenced(&header, 0), Some(Claim::Reclaimed(0)));
    // Next it would reclaim slot 1, the host's choice; the swap stops the host reading it.
    let third = picker.next_seq();
    assert!(picker.publish_fenced(&header, 0, third));
    assert_eq!(
        picker.claim_fenced(&header, 0),
        Some(Claim::Reclaimed(newest))
    );
    assert_eq!(consumer.claim_slot(&header, wanted), None, "being written");
    let fourth = picker.next_seq();
    assert!(picker.publish_fenced(&header, newest, fourth));
    header.publish(Token {
        generation: 3,
        seq: fourth,
        slot: newest as u8,
    });
    // Frame 3 in slot 0 is unconsumed but not the newest publish: the host takes 4.
    assert_eq!(consumer.claim(&header), Some((newest, fourth)));
    consumer.consumed(u64::from(fourth));
    assert_eq!(consumer.claim(&header), None, "already claimed");
    assert!(
        !Consumer::release(&header, newest, fourth + 1, 1),
        "not that frame"
    );
    assert!(Consumer::release(&header, newest, fourth, 1));
    assert_eq!(header.slot_state(0), SlotState::Published { seq: third });
}

/// One state of the exhaustive model below: both processes' views plus the two GPU
/// queues, each running in order.
#[derive(Clone, PartialEq, Eq, Hash)]
struct World {
    header: Vec<u64>,
    picker: Picker,
    consumer: Consumer,
    frames_left: u8,
    /// The driver took this slot for this frame and has not published it.
    writing: Option<(usize, u32)>,
    /// The driver's queue: each copy and the written-fence signal after it.
    copies: Vec<(usize, u32)>,
    written: u32,
    /// The slot the host saw in `W_LATEST` and is about to swap.
    wanted: Option<usize>,
    /// Claimed and not yet freed: `(slot, seq)`.
    holds: Vec<(usize, u32)>,
    /// The host's queue: `Ok((slot, seq))` waits for the written fence to reach `seq`,
    /// then reads the slot; `Err((slot, value))` signals the released fence after the
    /// slot's last read.
    reads: Vec<Result<(usize, u32), (usize, u64)>>,
    /// Slots the host's GPU is reading: past the wait, before the released signal.
    gpu_reads: Vec<usize>,
    released: u64,
    next_release: u64,
}

/// The host holds at most the newest frame and the one being encoded.
const MAX_HOLDS: usize = 2;

impl World {
    fn new(slots: usize, frames: u8) -> Self {
        Self {
            header: std::vec![0; HEADER_WORDS],
            picker: Picker::new(slots),
            consumer: Consumer::new(1, slots as u32),
            frames_left: frames,
            writing: None,
            copies: Vec::new(),
            written: 0,
            wanted: None,
            holds: Vec::new(),
            reads: Vec::new(),
            gpu_reads: Vec::new(),
            released: 0,
            next_release: 1,
        }
    }

    /// Run `f` on the header these words hold, then store them back.
    fn with_header<T>(&mut self, f: impl FnOnce(&Header<'_>, &mut Self) -> T) -> T {
        let atomics: Vec<AtomicU64> = self.header.iter().map(|&v| AtomicU64::new(v)).collect();
        let out = f(&Header::new(&atomics).unwrap(), self);
        self.header = atomics.iter().map(|a| a.load(Ordering::Relaxed)).collect();
        out
    }

    fn state(&mut self, slot: usize) -> SlotState {
        self.with_header(|h, _| h.slot_state(slot))
    }

    /// Every state one step from this one.
    fn steps(&self, slots: usize) -> Vec<Self> {
        let mut next = Vec::new();
        // Driver: take a slot for a new frame, or drop it.
        if self.writing.is_none() && self.frames_left > 0 {
            let mut w = self.clone();
            w.frames_left -= 1;
            let released = w.released;
            let idle = w.holds.is_empty() && w.reads.iter().all(Result::is_ok);
            match w.with_header(|h, w| w.picker.claim_fenced(h, released)) {
                Some(claim) => {
                    let seq = w.picker.next_seq();
                    assert_eq!(w.state(claim.slot()), SlotState::Writing { seq });
                    w.writing = Some((claim.slot(), seq));
                    w.copies.push((claim.slot(), seq));
                }
                // Dropping is the driver's only answer to a busy ring, and it happens
                // only while the host holds or still reads slots.
                None => assert!(!idle, "a ring the host left alone has no free slot"),
            }
            next.push(w);
        }
        // Driver: publish what it wrote, or give it up (a failed GPU signal).
        if let Some((slot, seq)) = self.writing {
            let mut w = self.clone();
            w.writing = None;
            w.with_header(|h, w| {
                h.write_slot(
                    slot,
                    SlotMeta {
                        seq: u64::from(seq),
                        ..Default::default()
                    },
                );
                assert!(
                    w.picker.publish_fenced(h, slot, seq),
                    "the driver's own slot"
                );
                h.publish(Token {
                    generation: 1,
                    seq,
                    slot: slot as u8,
                });
            });
            next.push(w);
            let mut w = self.clone();
            w.writing = None;
            w.with_header(|h, w| w.picker.abandon_fenced(h, slot, seq));
            next.push(w);
        }
        // Host: look at the newest publish.
        if self.wanted.is_none() && self.holds.len() < MAX_HOLDS {
            let mut w = self.clone();
            if let Some(slot) = w.with_header(|h, w| w.consumer.wanted(h.latest())) {
                w.wanted = Some(slot);
                next.push(w);
            }
        }
        // Host: swap the slot it looked at to Reading.
        if let Some(slot) = self.wanted {
            let mut w = self.clone();
            w.wanted = None;
            let before = w.consumer.last();
            if let Some((s, seq)) = w.with_header(|h, w| w.consumer.claim_slot(h, slot)) {
                assert_eq!(s, slot, "only the slot the newest publish named");
                assert!(seq > before, "only a frame not consumed yet");
                let meta = w.with_header(|h, _| h.slot(slot));
                assert_eq!(meta.seq, u64::from(seq), "the meta of the claimed frame");
                w.consumer.consumed(u64::from(seq));
                w.holds.push((slot, seq));
                w.reads.push(Ok((slot, seq)));
            }
            next.push(w);
        }
        // Host: free any claimed slot after its last read.
        for i in 0..self.holds.len() {
            let mut w = self.clone();
            let (slot, seq) = w.holds.remove(i);
            let release = w.next_release;
            w.next_release += 1;
            w.reads.push(Err((slot, release)));
            assert!(w.with_header(|h, _| Consumer::release(h, slot, seq, release)));
            next.push(w);
        }
        // GPU: the driver's next copy and its written signal finish.
        if !self.copies.is_empty() {
            let mut w = self.clone();
            let (_, seq) = w.copies.remove(0);
            w.written = seq;
            next.push(w);
        }
        // GPU: the host's queue moves past a satisfied wait or a signal.
        match self.reads.first() {
            Some(&Ok((slot, seq))) if self.written >= seq => {
                let mut w = self.clone();
                let _ = w.reads.remove(0);
                w.gpu_reads.push(slot);
                next.push(w);
            }
            Some(&Err((slot, value))) => {
                let mut w = self.clone();
                let _ = w.reads.remove(0);
                w.released = value;
                let read = w.gpu_reads.iter().position(|&s| s == slot).unwrap();
                w.gpu_reads.remove(read);
                next.push(w);
            }
            _ => {}
        }
        for w in &mut next {
            w.check(slots);
        }
        next
    }

    /// The invariants, after every step.
    fn check(&mut self, slots: usize) {
        for slot in 0..slots {
            let state = self.state(slot);
            // The driver's copy is in flight from its pick until its written signal ran;
            // a host read from the end of its wait until its released signal ran.
            let written_on_gpu = self.copies.iter().any(|&(s, _)| s == slot);
            let read_on_gpu = self.gpu_reads.contains(&slot);
            assert!(
                !(written_on_gpu && read_on_gpu),
                "slot {slot} written and read at once ({state:?})"
            );
            if let Some(&(_, seq)) = self.holds.iter().find(|&&(s, _)| s == slot) {
                assert_eq!(state, SlotState::Reading { seq });
            }
            if let Some((s, seq)) = self.writing
                && s == slot
            {
                assert_eq!(state, SlotState::Writing { seq });
            }
            assert!(!matches!(state, SlotState::Invalid(_)));
        }
    }
}

/// Explore every reachable state; returns how many there were.
fn explore(slots: usize, frames: u8) -> usize {
    let mut seen = std::collections::HashSet::new();
    let mut todo = std::vec![World::new(slots, frames)];
    let mut finished = 0;
    while let Some(world) = todo.pop() {
        if !seen.insert(world.clone()) {
            continue;
        }
        let next = world.steps(slots);
        if next.is_empty() {
            // Nothing left to do: every frame was published or dropped and both GPU
            // queues drained, so no wait is left hanging.
            assert!(world.frames_left == 0 && world.writing.is_none());
            assert!(world.copies.is_empty() && world.reads.is_empty());
            finished += 1;
        }
        todo.extend(next);
    }
    assert!(finished > 0);
    seen.len()
}

#[test]
fn every_interleaving_of_driver_host_and_gpus_keeps_slots_apart() {
    // Every order of the driver's pick, reclaim, publish and abandon against the host's
    // look, claim and free, with both GPU queues finishing at any point; the invariants
    // are checked after every step (`World::check`), at every pick and every claim.
    for (slots, frames) in [(2, 7), (3, 6), (4, 5)] {
        let states = explore(slots, frames);
        assert!(
            states > 1000,
            "{slots} slots: only {states} states explored"
        );
    }
}
