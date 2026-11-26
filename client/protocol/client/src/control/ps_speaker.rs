//! A PlayStation controller's speaker from a host that sends Nonary's speaker PCM (0x5602), as
//! Vibepollo does, rather than Rubylight's controller speaker (0x5533).
//!
//! The client asks for it with `x-ml-general.featureFlags` 0x08 ([`offer`]) and binds each
//! PlayStation controller whose speaker it can play with a 20-byte 0x55000009 packet right
//! before the arrival, which carries capability 0x4000 (the input queue does both). Blocks echo
//! the binding's owner; [`Receiver`] keeps those of the controller that is there now, in order,
//! and hands them on as the same [`Pcm`] blocks 0x5533 brings, so the apps play both alike.
//!
//! The binding shares its magic with Foundation Sunshine's touchpad packet, which a host
//! offering that packet would read it as, and 0x5533 already carries the speaker from a
//! Rubylight host: [`offer`] never asks a host that offers either.

use std::sync::atomic::{AtomicU64, Ordering};

use rubylight_protocol::ps_speaker_pcm::{FLAG_DISCONTINUITY, SESSION_FEATURE, SpeakerBlock};

use super::haptics::Pcm;
use super::negotiation::feature;

/// Rubylight's controller speaker, whose host never gets bindings.
const RUBYLIGHT_SPEAKER: u16 = 0x5533;

/// Whether the ANNOUNCE asks for speaker PCM: the app plays a controller speaker (`requested`),
/// the host is a Sunshine-family one on encrypted control, and it offers neither Rubylight's
/// controller speaker in `a=x-rl-control` nor Foundation Sunshine's touchpad packets in
/// `x-ss-general.featureFlags`.
pub fn offer(
    requested: bool,
    sunshine: bool,
    control_v2: bool,
    host_control_messages: &[u16],
    host_feature_flags: u32,
) -> bool {
    requested
        && sunshine
        && control_v2
        && !host_control_messages.contains(&RUBYLIGHT_SPEAKER)
        && host_feature_flags & (feature::TOUCHPAD_EVENTS | feature::TOUCHPAD_FRAME_EVENTS) == 0
}

/// The [`offer`] bit, for the ANNOUNCE.
pub const FEATURE: u32 = SESSION_FEATURE;

/// The next binding owner: nonzero and never repeated within the process, so a block for a
/// controller of an earlier binding or stream never plays on a later one. `None` once spent.
pub fn next_owner() -> Option<u64> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let seed = || {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |since| since.as_nanos() as u64);
        // Start low in the range so it never runs out, but differently per process.
        (((u64::from(std::process::id()) << 32) ^ nanos) >> 2) | 1
    };
    let _ = NEXT.compare_exchange(0, seed(), Ordering::AcqRel, Ordering::Acquire);
    NEXT.fetch_update(Ordering::AcqRel, Ordering::Acquire, |owner| {
        owner.checked_add(1)
    })
    .ok()
    .filter(|owner| *owner != 0)
}

/// Each slot's bound owner, 0 for none: the input queue writes it when it sends a binding and
/// [`Receiver`] reads it.
#[derive(Debug, Default)]
pub struct Owners([AtomicU64; 16]);

impl Owners {
    pub fn set(&self, slot: u8, owner: u64) {
        if let Some(cell) = self.0.get(usize::from(slot)) {
            cell.store(owner, Ordering::Release);
        }
    }

    pub fn get(&self, slot: u16) -> u64 {
        self.0
            .get(usize::from(slot))
            .map_or(0, |cell| cell.load(Ordering::Acquire))
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    owner: u64,
    generation: u32,
    sequence: u32,
    next_position: u64,
    /// The sequence of the last [`Pcm`] handed on; a gap of two tells its player to start over.
    played: u32,
}

/// Admits speaker blocks: the bound owner's only, newer generations and sequences only, and
/// gaps passed on as a skipped [`Pcm`] sequence, so the player drops what it holds.
#[derive(Debug)]
pub struct Receiver {
    owners: std::sync::Arc<Owners>,
    slots: [Option<Slot>; 16],
}

impl Receiver {
    pub fn new(owners: std::sync::Arc<Owners>) -> Self {
        Self {
            owners,
            slots: [None; 16],
        }
    }

    /// The block in `payload`, a 0x5602 message, as the controller's next speaker [`Pcm`].
    pub fn admit(&mut self, payload: &[u8]) -> Option<Pcm> {
        let block = SpeakerBlock::decode(payload)?;
        let owner = self.owners.get(block.controller);
        if owner == 0 || block.owner != owner {
            return None;
        }
        let slot = &mut self.slots[usize::from(block.controller)];
        let gap = match slot {
            Some(old) if old.owner == owner && old.generation == block.generation => {
                if block.sequence.wrapping_sub(old.sequence) as i32 <= 0 {
                    return None;
                }
                block.sequence != old.sequence.wrapping_add(1)
                    || block.sample_position != old.next_position
                    || block.flags & FLAG_DISCONTINUITY != 0
            }
            Some(old)
                if old.owner == owner
                    && block.generation.wrapping_sub(old.generation) as i32 <= 0 =>
            {
                return None;
            }
            _ => true,
        };
        let played = slot.map_or(0, |old| old.played);
        let played = played.wrapping_add(if gap { 2 } else { 1 });
        *slot = Some(Slot {
            owner,
            generation: block.generation,
            sequence: block.sequence,
            next_position: block.sample_position + u64::from(block.frames()),
            played,
        });
        Some(Pcm {
            controller: block.controller,
            sequence: played,
            samples: block.samples.to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn block(
        controller: u16,
        generation: u32,
        sequence: u32,
        position: u64,
        owner: u64,
    ) -> Vec<u8> {
        SpeakerBlock {
            controller,
            generation,
            sequence,
            sample_position: position,
            flags: 0,
            owner,
            samples: &[1; 960],
        }
        .encode()
        .unwrap()
    }

    #[test]
    fn offered_only_where_neither_0x5533_nor_the_touchpad_packet_is() {
        assert!(offer(true, true, true, &[], 0x03));
        assert!(!offer(false, true, true, &[], 0));
        assert!(!offer(true, false, true, &[], 0), "GameStream");
        assert!(!offer(true, true, false, &[], 0), "old control encryption");
        assert!(!offer(true, true, true, &[0x5530, 0x5533], 0), "Rubylight");
        assert!(!offer(true, true, true, &[], 0x10), "Foundation touchpad");
        assert!(
            !offer(true, true, true, &[], 0x20),
            "Foundation touchpad frames"
        );
        assert!(offer(true, true, true, &[0x5534, 0x5535], 0x03));
    }

    #[test]
    fn owners_are_nonzero_and_never_repeat() {
        let a = next_owner().unwrap();
        let b = next_owner().unwrap();
        assert!(a != 0 && b != 0 && b > a);
    }

    #[test]
    fn only_the_bound_owners_blocks_play_in_order() {
        let owners = Arc::new(Owners::default());
        let mut rx = Receiver::new(owners.clone());
        assert!(rx.admit(&block(0, 1, 0, 0, 5)).is_none(), "nothing bound");
        owners.set(0, 5);
        assert!(rx.admit(&block(0, 1, 0, 0, 6)).is_none(), "another owner");
        let first = rx.admit(&block(0, 1, 0, 0, 5)).unwrap();
        assert_eq!((first.controller, first.frames()), (0, 240));
        let next = rx.admit(&block(0, 1, 1, 240, 5)).unwrap();
        assert_eq!(next.sequence, first.sequence + 1, "contiguous");
        assert!(rx.admit(&block(0, 1, 1, 240, 5)).is_none(), "duplicate");
        assert!(rx.admit(&block(0, 1, 0, 0, 5)).is_none(), "old");
        let gap = rx.admit(&block(0, 1, 3, 720, 5)).unwrap();
        assert_eq!(
            gap.sequence,
            next.sequence + 2,
            "a gap makes the player start over"
        );
        let moved = rx.admit(&block(0, 1, 4, 999, 5)).unwrap();
        assert_eq!(
            moved.sequence,
            gap.sequence + 2,
            "a jump in position is a gap too"
        );
        assert!(rx.admit(&[0; 31]).is_none());
    }

    #[test]
    fn a_new_generation_restarts_and_an_old_one_is_dropped() {
        let owners = Arc::new(Owners::default());
        let mut rx = Receiver::new(owners.clone());
        owners.set(2, 9);
        let a = rx.admit(&block(2, 4, 100, 0, 9)).unwrap();
        let b = rx.admit(&block(2, 5, 0, 0, 9)).unwrap();
        assert_eq!(b.sequence, a.sequence + 2);
        assert!(
            rx.admit(&block(2, 4, 101, 240, 9)).is_none(),
            "old generation"
        );
        // A new binding (owner) takes whatever generation comes.
        owners.set(2, 10);
        assert!(
            rx.admit(&block(2, 5, 1, 240, 9)).is_none(),
            "the old controller's"
        );
        assert!(rx.admit(&block(2, 1, 0, 0, 10)).is_some());
    }
}
