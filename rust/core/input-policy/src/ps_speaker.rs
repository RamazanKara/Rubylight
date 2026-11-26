//! A client's PlayStation controller speaker over Nonary's speaker PCM (0x5602), for clients
//! that bind a controller with the 20-byte 0x55000009 packet: Nonary's Moonlight and others that
//! speak Vibepollo's format. Rubylight's own clients use its USB controller audio (0x5533 and
//! 0x5535, [`crate::controller_audio`]) instead; a controller takes one of the two, never both.
//!
//! The rules follow Vibepollo's (Nonary/vibeshine pull request 337, `src/controller_speaker_pcm.h`
//! and `docs/controller_speaker_pcm.md`, GPL-3.0-or-later): a binding counts only on a session that
//! negotiated it, only for a slot without a controller, and only once, at that slot's next
//! arrival, which must be a PlayStation one with the speaker capability. Every block echoes the
//! binding's owner and a generation that changes with each binding, so a client drops blocks
//! of a controller it no longer has.

use rubylight_protocol::ps_speaker_pcm::{
    BINDING_BYTES, BINDING_MAGIC, Binding, CTYPE_PS, FLAG_DISCONTINUITY, MAX_FRAMES,
    PS_SPEAKER_CAPABILITY, SESSION_FEATURE, SpeakerBlock, speaker_capable,
};
pub use rubylight_protocol::ps_speaker_pcm::{CHANNEL, PS_SPEAKER_PCM_MESSAGE_TYPE};

/// Whether a session takes speaker bindings and gets speaker blocks: the client announced
/// [`SESSION_FEATURE`], the control stream is encrypted (`encryption` bit 0 and
/// `x-nv-general.useReliableUdp` 13, Vibepollo's "protocol 13") and ENet allocated [`CHANNEL`].
pub fn negotiated(
    moonlight_features: u32,
    encryption: u32,
    reliable_control: u32,
    channels: usize,
) -> bool {
    moonlight_features & SESSION_FEATURE != 0
        && encryption & 1 != 0
        && reliable_control == 13
        && channels > usize::from(CHANNEL)
}

/// Whether an input packet is a speaker binding by its size and magic alone, before it is
/// decoded: exactly [`BINDING_BYTES`] with magic 0x55000009. Foundation Sunshine's touchpad
/// packet with that magic is 40 bytes and never one, so it still reaches the input decoder.
pub fn is_binding(packet: &[u8]) -> bool {
    packet.len() == BINDING_BYTES && packet[4..8] == BINDING_MAGIC.to_le_bytes()
}

/// Whether an arrival asks for the composite USB DualSense, whose audio endpoint the game's
/// haptics and speaker sound come from: waveform haptics (0x8000), or a speaker bound by
/// [`Speakers::arrival`], which leaves 0x4000 on a PlayStation arrival only when bound.
pub fn wants_usb_dualsense(kind: u8, capabilities: u16) -> bool {
    capabilities & crate::haptics::CAPABILITY != 0 || speaker_capable(kind, capabilities)
}

/// Why a binding was not kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The session did not negotiate speaker PCM.
    NotNegotiated,
    /// Not a valid binding (length field, version, slot or owner).
    Malformed,
    /// The slot already has a controller.
    SlotInUse,
}

/// One client's speaker bindings and the blocks it is sent.
#[derive(Debug, Default)]
pub struct Speakers {
    /// Owners bound for each slot's next arrival.
    pending: [u64; 16],
    /// The owner of each slot's controller, 0 when it has none or plays no speaker blocks.
    owners: [u64; 16],
    generations: [u32; 16],
    last_generation: u32,
    /// The last frame sequence sent for each slot in its generation.
    last_sequence: [Option<u32>; 16],
    /// The next block's sample position for each slot.
    positions: [u64; 16],
    /// Slots with a controller: arrivals set them, controller packets' masks replace them.
    active: u16,
}

impl Speakers {
    /// Keeps a binding for its slot's next arrival.
    pub fn bind(&mut self, packet: &[u8], negotiated: bool) -> Result<u16, Refused> {
        if !negotiated {
            return Err(Refused::NotNegotiated);
        }
        let binding = Binding::decode(packet).ok_or(Refused::Malformed)?;
        if self.active & (1 << binding.slot) != 0 {
            return Err(Refused::SlotInUse);
        }
        self.pending[usize::from(binding.slot)] = binding.owner;
        Ok(binding.slot)
    }

    /// Controller `id`'s arrival: whether it takes the speaker, and the capabilities the rest of
    /// the host sees. A PlayStation arrival without a pending binding on a negotiated session
    /// loses [`PS_SPEAKER_CAPABILITY`], so nothing reads it as asking for speaker sound, unless
    /// it announces a bound controller that never left again (a binding for its slot would have
    /// been ignored): that one keeps its binding. Every other type keeps its 0x4000 (a Steam
    /// Controller's attributes).
    pub fn arrival(
        &mut self,
        id: u8,
        kind: u8,
        capabilities: u16,
        negotiated: bool,
    ) -> (u16, bool) {
        let Some(slot) = (id < 16).then_some(usize::from(id)) else {
            return (capabilities, false);
        };
        let present = self.active & (1 << id) != 0;
        self.active |= 1 << id;
        let owner = std::mem::take(&mut self.pending[slot]);
        if owner == 0
            && present
            && self.owners[slot] != 0
            && negotiated
            && speaker_capable(kind, capabilities)
        {
            return (capabilities, true);
        }
        self.reset(slot);
        if negotiated && owner != 0 && speaker_capable(kind, capabilities) {
            self.owners[slot] = owner;
            self.last_generation = self.last_generation.wrapping_add(1).max(1);
            self.generations[slot] = self.last_generation;
            return (capabilities, true);
        }
        let capabilities = if kind == CTYPE_PS {
            capabilities & !PS_SPEAKER_CAPABILITY
        } else {
            capabilities
        };
        (capabilities, false)
    }

    /// A controller packet's mask of the slots with a controller: the others lose their speaker.
    pub fn controllers(&mut self, active: u16) {
        for slot in 0..16 {
            if active & (1 << slot) == 0 {
                self.reset(slot);
            }
        }
        self.active = active;
    }

    fn reset(&mut self, slot: usize) {
        self.owners[slot] = 0;
        self.generations[slot] = 0;
        self.last_sequence[slot] = None;
        self.positions[slot] = 0;
    }

    /// Whether controller `id` plays speaker blocks rather than Rubylight's USB controller audio.
    pub fn bound(&self, id: u16) -> bool {
        self.owners
            .get(usize::from(id))
            .is_some_and(|owner| *owner != 0)
    }

    /// Controllers bound to the speaker, a bit each.
    pub fn bound_mask(&self) -> u16 {
        (0..16)
            .filter(|slot| self.owners[*slot] != 0)
            .fold(0, |mask, slot| mask | (1 << slot))
    }

    /// The 0x5602 payload for one speaker frame of controller `id` (the virtual DualSense's
    /// speaker channels, `sequence` counting its frames), or `None` when `id` is not bound. A
    /// frame after a gap in `sequence` (frames dropped as late) is flagged a discontinuity, and
    /// its sample position skips what was missed.
    pub fn block(&mut self, id: u16, sequence: u32, samples: &[u8]) -> Option<Vec<u8>> {
        let slot = usize::from(id);
        if !self.bound(id) {
            return None;
        }
        let frames = (samples.len() / 4) as u64;
        if frames == 0 || frames > u64::from(MAX_FRAMES) || !samples.len().is_multiple_of(4) {
            return None;
        }
        let missed = self.last_sequence[slot]
            .map(|last| u64::from(sequence.wrapping_sub(last).wrapping_sub(1)))
            .unwrap_or(0);
        // A sequence that went backwards is a stale frame, not a gap of four billion.
        if missed >= u64::from(u32::MAX / 2) {
            return None;
        }
        let position = self.positions[slot].saturating_add(missed.saturating_mul(frames));
        let payload = SpeakerBlock {
            controller: id,
            generation: self.generations[slot],
            sequence,
            sample_position: position,
            flags: if missed > 0 { FLAG_DISCONTINUITY } else { 0 },
            owner: self.owners[slot],
            samples,
        }
        .encode()?;
        self.last_sequence[slot] = Some(sequence);
        self.positions[slot] = position.saturating_add(frames);
        Some(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rubylight_protocol::ps_speaker_pcm::CTYPE_STEAM;

    fn binding(slot: u16, owner: u64) -> [u8; 20] {
        Binding { slot, owner }.encode().unwrap()
    }

    #[test]
    fn negotiation_needs_the_flag_encryption_protocol_13_and_the_channel() {
        assert!(negotiated(0x0F, 1, 13, 9));
        assert!(!negotiated(0x07, 1, 13, 9), "no 0x08");
        assert!(!negotiated(0x08, 0, 13, 9), "unencrypted control");
        assert!(!negotiated(0x08, 1, 1, 9), "another control protocol");
        assert!(!negotiated(0x08, 1, 13, 8), "channel 8 not allocated");
    }

    #[test]
    fn a_binding_is_told_from_a_touchpad_packet_by_length() {
        assert!(is_binding(&binding(0, 1)));
        let mut touchpad = vec![0, 0, 0, 36];
        touchpad.extend_from_slice(&BINDING_MAGIC.to_le_bytes());
        touchpad.resize(40, 0);
        assert!(!is_binding(&touchpad));
        // A touchpad packet cut to 20 bytes goes to the binding reader, which refuses it: it
        // was never a valid input packet, and its length field still says 36.
        assert!(is_binding(&touchpad[..20]));
        let mut s = Speakers::default();
        assert_eq!(s.bind(&touchpad[..20], true), Err(Refused::Malformed));
        let mut other = binding(0, 1);
        other[4] = 0x0A;
        assert!(!is_binding(&other));
        assert!(!is_binding(&binding(0, 1)[..19]));
    }

    #[test]
    fn a_bound_playstation_arrival_takes_the_speaker_once() {
        let mut s = Speakers::default();
        assert_eq!(s.bind(&binding(2, 77), true), Ok(2));
        assert_eq!(
            s.arrival(2, CTYPE_PS, 0x4000 | 0x8000, true),
            (0xC000, true)
        );
        assert!(s.bound(2));
        assert_eq!(s.bound_mask(), 1 << 2);
        // The slot is taken now: a second binding for it is refused.
        assert_eq!(s.bind(&binding(2, 78), true), Err(Refused::SlotInUse));
        // Announced again without leaving, it keeps the binding; without 0x4000 it loses it.
        assert_eq!(s.arrival(2, CTYPE_PS, 0x4000, true), (0x4000, true));
        assert_eq!(s.owners[2], 77);
        assert_eq!(s.arrival(2, CTYPE_PS, 0x8000, true), (0x8000, false));
        assert!(!s.bound(2));
        assert_eq!(s.arrival(2, CTYPE_PS, 0x4000, true), (0, false));
        assert_eq!(s.bind(&binding(2, 79), true), Err(Refused::SlotInUse));
        // Leaving frees it; the next arrival without a binding has no speaker.
        s.controllers(0);
        assert!(!s.bound(2));
        assert_eq!(s.arrival(2, CTYPE_PS, 0x4000, true), (0, false));
    }

    #[test]
    fn bindings_count_only_negotiated_valid_and_for_playstation() {
        let mut s = Speakers::default();
        assert_eq!(s.bind(&binding(1, 5), false), Err(Refused::NotNegotiated));
        let mut bad = binding(1, 5);
        bad[10] = 2;
        assert_eq!(s.bind(&bad, true), Err(Refused::Malformed));
        assert_eq!(s.arrival(1, CTYPE_PS, 0x4000, true), (0, false));
        // A binding is spent by the next arrival even when that one cannot use it.
        s.controllers(0);
        assert_eq!(s.bind(&binding(1, 5), true), Ok(1));
        assert_eq!(s.arrival(1, CTYPE_PS, 0x0004, true), (0x0004, false));
        s.controllers(0);
        assert_eq!(s.arrival(1, CTYPE_PS, 0x4000, true), (0, false));
        // Not negotiated at arrival: no speaker, and the capability is dropped.
        s.controllers(0);
        assert_eq!(s.bind(&binding(1, 5), true), Ok(1));
        assert_eq!(s.arrival(1, CTYPE_PS, 0x4000, false), (0, false));
    }

    #[test]
    fn the_usb_dualsense_is_for_haptics_or_a_bound_playstation_speaker() {
        assert!(wants_usb_dualsense(CTYPE_PS, 0x8000));
        assert!(wants_usb_dualsense(CTYPE_PS, 0x4000));
        assert!(wants_usb_dualsense(0, 0x8000));
        assert!(!wants_usb_dualsense(CTYPE_STEAM, 0x4000));
        assert!(!wants_usb_dualsense(CTYPE_PS, 0x3FFF));
    }

    #[test]
    fn a_steam_controller_keeps_its_0x4000_and_never_takes_the_speaker() {
        let mut s = Speakers::default();
        assert_eq!(s.bind(&binding(3, 9), true), Ok(3));
        assert_eq!(
            s.arrival(3, CTYPE_STEAM, 0x4000 | 0x800, true),
            (0x4800, false)
        );
        assert!(!s.bound(3));
        assert_eq!(s.block(3, 0, &[0; 960]), None);
        // Other types keep the bit too, unbound.
        assert_eq!(s.arrival(4, 1, 0x4000, true), (0x4000, false));
    }

    #[test]
    fn blocks_echo_owner_and_generation_and_flag_gaps() {
        let mut s = Speakers::default();
        s.bind(&binding(0, 0xABCD), true).unwrap();
        s.arrival(0, CTYPE_PS, 0x4000, true);
        let samples = [1u8; 960];
        let first = s.block(0, 10, &samples).unwrap();
        let block = SpeakerBlock::decode(&first).unwrap();
        assert_eq!(
            (
                block.owner,
                block.generation,
                block.sequence,
                block.sample_position,
                block.flags
            ),
            (0xABCD, 1, 10, 0, 0)
        );
        let next = s.block(0, 11, &samples).unwrap();
        assert_eq!(SpeakerBlock::decode(&next).unwrap().sample_position, 240);
        // Frames 12 and 13 were late and dropped.
        let gap = s.block(0, 14, &samples).unwrap();
        let gap = SpeakerBlock::decode(&gap).unwrap();
        assert_eq!(
            (gap.sample_position, gap.flags),
            (480 + 480, FLAG_DISCONTINUITY)
        );
        // An old frame is not sent.
        assert_eq!(s.block(0, 13, &samples), None);
        assert_eq!(first.len(), 992);
        // A new binding is a new generation.
        s.controllers(0);
        s.bind(&binding(0, 0xABCE), true).unwrap();
        s.arrival(0, CTYPE_PS, 0x4000, true);
        let block = s.block(0, 15, &samples).unwrap();
        let block = SpeakerBlock::decode(&block).unwrap();
        assert_eq!(
            (block.owner, block.generation, block.sample_position),
            (0xABCE, 2, 0)
        );
        assert_eq!(s.block(1, 0, &samples), None, "unbound slot");
        assert_eq!(s.block(0, 16, &[]), None);
        assert_eq!(s.block(0, 16, &[0; 964]), None);
    }
}
