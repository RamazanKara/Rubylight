//! Sessions against a loopback host of the `x-ss-audio` dialect: the handshake accepts its AC3
//! offer, and its unchunked 640 kbit/s AC3 frames (2572-byte datagrams, parity included) reach
//! the renderer whole, one per 32 ms packet. Without the host's confirmation, or with
//! passthrough off, the stream plays Opus.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use nightjar_core::fec::{AUDIO_PARITY_MATRIX, ReedSolomon};
use nightjar_core::sdp::{AudioConfiguration, StreamConfig};
use nightjar_core::session::{Location, SessionConfig};
use rubylight_protocol::audio_ext::{CODEC_AC3, CODEC_EAC3, CODEC_OPUS};

use crate::host::{Media, start_dolby_ss_host};
use crate::media::{KEY, PACKET_SIZE};
use crate::recorder::{Record, start_session, until};

const FRAMES: u16 = 48;

/// A 640 kbit/s 5.1 AC3 frame, the longest AC3 frame (2560 bytes), distinct per index.
fn ac3_frame(index: u16) -> Vec<u8> {
    let mut frame: Vec<u8> = (0..2560u32)
        .map(|i| (i as u16 ^ index.wrapping_mul(31)) as u8)
        .collect();
    frame[..7].copy_from_slice(&[0x0B, 0x77, 0, 0, 36, 8 << 3, 7 << 5]);
    frame
}

/// Such a host's datagrams: one whole frame per RTP packet, timestamps stepping by one 32 ms
/// frame, and two parity packets after every fourth.
fn dolby_stream(count: u16) -> Vec<Vec<u8>> {
    let rs = ReedSolomon::with_parity_matrix(4, 2, &AUDIO_PARITY_MATRIX).unwrap();
    let rtp = |kind: u8, seq: u16, timestamp: u32| {
        let mut p = vec![0x80, kind];
        p.extend_from_slice(&seq.to_be_bytes());
        p.extend_from_slice(&timestamp.to_be_bytes());
        p.extend_from_slice(&[0; 4]);
        p
    };
    let mut out = Vec::new();
    let mut block = Vec::new();
    for seq in 0..count {
        let frame = ac3_frame(seq);
        let mut packet = rtp(97, seq, u32::from(seq) * 32);
        packet.extend_from_slice(&frame);
        out.push(packet);
        block.push(frame);
        if block.len() == 4 {
            let size = block[0].len();
            let mut shards = std::mem::take(&mut block);
            shards.extend([vec![0; size], vec![0; size]]);
            let mut refs: Vec<&mut [u8]> = shards.iter_mut().map(|s| &mut s[..]).collect();
            rs.encode(&mut refs, size).unwrap();
            for (index, shard) in shards[4..].iter().enumerate() {
                let mut packet = rtp(127, seq + 1 + index as u16, 0);
                packet.extend_from_slice(&[index as u8, 97]);
                packet.extend_from_slice(&(seq - 3).to_be_bytes());
                packet.extend_from_slice(&(u32::from(seq - 3) * 32).to_be_bytes());
                packet.extend_from_slice(&[0; 4]);
                packet.extend_from_slice(shard);
                out.push(packet);
            }
        }
    }
    out
}

/// A 5.1 stream asking for passthrough of what `output` names.
fn config(output: u8) -> SessionConfig {
    SessionConfig {
        stream: StreamConfig {
            width: 1280,
            height: 720,
            fps: 60,
            packet_size: PACKET_SIZE as i32,
            remote_input_aes_key: KEY,
            audio: AudioConfiguration::SURROUND_51,
            audio_passthrough: output,
            ..Default::default()
        },
        location: Location::Auto,
        ..Default::default()
    }
}

#[test]
fn a_confirmed_ac3_offer_plays_unchunked_frames_whole() {
    let host = start_dolby_ss_host("AC3");
    let record = Arc::new(Mutex::new(Record::default()));
    let session = start_session(&host, config(CODEC_AC3 | CODEC_EAC3), &record);
    let outcome = session.outcome();
    assert_eq!(
        (outcome.audio_codec, outcome.audio_unchunked),
        (CODEC_AC3, true)
    );
    assert!(outcome.sdp.contains("a=x-ss-audio-codec-accept:AC3 \r\n"));
    let setup = record.lock().unwrap().audio_setup.unwrap();
    assert_eq!(
        (
            setup.codec,
            setup.packet_duration_ms,
            setup.samples_per_frame
        ),
        (CODEC_AC3, 32, 1536)
    );
    assert_eq!(setup.configuration.channel_count, 6);

    let client = host.peer(Media::Audio);
    let packets = dolby_stream(FRAMES);
    assert!(
        packets
            .iter()
            .all(|p| p.len() == 12 + 2560 || p.len() == 24 + 2560)
    );
    for packet in &packets {
        host.audio.send_to(packet, client).unwrap();
        thread::sleep(Duration::from_micros(500));
    }
    let last = Some(ac3_frame(FRAMES - 1));
    until("the last AC3 frame", || {
        record.lock().unwrap().audio.last() == Some(&last)
    });
    session.stop();

    // Half a second of backlog is dropped and the queue joins at a block; from there every
    // frame plays whole and in order, none lost.
    let r = record.lock().unwrap();
    let first = r.audio[0].as_deref().expect("a frame, not a loss");
    let start = (0..FRAMES)
        .find(|&i| ac3_frame(i) == first)
        .expect("a frame sent");
    assert!(start <= 20, "{start}");
    let want: Vec<_> = (start..FRAMES).map(|i| Some(ac3_frame(i))).collect();
    assert_eq!(r.audio, want);
}

#[test]
fn without_the_hosts_confirmation_or_passthrough_the_stream_plays_opus() {
    // The host answers Opus to an accept; with passthrough off nothing is accepted at all.
    for (confirm, output, accepted) in [
        ("Opus", CODEC_AC3 | CODEC_EAC3, true),
        ("AC3", 0, false),
        ("AC3", CODEC_EAC3, false),
    ] {
        let host = start_dolby_ss_host(confirm);
        let record = Arc::new(Mutex::new(Record::default()));
        let session = start_session(&host, config(output), &record);
        let outcome = session.outcome();
        assert_eq!((outcome.audio_codec, outcome.audio_unchunked), (0, false));
        assert_eq!(
            outcome.sdp.contains("x-ss-audio-codec-accept"),
            accepted,
            "{confirm} {output}"
        );
        let setup = record.lock().unwrap().audio_setup.unwrap();
        assert_eq!(setup.codec, CODEC_OPUS);
        session.stop();
    }
}
