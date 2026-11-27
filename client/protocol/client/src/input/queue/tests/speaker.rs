//! Speaker bindings (20-byte 0x55000009) before PlayStation arrivals on a stream with speaker
//! PCM (0x5602), and 0x4000 kept only where it means what the host reads it as.

use super::*;
use crate::control::ps_speaker::Owners;
use rubylight_protocol::ps_speaker_pcm::{Binding, CTYPE_PS, CTYPE_STEAM, PS_SPEAKER_CAPABILITY};
use std::sync::Arc;

/// Each packet's channel and either its binding or its decoded input.
fn sent(q: &mut InputQueue) -> Vec<(u8, Result<Binding, Input>)> {
    let mut out = Vec::new();
    q.poll(Instant::now(), false, &mut out);
    out.iter()
        .map(|p| {
            let read = Binding::decode(&p.data).ok_or_else(|| decode(&p.data).unwrap());
            (p.channel, read)
        })
        .collect()
}

fn arrival_caps(packets: &[(u8, Result<Binding, Input>)]) -> Vec<u16> {
    packets
        .iter()
        .filter_map(|(_, read)| match read {
            Err(Input::Arrival { capabilities, .. }) => Some(*capabilities),
            _ => None,
        })
        .collect()
}

#[test]
fn a_playstation_arrival_is_bound_first_on_its_own_channel() {
    let owners = Arc::new(Owners::default());
    let mut q = queue(&sunshine());
    q.set_ps_speaker(Some(owners.clone()));
    q.send_controller_arrival(3, 1 << 3, CTYPE_PS, 0xFFFF, PS_SPEAKER_CAPABILITY | 0x8000)
        .unwrap();
    let packets = sent(&mut q);
    let (channel, Ok(binding)) = &packets[0] else {
        panic!("no binding first: {packets:?}");
    };
    assert_eq!(binding.slot, 3);
    assert_eq!(binding.owner, owners.get(3));
    assert_ne!(binding.owner, 0);
    let (
        arrival_channel,
        Err(Input::Arrival {
            id: 3,
            kind,
            capabilities,
            ..
        }),
    ) = &packets[1]
    else {
        panic!("no arrival second: {packets:?}");
    };
    assert_eq!(
        (channel, *kind, *capabilities),
        (arrival_channel, CTYPE_PS, 0xC000)
    );
    // Once it has left, a later arrival in the slot binds a new owner.
    let first = owners.get(3);
    q.send_controller(3, 0, 0, 0, 0, (0, 0), (0, 0)).unwrap();
    sent(&mut q);
    q.send_controller_arrival(3, 1 << 3, CTYPE_PS, 0, PS_SPEAKER_CAPABILITY)
        .unwrap();
    let packets = sent(&mut q);
    assert!(matches!(&packets[0], (_, Ok(b)) if b.owner == owners.get(3) && b.owner != first));
}

#[test]
fn a_controller_announced_again_keeps_its_binding() {
    let owners = Arc::new(Owners::default());
    let mut q = queue(&sunshine());
    q.set_ps_speaker(Some(owners.clone()));
    q.send_controller_arrival(1, 0b10, CTYPE_PS, 0, PS_SPEAKER_CAPABILITY)
        .unwrap();
    let owner = owners.get(1);
    sent(&mut q);
    // Its audio changed: the host has it and would ignore a second binding.
    q.send_controller_arrival(1, 0b10, CTYPE_PS, 0, PS_SPEAKER_CAPABILITY | 0x8000)
        .unwrap();
    let packets = sent(&mut q);
    assert!(packets.iter().all(|(_, read)| read.is_err()), "{packets:?}");
    assert_eq!(arrival_caps(&packets), [0xC000]);
    assert_eq!(owners.get(1), owner);
    // A slot the host has without a binding gets none either, nor 0x4000.
    q.send_controller_arrival(1, 0b10, CTYPE_PS, 0, 0).unwrap();
    q.send_controller_arrival(1, 0b10, CTYPE_PS, 0, PS_SPEAKER_CAPABILITY)
        .unwrap();
    let packets = sent(&mut q);
    assert!(packets.iter().all(|(_, read)| read.is_err()), "{packets:?}");
    assert_eq!(arrival_caps(&packets), [0, 0]);
    assert_eq!(owners.get(1), 0);
}

#[test]
fn without_speaker_pcm_a_playstation_arrival_drops_0x4000_and_sends_no_binding() {
    let mut q = queue(&sunshine());
    q.send_controller_arrival(0, 1, CTYPE_PS, 0, PS_SPEAKER_CAPABILITY | 0x8000)
        .unwrap();
    let packets = sent(&mut q);
    assert!(packets.iter().all(|(_, read)| read.is_err()), "{packets:?}");
    assert_eq!(arrival_caps(&packets), [0x8000]);
}

#[test]
fn a_steam_controller_keeps_0x4000_unbound_either_way() {
    for speaker in [false, true] {
        let owners = Arc::new(Owners::default());
        let mut q = queue(&sunshine());
        q.set_ps_speaker(speaker.then(|| owners.clone()));
        q.send_controller_arrival(1, 2, CTYPE_STEAM, 0, PS_SPEAKER_CAPABILITY | 0x800)
            .unwrap();
        let packets = sent(&mut q);
        assert!(packets.iter().all(|(_, read)| read.is_err()), "{packets:?}");
        assert_eq!(arrival_caps(&packets), [0x4800]);
        assert_eq!(owners.get(1), 0);
    }
}

#[test]
fn a_playstation_arrival_without_0x4000_clears_its_slots_owner() {
    let owners = Arc::new(Owners::default());
    let mut q = queue(&sunshine());
    q.set_ps_speaker(Some(owners.clone()));
    q.send_controller_arrival(2, 4, CTYPE_PS, 0, PS_SPEAKER_CAPABILITY)
        .unwrap();
    assert_ne!(owners.get(2), 0);
    q.send_controller_arrival(2, 4, CTYPE_PS, 0, 0).unwrap();
    assert_eq!(owners.get(2), 0);
    let packets = sent(&mut q);
    assert_eq!(packets.iter().filter(|(_, read)| read.is_ok()).count(), 1);
}

#[test]
fn a_held_binding_is_replayed_before_its_arrival() {
    let owners = Arc::new(Owners::default());
    let mut q = queue(&sunshine());
    q.set_ps_speaker(Some(owners.clone()));
    q.send_controller_arrival(5, 1 << 5, CTYPE_PS, 0, PS_SPEAKER_CAPABILITY)
        .unwrap();
    let (now, mut out) = (Instant::now(), Vec::new());
    // The host is silent: everything is held, then the latest state goes out.
    q.poll(now, true, &mut out);
    assert!(out.is_empty() && q.is_holding());
    q.poll(now, false, &mut out);
    let order: Vec<bool> = out
        .iter()
        .filter(|p| {
            [0x5500_0009u32, 0x5500_0004]
                .iter()
                .any(|magic| p.data[4..8] == magic.to_le_bytes())
        })
        .map(|p| Binding::decode(&p.data).is_some())
        .collect();
    assert_eq!(order, [true, false], "binding, then arrival");
}

const CTYPE_XBOX: u8 = 1;

fn bindings_in(packets: &[InputPacket]) -> Vec<Binding> {
    packets
        .iter()
        .filter_map(|p| Binding::decode(&p.data))
        .collect()
}

#[test]
fn disconnect_clears_owner_and_drops_held_speaker_binding() {
    let owners = Arc::new(Owners::default());
    let mut q = queue(&sunshine());
    q.set_ps_speaker(Some(owners.clone()));

    // Live disconnect clears the slot's owner immediately.
    q.send_controller_arrival(0, 1, CTYPE_PS, 0, PS_SPEAKER_CAPABILITY)
        .unwrap();
    assert_ne!(owners.get(0), 0);
    sent(&mut q);
    q.send_controller(0, 0, 0, 0, 0, (0, 0), (0, 0)).unwrap();
    assert_eq!(owners.get(0), 0);
    sent(&mut q);

    // Arrival followed by disconnect while queued together before hold drops held speaker_bindings.
    let (now, mut out) = (Instant::now(), Vec::new());
    q.send_controller_arrival(0, 1, CTYPE_PS, 0, PS_SPEAKER_CAPABILITY)
        .unwrap();
    q.send_controller(0, 0, 0, 0, 0, (0, 0), (0, 0)).unwrap();
    assert_eq!(owners.get(0), 0);
    q.poll(now, true, &mut out);
    assert!(out.is_empty() && q.is_holding());
    q.poll(now, false, &mut out);
    assert!(
        bindings_in(&out).is_empty(),
        "stale binding must not replay after disconnect"
    );

    // Arrival already held, then disconnect arrives while still holding before release.
    out.clear();
    q.send_controller_arrival(0, 1, CTYPE_PS, 0, PS_SPEAKER_CAPABILITY)
        .unwrap();
    q.poll(now, true, &mut out);
    assert_ne!(owners.get(0), 0);
    q.send_controller(0, 0, 0, 0, 0, (0, 0), (0, 0)).unwrap();
    assert_eq!(owners.get(0), 0);
    q.poll(now, false, &mut out);
    assert!(
        bindings_in(&out).is_empty(),
        "held binding cleared before release on disconnect"
    );
}

#[test]
fn replacing_dualsense_with_xbox_in_slot_0_clears_owner_and_held_binding() {
    let owners = Arc::new(Owners::default());
    let mut q = queue(&sunshine());
    q.set_ps_speaker(Some(owners.clone()));

    // Live DualSense -> Xbox replacement in slot 0 clears owner even while active_mask stays 1.
    q.send_controller_arrival(0, 1, CTYPE_PS, 0, PS_SPEAKER_CAPABILITY)
        .unwrap();
    assert_ne!(owners.get(0), 0);
    sent(&mut q);
    q.send_controller_arrival(0, 1, CTYPE_XBOX, 0, 0).unwrap();
    assert_eq!(owners.get(0), 0);
    sent(&mut q);
    // Clear slot 0 before testing held scenarios.
    q.send_controller(0, 0, 0, 0, 0, (0, 0), (0, 0)).unwrap();
    sent(&mut q);

    // DualSense -> Xbox queued together before entering hold drops held speaker_bindings[0].
    let (now, mut out) = (Instant::now(), Vec::new());
    q.send_controller_arrival(0, 1, CTYPE_PS, 0, PS_SPEAKER_CAPABILITY)
        .unwrap();
    q.send_controller_arrival(0, 1, CTYPE_XBOX, 0, 0).unwrap();
    assert_eq!(owners.get(0), 0);
    q.poll(now, true, &mut out);
    q.poll(now, false, &mut out);
    assert!(
        bindings_in(&out).is_empty(),
        "Xbox replacement in slot 0 must not replay PS binding"
    );

    // DualSense already held, then Xbox arrives in slot 0 before contact returns.
    q.send_controller(0, 0, 0, 0, 0, (0, 0), (0, 0)).unwrap();
    sent(&mut q);
    out.clear();
    q.send_controller_arrival(0, 1, CTYPE_PS, 0, PS_SPEAKER_CAPABILITY)
        .unwrap();
    q.poll(now, true, &mut out);
    assert_ne!(owners.get(0), 0);
    q.send_controller_arrival(0, 1, CTYPE_XBOX, 0, 0).unwrap();
    assert_eq!(owners.get(0), 0);
    q.poll(now, false, &mut out);
    assert!(
        bindings_in(&out).is_empty(),
        "held PS binding cleared when replaced by Xbox"
    );

    // DualSense held, then PlayStation arrival without speaker capability drops held binding too.
    q.send_controller(0, 0, 0, 0, 0, (0, 0), (0, 0)).unwrap();
    sent(&mut q);
    out.clear();
    q.send_controller_arrival(0, 1, CTYPE_PS, 0, PS_SPEAKER_CAPABILITY)
        .unwrap();
    q.send_controller_arrival(0, 1, CTYPE_PS, 0, 0).unwrap();
    assert_eq!(owners.get(0), 0);
    q.poll(now, true, &mut out);
    q.poll(now, false, &mut out);
    assert!(
        bindings_in(&out).is_empty(),
        "PS arrival without speaker cap must not replay binding"
    );
}
