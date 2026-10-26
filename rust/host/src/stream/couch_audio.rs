//! Night mode and headphone surround on the audio thread (`docs/wake-and-audio-design.md` B.2):
//! the channels a stream captures, and the processing of each packet between `collect` and Opus.

use anyhow::Result;
use butterpollo_core::{
    audio_dsp::{self, Chain, Conditions, Controls, Live, Memory, Report, Settings},
    config::Config,
    session::Warnings,
};

use crate::state::Session;

pub(super) struct CouchAudio<'a> {
    /// The choice the stream started with; the session's live changes go on top.
    start: Settings,
    /// Where the session's live changes are kept for the client's next session of the app.
    memory: Option<(&'a Memory, String, u32)>,
    /// The live changes last kept there.
    remembered: Live,
    conditions: Conditions,
    chain: Chain,
    report: Report,
    /// The endpoint layout the chain was last given.
    source: Option<(usize, u32)>,
}

impl<'a> CouchAudio<'a> {
    /// Resolves the stream's choice from its launch, the client's remembered live choice for
    /// the app and the effective `config`, and picks the capture channels from it.
    pub(super) fn start(
        s: &Session,
        config: &Config,
        memory: &'a Memory,
        game_audio: bool,
        muted: bool,
    ) -> Result<Self> {
        let (client, app) = (s.launch.client.uuid.clone(), s.launch.app_id);
        let remembered = memory.recall(&client, app);
        let mut audio = Self::new(
            &s.audio,
            audio_dsp::resolve(config, &s.launch.options, remembered),
            usize::from(s.config.audio_channels),
            game_audio,
            muted,
        )?;
        audio.memory = Some((memory, client, app));
        Ok(audio)
    }

    fn new(
        controls: &Controls,
        start: Settings,
        client_channels: usize,
        game_audio: bool,
        muted: bool,
    ) -> Result<Self> {
        controls.start(start);
        // A live change made before the audio thread started counts.
        let capture_channels = Conditions::capture_channels(
            controls.settings(start),
            client_channels,
            game_audio,
            muted,
        );
        if capture_channels != client_channels {
            tracing::info!(
                capture_channels,
                client_channels,
                "headphone surround captures 8 channels for a stereo client"
            );
        }
        Ok(Self {
            start,
            memory: None,
            remembered: Live::default(),
            conditions: Conditions {
                client_channels,
                game_audio,
                muted,
                capture_channels,
                source_surround: None,
            },
            chain: Chain::new(capture_channels, client_channels)?,
            report: Report::default(),
            source: None,
        })
    }

    /// The channels the sink, the route and the capture are opened with.
    pub(super) fn capture_channels(&self) -> usize {
        self.chain.capture_channels()
    }

    /// A silent packet of `frames`, in capture channels like the packets `collect` returns:
    /// Opus refuses a packet of the wrong size and the audio thread ends.
    pub(super) fn silence(&self, frames: usize) -> Vec<f32> {
        vec![0.; frames * self.capture_channels()]
    }

    /// The open capture's endpoint channels and speaker mask (`Loopback::source`).
    pub(super) fn set_source(&mut self, source: (usize, u32)) -> Result<()> {
        if self.source != Some(source) {
            self.chain.set_source(source.0, source.1)?;
            self.source = Some(source);
            self.conditions.source_surround = self.chain.source_surround();
        }
        Ok(())
    }

    /// Turns one packet of capture channels into the client's, with the settings as they are
    /// now; inactive reasons become session warnings.
    pub(super) fn process(
        &mut self,
        controls: &Controls,
        warnings: &Warnings,
        packet: &mut Vec<f32>,
    ) {
        let settings = controls.settings(self.start);
        self.report.update(
            controls,
            warnings,
            &self.conditions,
            settings,
            &mut self.chain,
        );
        self.chain.process(packet, settings);
        let live = controls.live();
        if live != self.remembered {
            if let Some((memory, client, app)) = &self.memory {
                memory.remember(client, *app, live);
            }
            self.remembered = live;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterpollo_core::audio_dsp::{Inactive, Key};

    const OFF: Settings = Settings {
        night_mode: false,
        headphone_surround: false,
    };
    const SURROUND: Settings = Settings {
        night_mode: false,
        headphone_surround: true,
    };
    const NIGHT: Settings = Settings {
        night_mode: true,
        headphone_surround: false,
    };
    const SEVEN_ONE: (usize, u32) = (8, 0x63f);

    fn noise(samples: usize, seed: u64) -> Vec<f32> {
        let mut state = seed;
        (0..samples)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((state >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 1.8
            })
            .collect()
    }

    fn bits(samples: &[f32]) -> Vec<u32> {
        samples.iter().map(|v| v.to_bits()).collect()
    }

    fn warning(warnings: &Warnings, key: Key) -> Option<String> {
        warnings
            .snapshot()
            .into_iter()
            .find(|w| w.code == key.warning())
            .map(|w| w.message)
    }

    #[test]
    fn with_both_settings_off_the_opus_input_is_bit_identical_to_the_capture() {
        for (client, game_audio, muted) in [
            (2, false, false),
            (6, false, false),
            (8, false, false),
            (2, true, false),
            (2, false, true),
        ] {
            let controls = Controls::default();
            let warnings = Warnings::default();
            let mut audio = CouchAudio::new(&controls, OFF, client, game_audio, muted).unwrap();
            // The sink, the route, the capture and the silence are opened as before.
            assert_eq!(audio.capture_channels(), client);
            audio.set_source((client, 0)).unwrap();
            let mut input = noise(240 * client * 12, client as u64);
            input[3] = -0.;
            input[5] = 1e-40;
            for packet in input.chunks(240 * client) {
                let mut samples = packet.to_vec();
                audio.process(&controls, &warnings, &mut samples);
                assert_eq!(bits(&samples), bits(packet), "{client} channels");
            }
            assert_eq!(warnings.snapshot().len(), 0);
        }
    }

    #[test]
    fn the_session_choice_picks_the_capture_channels_per_client_and_setting() {
        let f = crate::state::test_support::Fixture::new();
        let config = |text: &str| Config::parse(text).unwrap();
        let client_entry = f.client(u32::MAX);
        for (client, host, option, game_audio, muted, want) in [
            (2, "", None, false, false, 2),
            (2, "audio_headphone_surround = on\n", None, false, false, 8),
            (
                2,
                "audio_headphone_surround = on\n",
                Some("off"),
                false,
                false,
                2,
            ),
            (2, "", Some("on"), false, false, 8),
            (2, "", Some("loud"), false, false, 2),
            (6, "", Some("on"), false, false, 6),
            (8, "", Some("on"), false, false, 8),
            (2, "", Some("on"), true, false, 2),
            (2, "", Some("on"), false, true, 2),
            (2, "audio_night_mode = on\n", None, false, false, 2),
        ] {
            let mut launch = f.launch(
                client_entry.clone(),
                butterpollo_core::session::Role::Stream,
            );
            if let Some(value) = option {
                launch
                    .options
                    .insert(Key::HeadphoneSurround.parameter().into(), value.into());
            }
            let s = crate::state::Session::new(
                launch,
                butterpollo_core::rtsp::Negotiated {
                    audio_channels: client,
                    ..Default::default()
                },
            );
            let memory = Memory::default();
            let audio = CouchAudio::start(&s, &config(host), &memory, game_audio, muted).unwrap();
            assert_eq!(
                audio.capture_channels(),
                want,
                "{client} channels, {host:?}, {option:?}, game {game_audio}, muted {muted}"
            );
        }
        // A live change made before the audio thread starts decides the capture.
        let controls = Controls::default();
        controls.set(Key::HeadphoneSurround, true);
        let audio = CouchAudio::new(&controls, OFF, 2, false, false).unwrap();
        assert_eq!(audio.capture_channels(), 8);
    }

    #[test]
    fn active_surround_is_logged_and_the_opus_layout_stays_stereo() {
        let f = crate::state::test_support::Fixture::new();
        let mut launch = f.launch(f.client(u32::MAX), butterpollo_core::session::Role::Stream);
        launch
            .options
            .insert(Key::HeadphoneSurround.parameter().into(), "on".into());
        let s = crate::state::Session::new(
            launch,
            butterpollo_core::rtsp::Negotiated {
                audio_channels: 2,
                ..Default::default()
            },
        );
        let path = f.host.directory.join("couch-audio.log");
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(std::sync::Mutex::new(std::fs::File::create(&path).unwrap()))
            .finish();
        let memory = Memory::default();
        let warnings = Warnings::default();
        tracing::subscriber::with_default(subscriber, || {
            let config = Config::parse("").unwrap();
            let mut audio = CouchAudio::start(&s, &config, &memory, false, false).unwrap();
            assert_eq!(audio.capture_channels(), 8);
            audio.set_source(SEVEN_ONE).unwrap();
            let mut packet = noise(240 * 8, 21);
            audio.process(&s.audio, &warnings, &mut packet);
            assert_eq!(packet.len(), 480);
        });
        let log = std::fs::read_to_string(path).unwrap();
        assert!(
            log.contains("headphone surround captures 8 channels for a stereo client"),
            "{log}"
        );
        assert_eq!(
            log.matches("headphone surround active (8 to 2)").count(),
            1,
            "{log}"
        );
        // The wire stays stereo Opus.
        let layout = crate::stream::audio_layout(&s, None).unwrap();
        assert_eq!(
            (
                layout.channels,
                layout.streams,
                layout.coupled,
                &layout.mapping[..]
            ),
            (2, 1, 1, &[0, 1][..])
        );
    }

    #[test]
    fn a_live_choice_carries_to_the_clients_next_session_of_the_app() {
        let f = crate::state::test_support::Fixture::new();
        let config = Config::parse("").unwrap();
        let memory = Memory::default();
        let warnings = Warnings::default();
        let session = |client: butterpollo_core::state::Client, app: Option<u32>| {
            let mut launch = f.launch(client, butterpollo_core::session::Role::Stream);
            if let Some(app) = app {
                launch.app_id = app;
            }
            crate::state::Session::new(
                launch,
                butterpollo_core::rtsp::Negotiated {
                    audio_channels: 2,
                    ..Default::default()
                },
            )
        };
        let client = f.client(u32::MAX);
        let mut other = client.clone();
        other.uuid = "another-device".into();

        // Turned on live with stereo capture: it applies from the next stream.
        let first = session(client.clone(), None);
        let mut audio = CouchAudio::start(&first, &config, &memory, false, false).unwrap();
        assert_eq!(audio.capture_channels(), 2);
        audio.set_source((2, 3)).unwrap();
        first.audio.set(Key::HeadphoneSurround, true);
        audio.process(&first.audio, &warnings, &mut vec![0.; 480]);
        drop(audio);

        // The same client and app (a new launch or a resume): surround on, 8 channels.
        let next = session(client.clone(), None);
        let audio = CouchAudio::start(&next, &config, &memory, false, false).unwrap();
        assert_eq!(audio.capture_channels(), 8);
        assert!(next.audio.settings(OFF).headphone_surround);
        // Another client, or another app of the same client, starts from its config.
        let elsewhere = session(other, None);
        let audio = CouchAudio::start(&elsewhere, &config, &memory, false, false).unwrap();
        assert_eq!(audio.capture_channels(), 2);
        let app = next.launch.app_id.wrapping_add(1);
        let elsewhere = session(client, Some(app));
        let audio = CouchAudio::start(&elsewhere, &config, &memory, false, false).unwrap();
        assert_eq!(audio.capture_channels(), 2);
    }

    #[test]
    fn a_live_toggle_takes_effect_on_the_next_packet() {
        // Night mode on a stereo stream: off, then on for the next packet, then off again.
        let controls = Controls::default();
        let warnings = Warnings::default();
        let mut audio = CouchAudio::new(&controls, OFF, 2, false, false).unwrap();
        audio.set_source((2, 3)).unwrap();
        let loud = noise(480, 1);
        let mut first = loud.clone();
        audio.process(&controls, &warnings, &mut first);
        assert_eq!(bits(&first), bits(&loud));
        controls.set(Key::NightMode, true);
        let mut second = loud.clone();
        audio.process(&controls, &warnings, &mut second);
        assert_ne!(bits(&second), bits(&loud));
        assert!(second.iter().all(|v| v.abs() <= 0.98));

        // Headphone surround started on with 7.1 capture: the HRIRs render, a live off turns
        // the next packet to the downmix and a live on back to the HRIRs.
        let controls = Controls::default();
        let mut audio = CouchAudio::new(&controls, SURROUND, 2, false, false).unwrap();
        assert_eq!(audio.capture_channels(), 8);
        audio.set_source(SEVEN_ONE).unwrap();
        let mut reference = Chain::new(8, 2).unwrap();
        reference.set_source(SEVEN_ONE.0, SEVEN_ONE.1).unwrap();
        let input = noise(240 * 8 * 6, 3);
        for (i, packet) in input.chunks(240 * 8).enumerate() {
            let on = !(2..4).contains(&i);
            controls.set(Key::HeadphoneSurround, on);
            let settings = Settings {
                headphone_surround: on,
                ..OFF
            };
            assert_eq!(audio.chain.rendering(settings), on, "packet {i}");
            let mut samples = packet.to_vec();
            audio.process(&controls, &warnings, &mut samples);
            let mut want = packet.to_vec();
            reference.process(&mut want, settings);
            assert_eq!(samples.len(), 480);
            assert_eq!(bits(&samples), bits(&want), "packet {i}");
        }
        assert_eq!(controls.reason(Key::HeadphoneSurround), None);
        assert_eq!(warning(&warnings, Key::HeadphoneSurround), None);
    }

    #[test]
    fn turned_on_mid_stream_without_surround_capture_it_applies_next_stream() {
        let controls = Controls::default();
        let warnings = Warnings::default();
        let mut audio = CouchAudio::new(&controls, OFF, 2, false, false).unwrap();
        assert_eq!(audio.capture_channels(), 2);
        audio.set_source((2, 3)).unwrap();
        let input = noise(480, 7);
        let mut samples = input.clone();
        audio.process(&controls, &warnings, &mut samples);
        assert_eq!(warning(&warnings, Key::HeadphoneSurround), None);
        controls.set(Key::HeadphoneSurround, true);
        let mut samples = input.clone();
        audio.process(&controls, &warnings, &mut samples);
        // The sink and the capture stay stereo: the packet passes as it is.
        assert_eq!(bits(&samples), bits(&input));
        assert_eq!(
            controls.reason(Key::HeadphoneSurround),
            Some(Inactive::NextStream)
        );
        assert_eq!(
            warning(&warnings, Key::HeadphoneSurround).as_deref(),
            Some(Inactive::NextStream.message(Key::HeadphoneSurround))
        );
        // Turned off again, the warning goes.
        controls.set(Key::HeadphoneSurround, false);
        audio.process(&controls, &warnings, &mut input.clone());
        assert_eq!(warning(&warnings, Key::HeadphoneSurround), None);
    }

    #[test]
    fn each_inactive_reason_becomes_a_session_warning() {
        let surround = Key::HeadphoneSurround;
        for (start, client, game_audio, muted, source, key, reason) in [
            (
                SURROUND,
                6,
                false,
                false,
                (6, 0x3f),
                surround,
                Inactive::ClientSurround,
            ),
            (
                SURROUND,
                2,
                true,
                false,
                (2, 3),
                surround,
                Inactive::GameOnlyAudio,
            ),
            (
                SURROUND,
                2,
                false,
                false,
                (2, 3),
                surround,
                Inactive::StereoEndpoint,
            ),
            (SURROUND, 2, false, true, (2, 3), surround, Inactive::Muted),
            (
                NIGHT,
                2,
                false,
                true,
                (2, 3),
                Key::NightMode,
                Inactive::Muted,
            ),
        ] {
            let controls = Controls::default();
            let warnings = Warnings::default();
            let mut audio = CouchAudio::new(&controls, start, client, game_audio, muted).unwrap();
            audio.set_source(source).unwrap();
            let channels = audio.capture_channels();
            let mut silence = vec![0.; 240 * channels];
            audio.process(&controls, &warnings, &mut silence);
            assert_eq!(silence, vec![0.; 240 * client], "{reason:?}");
            assert_eq!(controls.reason(key), Some(reason));
            assert_eq!(
                warning(&warnings, key).as_deref(),
                Some(reason.message(key)),
                "{reason:?}"
            );
        }
    }

    #[test]
    fn nan_or_infinite_samples_raise_one_session_warning_per_stream() {
        let bad = |warnings: &Warnings| {
            warnings
                .snapshot()
                .into_iter()
                .filter(|w| w.code == audio_dsp::BAD_SAMPLES_WARNING)
                .count()
        };
        for (start, client, source) in [(NIGHT, 2, (2, 3)), (SURROUND, 2, SEVEN_ONE)] {
            let controls = Controls::default();
            let warnings = Warnings::default();
            let mut audio = CouchAudio::new(&controls, start, client, false, false).unwrap();
            audio.set_source(source).unwrap();
            let channels = audio.capture_channels();
            let mut packet = noise(240 * channels, 11);
            audio.process(&controls, &warnings, &mut packet);
            assert_eq!(bad(&warnings), 0);
            let mut packet = noise(240 * channels, 12);
            packet[1] = f32::NAN;
            packet[7] = f32::NEG_INFINITY;
            audio.process(&controls, &warnings, &mut packet);
            assert!(packet.iter().all(|v| v.is_finite()));
            // The packet after the bad one reports it.
            audio.process(&controls, &warnings, &mut noise(240 * channels, 13));
            assert_eq!(bad(&warnings), 1, "{start:?}");
            // Once per stream: more bad samples don't raise it again.
            warnings.clear(audio_dsp::BAD_SAMPLES_WARNING);
            for _ in 0..2 {
                audio.process(&controls, &warnings, &mut vec![f32::NAN; 240 * channels]);
            }
            assert_eq!(bad(&warnings), 0, "{start:?}");
        }
    }

    #[test]
    fn the_silence_packet_is_sized_for_the_capture_channels() {
        for (start, client, capture) in [
            (SURROUND, 2, 8),
            (OFF, 2, 2),
            (SURROUND, 6, 6),
            (NIGHT, 8, 8),
        ] {
            let controls = Controls::default();
            let warnings = Warnings::default();
            let mut audio = CouchAudio::new(&controls, start, client, false, false).unwrap();
            audio.set_source((capture, 0x63f)).unwrap();
            for frames in [240, 480, 960] {
                let mut silence = audio.silence(frames);
                assert_eq!(silence.len(), frames * capture, "{start:?} {client}");
                // The chain turns it into a packet the client's Opus layout takes.
                audio.process(&controls, &warnings, &mut silence);
                assert_eq!(silence, vec![0.; frames * client], "{start:?} {client}");
            }
        }
    }
}
