//! What this host offers of the 3.0 protocol extensions
//! (`butterpollo_core::extensions`), one function per offer so each
//! workstream turns on only its own. An offer adds serverinfo fields and
//! DESCRIBE lines that stock clients ignore, and the messages behind it go
//! only to a client that asks; an ANNOUNCE that picks an extension that is
//! not offered is refused.
use butterpollo_core::config::Config;
use butterpollo_core::extensions::Offered;

use crate::state::Shared;

/// The serverinfo offer for one client, `config` being the host's
/// configuration. Its AC3 and E-AC3 bits are FFmpeg's stereo probes as last
/// run: [`probe_serverinfo_audio`] runs them first.
pub fn offered(h: &Shared, config: &Config) -> Offered {
    let dolby = config
        .audio_passthrough()
        .then(|| {
            let exe = std::env::current_exe().ok()?;
            crate::stream::audio_encoder::dolby_codecs_probed(exe.parent()?, 2)
        })
        .flatten();
    offer(h, config, dolby.unwrap_or_default())
}

/// Runs the FFmpeg probes serverinfo's [`offered`] reads, off the async task,
/// when none is kept or a failure's retry is due.
pub async fn probe_serverinfo_audio(config: &Config) -> anyhow::Result<()> {
    if !config.audio_passthrough() {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let directory = exe.parent().unwrap().to_owned();
    if crate::stream::audio_encoder::dolby_codecs_probed(&directory, 2).is_none() {
        tokio::task::spawn_blocking(move || {
            crate::stream::audio_encoder::dolby_codecs(&directory, 2)
        })
        .await?;
    }
    Ok(())
}

/// The offer for a stream's DESCRIBE and ANNOUNCE, `config` being its
/// effective configuration and `dolby` the AC3 and E-AC3 bits FFmpeg opens
/// for the launch's channels (`audio_encoder::dolby_codecs`).
pub fn stream_offered(h: &Shared, config: &Config, dolby: u8) -> Offered {
    offer(h, config, dolby)
}

fn offer(h: &Shared, config: &Config, dolby: u8) -> Offered {
    Offered {
        hdr_formats: hdr_formats(h, config),
        client_fec: client_fec(h, config),
        bandwidth_probe: bandwidth_probe(h, config),
        usb: usb(h, config),
        audio_codecs: audio_codecs(h, config, dolby),
        surround_714: surround_714(h, config),
        text_input: text_input(h, config),
        keyboard_layout: keyboard_layout(h, config),
        pen_barrel_roll: pen_barrel_roll(h, config),
        cursor_shape: cursor_shape(h, config),
        pipeline_rates: pipeline_rates(h, config),
    }
}

/// AMF signals transfer 18 (`amf::transfer_characteristic`) and the colour
/// shader encodes HLG (`hdr == 2`) for HLG and Dolby Vision 8.4 streams. Turning
/// this off stops offering both, so no client can pick them.
const ENCODER_WRITES_HLG: bool = true;

/// HDR (A): the `rubylight_protocol::hdr` formats this host can encode now:
/// HLG on HEVC or AV1 Main10 while `hdr_hlg` is on, and Dolby Vision 8.1 (8.4
/// with HLG) beside PQ on HEVC Main10 while `hdr_dolby_vision` is on
/// (`extensions::hdr_offer`).
fn hdr_formats(h: &Shared, config: &Config) -> u8 {
    butterpollo_core::extensions::hdr_offer(
        h.codecs.load(std::sync::atomic::Ordering::Acquire),
        ENCODER_WRITES_HLG && config.hdr_hlg(),
        config.hdr_dolby_vision(),
    )
}

/// Network (B): client-chosen and statistical FEC (`rlFecPercentage` and
/// 0x5539 reports). Always offered: a client that sends neither keeps the
/// host's `fec_percentage` and `adaptive_fec`, and `fec_max_percentage` caps
/// what a client may ask for.
fn client_fec(_h: &Shared, _config: &Config) -> bool {
    true
}

/// Network (B): `GET /bandwidth-probe`, while `bandwidth_probe` is on.
pub fn bandwidth_probe(_h: &Shared, config: &Config) -> bool {
    config.bandwidth_probe()
}

/// USB forwarding (C): the 0x553A channel, while `usb_forwarding` is on and the USB/IP driver
/// is ready.
fn usb(h: &Shared, config: &Config) -> bool {
    config.usb_forwarding() && h.usb.ready()
}

/// Audio (E): `audio_ext::CODEC_*` bits besides Opus, while `audio_passthrough` is on: PCM,
/// and the AC3 and E-AC3 bits `dolby` whose FFmpeg encoder opens.
fn audio_codecs(_h: &Shared, config: &Config, dolby: u8) -> u8 {
    use butterpollo_core::audio_ext::{CODEC_AC3, CODEC_EAC3, CODEC_PCM};
    if !config.audio_passthrough() {
        return 0;
    }
    CODEC_PCM | dolby & (CODEC_AC3 | CODEC_EAC3)
}

/// Audio (E): 7.1.4 Opus, while `audio_surround_714` is on. The capture
/// takes a 7.1.4 endpoint's heights, and leaves them silent on any other.
fn surround_714(_h: &Shared, config: &Config) -> bool {
    config.audio_surround_714()
}

/// Input (F): the device's keyboard follows the host's text fields, while the
/// keyboard and `keyboard_follows_focus` are on.
fn text_input(_h: &Shared, config: &Config) -> bool {
    config.boolean("keyboard", true) && config.keyboard_follows_focus()
}

/// Input (F): the client's keyboard layout, while the keyboard is on and
/// `keyboard_layout` leaves the layout to the device.
fn keyboard_layout(_h: &Shared, config: &Config) -> bool {
    config.boolean("keyboard", true)
        && config.keyboard_layout() == butterpollo_core::config::KeyboardLayoutSetting::Client
}

/// Input (F): pen barrel roll.
fn pen_barrel_roll(_h: &Shared, _config: &Config) -> bool {
    false
}

/// Input (F): cursor shapes for a client-drawn cursor.
fn cursor_shape(_h: &Shared, _config: &Config) -> bool {
    false
}

/// Diagnostics (B): pipeline frame rates. Every stream counts them, off the
/// hot path, and reports only to a client that asks.
fn pipeline_rates(_h: &Shared, _config: &Config) -> bool {
    true
}

/// The native API features of the 3.0 extensions, appended to
/// `GET /capabilities`. The caller holds the host configuration's read lock:
/// don't take `h.config` again.
pub fn native_features(h: &Shared, config: &Config) -> Vec<&'static str> {
    [
        usb_feature(h, config),
        clipboard_image_feature(h, config),
        folders_features(h, config),
    ]
    .concat()
}

/// USB forwarding (C): `usb`, while `usb_forwarding` is on and the USB/IP
/// driver is ready.
fn usb_feature(h: &Shared, config: &Config) -> Vec<&'static str> {
    if usb(h, config) {
        vec![butterpollo_core::native_api::feature::USB]
    } else {
        Vec::new()
    }
}

/// Features (G): `clipboard.image`, while `clipboard_image` is on.
fn clipboard_image_feature(_h: &Shared, _config: &Config) -> Vec<&'static str> {
    Vec::new()
}

/// Features (G): `folders` and `folders.mount`.
fn folders_features(_h: &Shared, _config: &Config) -> Vec<&'static str> {
    Vec::new()
}
