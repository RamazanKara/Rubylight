//! Audio passthrough on the receive side: which of the 3.0 audio extension's codecs to ask a
//! host for (`rubylight_protocol::audio_ext`), and the AC3 and E-AC3 frames put back together
//! from their numbered RTP chunks. PCM needs neither: each packet is s16le samples.
use rubylight_protocol::audio_ext::{
    self, ChunkHeader, CHUNK_HEADER_BYTES, CODEC_AC3, CODEC_EAC3, CODEC_OPUS, CODEC_PCM,
};

use super::AudioOutput;

/// Lost frames reported for one gap in the frame numbers; a longer gap is a resync.
const MAX_GAP_FRAMES: u16 = 8;

/// The codec to ask for (one `CODEC_*` bit), or 0 to send nothing and stream Opus. `output` is
/// what the platform's audio output can play for `channels` (PCM, AC3, E-AC3 bits), `offered`
/// what the host's DESCRIBE offered. PCM comes first (lossless, no decoder, the shortest
/// packets), then E-AC3 and AC3. E-AC3 is asked for only up to 5.1: the host's encoder
/// (FFmpeg's) has no 7.1, and the host refuses that pick before the stream starts.
pub fn pick_codec(output: u8, offered: u8, channels: u8) -> u8 {
    [CODEC_PCM, CODEC_EAC3, CODEC_AC3]
        .into_iter()
        .filter(|&codec| codec != CODEC_EAC3 || channels <= 6)
        .find(|&codec| output & offered & codec != 0 && audio_ext::channels_fit(codec, channels))
        .unwrap_or(0)
}

/// The milliseconds of audio in each packet the renderer gets: the negotiated Opus duration,
/// PCM's fixed packet for `channels`, or one 32 ms AC3 or E-AC3 frame. The host steps its
/// timestamps by the same amount.
pub fn packet_ms(codec: u8, channels: u8, opus_ms: u32) -> u32 {
    match codec {
        CODEC_PCM => audio_ext::pcm_packet_ms(channels).map_or(opus_ms, u32::from),
        CODEC_AC3 | CODEC_EAC3 => audio_ext::AC3_FRAME_SAMPLES / 48,
        _ => opus_ms,
    }
}

/// The codec's name for logs and the overlay.
pub fn codec_name(codec: u8) -> &'static str {
    match codec {
        CODEC_PCM => "PCM",
        CODEC_AC3 => "Dolby Digital (AC3)",
        CODEC_EAC3 => "Dolby Digital Plus (E-AC3)",
        CODEC_OPUS | 0 => "Opus",
        _ => "unknown",
    }
}

/// The length of the AC3 or E-AC3 frame at the start of `data`, from its sync header: the host
/// zero-pads a frame's last chunk, and a receiver must get the frame alone. `None` when it is
/// not a 48 kHz AC3 or an E-AC3 frame.
pub fn dolby_frame_len(data: &[u8]) -> Option<usize> {
    /// AC3 bitrates in kbit/s, one per pair of `frmsizecod` values.
    const AC3_KBPS: [usize; 19] = [
        32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 576, 640,
    ];
    if data.len() < 6 || data[0] != 0x0B || data[1] != 0x77 {
        return None;
    }
    match data[5] >> 3 {
        // AC3: 48 kHz (fscod 0) frames are 4 bytes per kbit/s.
        bsid if bsid <= 8 => {
            let (fscod, code) = (data[4] >> 6, usize::from(data[4] & 0x3F));
            (fscod == 0)
                .then(|| AC3_KBPS.get(code / 2).map(|kbps| kbps * 4))
                .flatten()
        }
        // E-AC3: frmsiz is the frame's 16-bit words minus one.
        11..=16 => Some(((usize::from(data[2] & 0x07) << 8 | usize::from(data[3])) + 1) * 2),
        _ => None,
    }
}

/// Puts AC3 and E-AC3 frames back together from the receiver's chunks, in order. A frame with a
/// chunk missing is lost as a whole, reported as one [`AudioOutput::Lost`] per frame number
/// skipped once a later frame completes.
#[derive(Default)]
pub struct FrameAssembler {
    frame: Vec<u8>,
    /// The frame number being collected, while its chunks keep coming in order.
    collecting: Option<u16>,
    /// The last frame number handed on.
    done: Option<u16>,
    /// Frames whose chunks were complete but whose header was not an AC3 or E-AC3 frame.
    pub invalid: u32,
}

impl FrameAssembler {
    /// Takes one step of the receiver's output and passes on the frames it completes.
    pub fn push(&mut self, item: AudioOutput<'_>, out: &mut impl FnMut(AudioOutput<'_>)) {
        let payload = match item {
            AudioOutput::Lost => {
                self.collecting = None;
                return;
            }
            AudioOutput::Packet(payload) => payload,
        };
        let Some(header) = ChunkHeader::decode(payload) else {
            self.collecting = None;
            self.invalid += 1;
            return;
        };
        let piece = &payload[CHUNK_HEADER_BYTES..];
        if header.first {
            self.frame.clear();
            self.collecting = Some(header.frame_index);
        } else if self.collecting != Some(header.frame_index) {
            self.collecting = None;
            return;
        }
        self.frame.extend_from_slice(piece);
        if !header.last {
            return;
        }
        self.collecting = None;
        let index = header.frame_index;
        if let Some(done) = self.done {
            // The receiver hands packets on in order, so a frame number behind the last one
            // is the host starting over: no frames are counted lost for it.
            let gap = index.wrapping_sub(done).wrapping_sub(1);
            let gap = if gap >= u16::MAX / 2 { 0 } else { gap };
            for _ in 0..gap.min(MAX_GAP_FRAMES) {
                out(AudioOutput::Lost);
            }
        }
        self.done = Some(index);
        match dolby_frame_len(&self.frame) {
            Some(len) if len <= self.frame.len() => out(AudioOutput::Packet(&self.frame[..len])),
            _ => {
                self.invalid += 1;
                out(AudioOutput::Lost);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rubylight_protocol::audio_ext::CHUNK_BYTES;

    /// A 48 kHz AC3 frame header for `kbps` (bsid 8), filled to its length.
    fn ac3_frame(kbps: usize, fill: u8) -> Vec<u8> {
        let code = [
            32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 576, 640,
        ]
        .iter()
        .position(|&k| k == kbps)
        .unwrap() as u8
            * 2;
        let mut frame = vec![fill; kbps * 4];
        frame[..6].copy_from_slice(&[0x0B, 0x77, 0, 0, code, 8 << 3]);
        frame
    }

    /// The host's chunks of `frame` (`audio_passthrough::chunks`): each piece behind its header,
    /// the last zero-padded.
    fn chunks(frame: &[u8], frame_index: u16) -> Vec<Vec<u8>> {
        let count = audio_ext::chunks_for(frame.len());
        frame
            .chunks(CHUNK_BYTES)
            .enumerate()
            .map(|(index, piece)| {
                let header = ChunkHeader {
                    first: index == 0,
                    last: index + 1 == count,
                    frame_index,
                };
                let mut payload = header.encode().to_vec();
                payload.extend_from_slice(piece);
                payload.resize(audio_ext::CHUNK_PAYLOAD_BYTES, 0);
                payload
            })
            .collect()
    }

    #[derive(Debug, PartialEq)]
    enum Got {
        Frame(Vec<u8>),
        Lost,
    }

    fn feed(assembler: &mut FrameAssembler, items: &[Option<Vec<u8>>]) -> Vec<Got> {
        let mut got = Vec::new();
        for item in items {
            let output = match item {
                Some(payload) => AudioOutput::Packet(payload),
                None => AudioOutput::Lost,
            };
            assembler.push(output, &mut |out| {
                got.push(match out {
                    AudioOutput::Packet(frame) => Got::Frame(frame.to_vec()),
                    AudioOutput::Lost => Got::Lost,
                })
            });
        }
        got
    }

    #[test]
    fn the_pick_prefers_pcm_then_e_ac3_then_ac3_within_what_both_sides_take() {
        let all = CODEC_PCM | CODEC_AC3 | CODEC_EAC3;
        assert_eq!(pick_codec(all, all, 6), CODEC_PCM);
        assert_eq!(pick_codec(CODEC_AC3 | CODEC_EAC3, all, 6), CODEC_EAC3);
        assert_eq!(
            pick_codec(CODEC_AC3 | CODEC_EAC3, CODEC_PCM | CODEC_AC3, 6),
            CODEC_AC3
        );
        assert_eq!(pick_codec(CODEC_AC3, all, 2), CODEC_AC3);
        // Nothing the host offered, nothing the output takes, or nothing that fits: Opus.
        assert_eq!(pick_codec(all, 0, 6), 0);
        assert_eq!(pick_codec(0, all, 6), 0);
        assert_eq!(pick_codec(CODEC_AC3, all, 8), 0);
        // The host's E-AC3 encoder stops at 5.1: 7.1 never asks for it.
        assert_eq!(pick_codec(CODEC_EAC3, all, 8), 0);
        assert_eq!(pick_codec(CODEC_EAC3 | CODEC_PCM, all, 8), CODEC_PCM);
        assert_eq!(pick_codec(all, all, 4), 0);
    }

    #[test]
    fn packets_last_as_long_as_the_hosts_timestamp_step() {
        assert_eq!(packet_ms(0, 6, 10), 10);
        assert_eq!(packet_ms(CODEC_OPUS, 2, 5), 5);
        for (channels, ms) in [(2, 5), (6, 2), (8, 1)] {
            assert_eq!(packet_ms(CODEC_PCM, channels, 5), ms);
        }
        assert_eq!(packet_ms(CODEC_AC3, 6, 5), 32);
        assert_eq!(packet_ms(CODEC_EAC3, 2, 20), 32);
    }

    #[test]
    fn frame_lengths_come_from_the_sync_header() {
        assert_eq!(dolby_frame_len(&ac3_frame(640, 0)), Some(2560));
        assert_eq!(dolby_frame_len(&ac3_frame(192, 0)), Some(768));
        // E-AC3 (bsid 16): frmsiz 1535 words + 1.
        assert_eq!(
            dolby_frame_len(&[0x0B, 0x77, 0x05, 0xFF, 0, 16 << 3]),
            Some(3072)
        );
        // 44.1 kHz AC3, a bad sync word, a short buffer, an unknown bsid.
        assert_eq!(
            dolby_frame_len(&[0x0B, 0x77, 0, 0, 0x40 | 36, 8 << 3]),
            None
        );
        assert_eq!(dolby_frame_len(&[0x0B, 0x78, 0, 0, 36, 8 << 3]), None);
        assert_eq!(dolby_frame_len(&[0x0B, 0x77, 0, 0]), None);
        assert_eq!(dolby_frame_len(&[0x0B, 0x77, 0, 0, 36, 10 << 3]), None);
    }

    #[test]
    fn chunks_reassemble_into_the_frame_without_its_padding() {
        let mut assembler = FrameAssembler::default();
        let frames = [ac3_frame(640, 1), ac3_frame(640, 2), ac3_frame(192, 3)];
        let items: Vec<_> = frames
            .iter()
            .enumerate()
            .flat_map(|(index, frame)| chunks(frame, index as u16))
            .map(Some)
            .collect();
        assert_eq!(items.len(), 3 + 3 + 1);
        let got = feed(&mut assembler, &items);
        let want: Vec<_> = frames.iter().cloned().map(Got::Frame).collect();
        assert_eq!(got, want);
        assert_eq!(assembler.invalid, 0);
    }

    #[test]
    fn a_frame_missing_a_chunk_is_lost_whole_and_reported_once() {
        let mut assembler = FrameAssembler::default();
        let frame = ac3_frame(640, 7);
        let mut items: Vec<Option<Vec<u8>>> = Vec::new();
        items.extend(chunks(&frame, 0).into_iter().map(Some));
        // Frame 1 loses its middle chunk; frame 2 loses every chunk; frame 3 arrives.
        let mut second: Vec<_> = chunks(&frame, 1).into_iter().map(Some).collect();
        second[1] = None;
        items.extend(second);
        items.extend([None, None, None]);
        items.extend(chunks(&frame, 3).into_iter().map(Some));
        let got = feed(&mut assembler, &items);
        assert_eq!(
            got,
            [
                Got::Frame(frame.clone()),
                Got::Lost,
                Got::Lost,
                Got::Frame(frame.clone())
            ]
        );
    }

    #[test]
    fn a_stream_joined_mid_frame_waits_for_the_next_first_chunk() {
        let mut assembler = FrameAssembler::default();
        let frame = ac3_frame(640, 9);
        let mut items: Vec<_> = chunks(&frame, 40).into_iter().skip(1).map(Some).collect();
        items.extend(chunks(&frame, 41).into_iter().map(Some));
        assert_eq!(feed(&mut assembler, &items), [Got::Frame(frame)]);
    }

    #[test]
    fn frame_numbers_wrap_and_a_long_gap_resyncs() {
        let mut assembler = FrameAssembler::default();
        let frame = ac3_frame(192, 4);
        let items: Vec<_> = [u16::MAX, 0, 500]
            .into_iter()
            .flat_map(|index| chunks(&frame, index))
            .map(Some)
            .collect();
        let got = feed(&mut assembler, &items);
        let mut want = vec![Got::Frame(frame.clone()), Got::Frame(frame.clone())];
        want.extend((0..MAX_GAP_FRAMES).map(|_| Got::Lost));
        want.push(Got::Frame(frame));
        assert_eq!(got, want);
    }

    #[test]
    fn complete_chunks_that_are_not_a_dolby_frame_are_lost_and_counted() {
        let mut assembler = FrameAssembler::default();
        let mut items: Vec<_> = chunks(&[0x55; 1500], 0).into_iter().map(Some).collect();
        items.push(Some(vec![0, 3, 1, 0, 0x0B]));
        assert_eq!(feed(&mut assembler, &items), [Got::Lost]);
        assert_eq!(assembler.invalid, 2);
    }
}
