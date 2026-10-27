//! Haptics from game audio on the control loop (`butterpollo_core::audio_haptics`):
//! the settings follow saves mid-stream, and a value the host would not use
//! says so and keeps them off.
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use butterpollo_core::audio_haptics::{Arbiter, Settings, Tap};
use butterpollo_core::config::Config;
use butterpollo_core::controller_audio::PcmTraffic;
use butterpollo_core::haptics;
use butterpollo_core::input::Input;
use butterpollo_core::session::Warnings;

/// The session warning code while a setting is refused.
pub(super) const WARNING: &str = "audio_haptics";

/// One control peer's audio haptics and the configuration version they follow.
/// Dropped with the peer, it stops the audio thread's copy it wanted.
#[derive(Default)]
pub(super) struct PeerHaptics {
    pub(super) arbiter: Arbiter,
    version: Option<u64>,
    /// The tap this peer's controllers are generated from, while they are.
    tap: Option<Arc<Tap>>,
}

impl Drop for PeerHaptics {
    fn drop(&mut self) {
        if let Some(tap) = self.tap.take() {
            self.arbiter.release(&tap);
        }
    }
}

impl PeerHaptics {
    /// A client input event before it is queued: an arrival loses a waveform
    /// capability the stream did not negotiate, and arrivals and the
    /// connected-controller mask choose the controllers generated for.
    pub(super) fn input(&mut self, event: &mut Input, negotiated: bool) {
        if let Input::Arrival { capabilities, .. } = event
            && !negotiated
        {
            *capabilities &= !haptics::CAPABILITY;
        }
        self.arbiter.input(event);
    }

    /// One pass's generated 0x5601 payloads, after the game's own reports of
    /// the same pass reached the arbiter. The controllers generated for keep
    /// their PCM bitrate reserved through quiet stretches, which send nothing.
    pub(super) fn generate(
        &mut self,
        allowed: bool,
        tap: &Arc<Tap>,
        pcm: &mut PcmTraffic,
        now: Instant,
    ) -> Vec<Vec<u8>> {
        if let Some(old) = self.tap.take_if(|old| !Arc::ptr_eq(old, tap)) {
            self.arbiter.release(&old);
        }
        let frames = self.arbiter.generate(allowed, tap, now);
        let generating = self.arbiter.generating();
        if generating == 0 {
            self.tap = None;
        } else {
            self.tap.get_or_insert_with(|| tap.clone());
        }
        for id in (0..16).filter(|id| generating & (1 << id) != 0) {
            pcm.observe(id, haptics::PACKET_TYPE, now);
        }
        frames.iter().filter_map(haptics::Frame::payload).collect()
    }

    /// Reads the settings again when the host configuration `version` moved.
    pub(super) fn follow(
        &mut self,
        version: u64,
        warnings: &Warnings,
        config: impl FnOnce() -> Result<Config>,
    ) {
        if self.version == Some(version) {
            return;
        }
        self.version = Some(version);
        let settings = match config().and_then(|config| Settings::from_config(&config)) {
            Ok(settings) => {
                warnings.clear(WARNING);
                settings
            }
            Err(error) => {
                warnings.set(
                    WARNING,
                    format!(
                        "Haptics from game audio are off ({error:#}). Correct the setting under Controllers."
                    ),
                );
                Settings::default()
            }
        };
        if settings != self.arbiter.settings() {
            tracing::info!(
                enabled = settings.enabled,
                strength = settings.strength,
                curve = ?settings.curve,
                gate_db = settings.gate_db,
                "haptics from game audio"
            );
            self.arbiter.set_settings(settings);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterpollo_core::audio_haptics::{Curve, STRENGTH};

    fn warned(warnings: &Warnings) -> bool {
        warnings.snapshot().iter().any(|w| w.code == WARNING)
    }

    fn config(values: &[(&str, &str)]) -> Result<Config> {
        Ok(Config {
            values: values
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
        })
    }

    #[test]
    fn a_save_applies_mid_stream_and_a_refused_value_turns_them_off_loudly() {
        let warnings = Warnings::default();
        let mut peer = PeerHaptics::default();
        peer.follow(1, &warnings, || {
            config(&[("audio_haptics", "on"), ("audio_haptics_curve", "soft")])
        });
        assert!(peer.arbiter.settings().enabled);
        assert_eq!(peer.arbiter.settings().curve, Curve::Soft);
        // The same version is not read again.
        peer.follow(1, &warnings, || panic!("read again"));
        peer.follow(2, &warnings, || {
            config(&[("audio_haptics", "on"), (STRENGTH, "40")])
        });
        assert_eq!(peer.arbiter.settings().strength, 40);
        assert!(!warned(&warnings));
        peer.follow(3, &warnings, || {
            config(&[("audio_haptics", "on"), (STRENGTH, "900")])
        });
        assert!(!peer.arbiter.settings().enabled);
        assert!(warned(&warnings));
        peer.follow(4, &warnings, || anyhow::bail!("config unreadable"));
        assert!(!peer.arbiter.settings().enabled);
        peer.follow(5, &warnings, || config(&[("audio_haptics", "on")]));
        assert!(peer.arbiter.settings().enabled);
        assert!(!warned(&warnings));
    }

    use super::super::{PadReport, PadSends, pad_reports};
    use std::time::Duration;

    const PASS: Duration = Duration::from_millis(5);

    /// Controller 0 arrives with waveform haptics.
    fn arrival() -> Input {
        Input::Arrival {
            id: 0,
            kind: 2,
            capabilities: haptics::CAPABILITY,
            buttons: 0,
        }
    }

    /// A 5 ms bass packet of game audio, stereo, into the session's tap.
    fn audio(tap: &Tap) {
        let packet: Vec<f32> = (0..240)
            .map(|n| 0.1 * (std::f32::consts::TAU * 60. * n as f32 / 48_000.).sin())
            .flat_map(|x| [x, x])
            .collect();
        tap.push(&packet, 2);
    }

    struct Pass {
        peer: PeerHaptics,
        tap: Arc<Tap>,
        pcm: PcmTraffic,
        now: Instant,
    }

    impl Pass {
        /// A peer with audio haptics on and a negotiated waveform controller 0.
        fn new() -> Self {
            let mut peer = PeerHaptics::default();
            peer.follow(1, &Warnings::default(), || {
                config(&[("audio_haptics", "on")])
            });
            peer.input(&mut arrival(), true);
            let mut pass = Self {
                peer,
                tap: Default::default(),
                pcm: Default::default(),
                now: Instant::now(),
            };
            assert!(pass.generate(false).is_empty());
            assert!(pass.tap.wanted());
            pass
        }

        /// One control pass: the gamepad thread's `reports`, then (with game
        /// audio in the tap when `sound`) generation.
        fn run(&mut self, reports: Vec<PadReport>, sound: bool) -> (PadSends, Vec<Vec<u8>>) {
            let sends = pad_reports(
                reports,
                |_| true,
                true,
                &[0; 16],
                &mut self.pcm,
                &mut self.peer.arbiter,
                &mut Default::default(),
            );
            (sends, self.generate(sound))
        }

        fn generate(&mut self, sound: bool) -> Vec<Vec<u8>> {
            if sound {
                audio(&self.tap);
            }
            self.now += PASS;
            self.peer.generate(true, &self.tap, &mut self.pcm, self.now)
        }

        /// Passes of game audio until generated frames come out.
        fn sounding(&mut self) {
            assert!(
                (0..20).any(|_| !self.run(vec![], true).1.is_empty()),
                "audio in the tap generated no 0x5601 frames"
            );
        }
    }

    #[test]
    fn game_audio_becomes_0x5601_frames_and_keeps_their_bitrate_through_quiet() {
        let mut pass = Pass::new();
        pass.sounding();
        assert_eq!(pass.pcm.streams(pass.now), 1);
        // Quiet for longer than a PCM path's reservation: nothing is sent,
        // and the bitrate stays reserved while the controller is generated for.
        pass.now += Duration::from_secs(6);
        assert!(pass.generate(false).is_empty());
        assert_eq!(pass.pcm.streams(pass.now), 1);
        assert!(pass.tap.wanted());
    }

    #[test]
    fn the_games_own_haptics_or_rumble_stop_generation_in_the_same_pass() {
        let audible = haptics::Frame {
            id: 0,
            sequence: 0,
            created: Instant::now(),
            samples: [4000i16.to_le_bytes(); 2 * haptics::FRAMES].concat(),
        };
        let rumble = PadReport::Feedback {
            id: 0,
            kind: 4,
            data: vec![0, 0x40, 0, 0, 0, 0, 0, 0],
        };
        let trigger_rumble = PadReport::Feedback {
            id: 0,
            kind: 4,
            data: vec![0, 0, 0, 0, 0, 0x40, 0, 0],
        };
        for (name, report, kind) in [
            (
                "waveform",
                PadReport::Haptics(audible),
                haptics::PACKET_TYPE,
            ),
            ("rumble", rumble, 0x010b),
            ("trigger rumble", trigger_rumble, 0x5500),
        ] {
            let mut pass = Pass::new();
            pass.sounding();
            let (sends, generated) = pass.run(vec![report], true);
            assert!(generated.is_empty(), "{name}: generated in the same pass");
            assert!(!pass.tap.wanted(), "{name}: the audio thread still copies");
            let forwarded = sends.waveforms.iter().chain(&sends.messages);
            assert!(
                forwarded.into_iter().any(|(k, _)| *k == kind),
                "{name}: the game's own {kind:#06x} was not forwarded"
            );
        }
    }

    #[test]
    fn a_peer_that_goes_or_loses_its_controller_stops_the_copy() {
        // The control peer is dropped (it left, or its pads were parked).
        let pass = Pass::new();
        let tap = pass.tap.clone();
        drop(pass);
        assert!(!tap.wanted());
        // A peer that never generated leaves another's copy running.
        let pass = Pass::new();
        drop(PeerHaptics::default());
        assert!(pass.tap.wanted());
        // The client's connected-controller mask drops controller 0.
        let mut pass = Pass::new();
        pass.sounding();
        pass.peer.input(
            &mut Input::Controller {
                id: 1,
                active: 0b10,
                buttons: 0,
                left_trigger: 0,
                right_trigger: 0,
                sticks: [0; 4],
            },
            true,
        );
        assert!(pass.run(vec![], true).1.is_empty());
        assert!(!pass.tap.wanted());
        // An arrival the stream did not negotiate waveform haptics for.
        let mut pass = Pass::new();
        pass.peer.input(&mut arrival(), false);
        assert!(pass.run(vec![], true).1.is_empty());
        assert!(!pass.tap.wanted());
    }
}
