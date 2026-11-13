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
mod tests;
