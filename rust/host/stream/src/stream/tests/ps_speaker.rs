//! Nonary's controller speaker PCM (0x5602) in the control pass: negotiation read from a real
//! ANNOUNCE, and a bound controller's speaker frames going out as 0x5602 alone, never also as
//! Rubylight's 0x5533 or with its 0x5535 state.
use super::super::*;
use butterpollo_input_policy::ps_speaker::{self, PS_SPEAKER_PCM_MESSAGE_TYPE, Speakers};
use rubylight_protocol::ps_speaker_pcm::{Binding, CTYPE_PS, SpeakerBlock};

fn negotiated(sdp: &[u8], channels: usize) -> bool {
    let n = butterpollo_media_policy::rtsp::Negotiated::from_sdp(sdp).unwrap();
    ps_speaker::negotiated(
        n.moonlight_features,
        n.encryption,
        n.reliable_control,
        channels,
    )
}

#[test]
fn speaker_pcm_needs_the_flag_encrypted_protocol_13_control_and_channel_8() {
    let nonary = b"a=x-ml-general.featureFlags:15\na=x-ss-general.encryptionEnabled:1\n";
    assert!(negotiated(nonary, 9));
    assert!(!negotiated(nonary, 8));
    let reliable = b"a=x-ml-general.featureFlags:15\na=x-ss-general.encryptionEnabled:1\n\
        a=x-nv-general.useReliableUdp:1\n";
    assert!(!negotiated(reliable, 9));
    assert!(!negotiated(
        b"a=x-ml-general.featureFlags:7\na=x-ss-general.encryptionEnabled:1\n",
        9
    ));
    assert!(!negotiated(b"a=x-ml-general.featureFlags:15\n", 9));
}

fn bound(slot: u8) -> Speakers {
    let mut speakers = Speakers::default();
    let binding = Binding {
        slot: u16::from(slot),
        owner: 0x5150,
    }
    .encode()
    .unwrap();
    speakers.bind(&binding, true).unwrap();
    assert_eq!(
        speakers.arrival(slot, CTYPE_PS, 0x4000, true),
        (0x4000, true)
    );
    speakers
}

fn speaker(id: u16, sequence: u32) -> PadReport {
    PadReport::Speaker(haptics::Frame {
        id,
        sequence,
        created: Instant::now(),
        samples: vec![3; 960],
    })
}

fn pass(
    reports: Vec<PadReport>,
    controller_audio: &[u8; 16],
    speakers: &mut Speakers,
    pcm: &mut controller_audio::PcmTraffic,
) -> PadSends {
    pad_reports(
        reports,
        |_| true,
        true,
        controller_audio,
        speakers,
        pcm,
        &mut Default::default(),
        &mut Default::default(),
    )
}

#[test]
fn a_bound_controller_gets_speaker_pcm_and_never_usb_controller_audio() {
    let mut speakers = bound(1);
    let mut pcm = controller_audio::PcmTraffic::default();
    // Even with a PLAYBACK state for the slot, the bound controller's frames go as 0x5602.
    let mut flags = [0; 16];
    flags[1] = controller_audio::PLAYBACK;
    flags[2] = controller_audio::PLAYBACK;
    let state = PadReport::Feedback {
        id: 1,
        kind: controller_audio::CONTROL,
        data: vec![0; 12],
    };
    let other_state = PadReport::Feedback {
        id: 2,
        kind: controller_audio::CONTROL,
        data: vec![0; 12],
    };
    let sends = pass(
        vec![speaker(1, 40), speaker(2, 7), state, other_state],
        &flags,
        &mut speakers,
        &mut pcm,
    );
    let kinds: Vec<u16> = sends.waveforms.iter().map(|(kind, _)| *kind).collect();
    assert_eq!(
        kinds,
        [PS_SPEAKER_PCM_MESSAGE_TYPE, controller_audio::SPEAKER]
    );
    let block = SpeakerBlock::decode(&sends.waveforms[0].1).unwrap();
    assert_eq!(
        (block.controller, block.owner, block.frames()),
        (1, 0x5150, 240)
    );
    // Its virtual DualSense's USB audio state stays home; the unbound slot's goes out.
    let messages: Vec<(u16, u8)> = sends
        .messages
        .iter()
        .map(|(kind, payload)| (*kind, payload[0]))
        .collect();
    assert_eq!(messages, [(controller_audio::CONTROL, 0)]);
    assert_eq!(pcm.streams(Instant::now()), 2);
}

#[test]
fn an_unbound_controller_keeps_usb_controller_audio_and_late_frames_make_a_gap() {
    let mut speakers = bound(0);
    let mut pcm = controller_audio::PcmTraffic::default();
    let sends = pass(vec![speaker(3, 1)], &[0; 16], &mut speakers, &mut pcm);
    assert!(
        sends.waveforms.is_empty(),
        "no PLAYBACK, no binding: nothing"
    );
    let late = PadReport::Speaker(haptics::Frame {
        id: 0,
        sequence: 1,
        created: Instant::now() - Duration::from_millis(50),
        samples: vec![3; 960],
    });
    let sends = pass(
        vec![speaker(0, 0), late, speaker(0, 2)],
        &[0; 16],
        &mut speakers,
        &mut pcm,
    );
    let blocks: Vec<_> = sends
        .waveforms
        .iter()
        .map(|(_, payload)| SpeakerBlock::decode(payload).unwrap())
        .map(|block| (block.sequence, block.sample_position, block.flags))
        .collect();
    assert_eq!(blocks, [(0, 0, 0), (2, 480, 1)]);
}
