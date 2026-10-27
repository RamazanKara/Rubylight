//! What this host offers of the 3.0 protocol extensions
//! (`butterpollo_core::extensions`), one function per offer so each
//! workstream turns on only its own. An offer adds serverinfo fields and
//! DESCRIBE lines that stock clients don't read; an ANNOUNCE that picks an
//! extension that is off is refused.
use butterpollo_core::config::Config;
use butterpollo_core::extensions::Offered;

use crate::state::Shared;

/// The offer for one client: `config` is the host's configuration for
/// serverinfo and the stream's effective configuration for DESCRIBE and
/// ANNOUNCE.
pub fn offered(h: &Shared, config: &Config) -> Offered {
    Offered {
        hdr_formats: hdr_formats(h, config),
        client_fec: client_fec(h, config),
        bandwidth_probe: bandwidth_probe(h, config),
        usb: usb(h, config),
        audio_codecs: audio_codecs(h, config),
        surround_714: surround_714(h, config),
        text_input: text_input(h, config),
        keyboard_layout: keyboard_layout(h, config),
        pen_barrel_roll: pen_barrel_roll(h, config),
        cursor_shape: cursor_shape(h, config),
        pipeline_rates: pipeline_rates(h, config),
    }
}

/// HDR (A): the `rubylight_protocol::hdr` formats this host can encode now.
fn hdr_formats(_h: &Shared, _config: &Config) -> u8 {
    0
}

/// Network (B): client-chosen and statistical FEC.
fn client_fec(_h: &Shared, _config: &Config) -> bool {
    false
}

/// Network (B): `GET /bandwidth-probe`.
fn bandwidth_probe(_h: &Shared, _config: &Config) -> bool {
    false
}

/// USB forwarding (C): the 0x553A channel.
fn usb(_h: &Shared, _config: &Config) -> bool {
    false
}

/// Audio (E): `audio_ext::CODEC_*` bits besides Opus.
fn audio_codecs(_h: &Shared, _config: &Config) -> u8 {
    0
}

/// Audio (E): 7.1.4.
fn surround_714(_h: &Shared, _config: &Config) -> bool {
    false
}

/// Input (F): IME text input.
fn text_input(_h: &Shared, _config: &Config) -> bool {
    false
}

/// Input (F): the client's keyboard layout.
fn keyboard_layout(_h: &Shared, _config: &Config) -> bool {
    false
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
fn usb_feature(_h: &Shared, _config: &Config) -> Vec<&'static str> {
    Vec::new()
}

/// Features (G): `clipboard.image`, while `clipboard_image` is on.
fn clipboard_image_feature(_h: &Shared, _config: &Config) -> Vec<&'static str> {
    Vec::new()
}

/// Features (G): `folders` and `folders.mount`.
fn folders_features(_h: &Shared, _config: &Config) -> Vec<&'static str> {
    Vec::new()
}
