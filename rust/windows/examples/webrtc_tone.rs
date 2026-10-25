//! Play one tone per channel to the virtual sink the WebRTC test host captures, for the 5.1 and
//! 7.1 legs of the end-to-end harness. Channel `c` of the endpoint's mix format carries
//! `500 + 200 * c` Hz, and the client checks each decoded channel for the same table.
//!
//!   webrtc_tone --format [ENDPOINT_ID]     the sink's mix format as JSON
//!   webrtc_tone --set-channels N           give the sink an N channel format; prints the old one
//!   webrtc_tone --restore HEX              put that format back
//!   webrtc_tone SECONDS [ENDPOINT_ID]      play (a quiet level, only to the Steam sink)
//!
//! It refuses every endpoint but the Steam Streaming Speakers sink: the user's own devices are
//! never rendered to.
use anyhow::{Context, Result, bail};
use butterpollo_windows::{
    audio_route,
    capture::{ComGuard, Priority},
    timing::Timer,
};
use std::{
    ffi::c_void,
    time::{Duration, Instant},
};
use windows::{
    Win32::{Media::Audio::*, System::Com::*},
    core::PCWSTR,
};

struct Render(IAudioClient);
impl Drop for Render {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.Stop();
        }
    }
}

fn sink_id(selected: Option<String>) -> Result<String> {
    Ok(audio_route::endpoints()?
        .into_iter()
        .find(|e| e.virtual_sink && selected.as_ref().is_none_or(|id| id == &e.id))
        .context("Steam Streaming Speakers is required for this isolated audio test")?
        .id)
}

fn tone_hz(channel: usize) -> f64 {
    500. + 200. * channel as f64
}

fn main() -> Result<()> {
    let _com = ComGuard::new()?;
    let mut args = std::env::args().skip(1);
    let first = args.next().context(
        "usage: webrtc_tone (--format | --set-channels N | --restore HEX | SECONDS) [ENDPOINT_ID]",
    )?;
    // The surround legs need a 6 or 8 channel sink format and put the old one back; no default
    // device is touched.
    if first == "--set-channels" || first == "--restore" {
        let value = args.next().context("a value is required")?;
        let id = sink_id(args.next())?;
        if first == "--restore" {
            let bytes = (0..value.len() / 2)
                .map(|i| u8::from_str_radix(&value[2 * i..2 * i + 2], 16))
                .collect::<Result<Vec<u8>, _>>()?;
            audio_route::restore_sink_format(&id, &bytes)?;
            println!("{{\"restored\":true}}");
        } else {
            let before = audio_route::set_sink_channels(&id, value.parse()?)?;
            let hex: String = before.iter().map(|b| format!("{b:02x}")).collect();
            println!("{}", serde_json::json!({"before": hex}));
        }
        return Ok(());
    }
    let selected = args.next();
    let endpoint = audio_route::endpoints()?
        .into_iter()
        .find(|e| e.virtual_sink && selected.as_ref().is_none_or(|id| id == &e.id))
        .context("Steam Streaming Speakers is required for this isolated audio test")?;
    let id: Vec<u16> = endpoint.id.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDevice(PCWSTR(id.as_ptr()))?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let format = client.GetMixFormat()?;
        let f = *format;
        let float = f.wFormatTag == 3
            || (f.wFormatTag == 65534
                && f.cbSize >= 22
                && (*(format as *const WAVEFORMATEXTENSIBLE)).SubFormat.data1 == 3);
        let mask = if f.wFormatTag == 65534 && f.cbSize >= 22 {
            (*(format as *const WAVEFORMATEXTENSIBLE)).dwChannelMask
        } else {
            0
        };
        let channels = f.nChannels as usize;
        let rate = f.nSamplesPerSec;
        let bits = f.wBitsPerSample;
        if first == "--format" {
            println!(
                "{}",
                serde_json::json!({"endpoint":endpoint.name,"channels":channels,"mask":mask,"sample_rate":rate,"bits":bits,"float":float})
            );
            CoTaskMemFree(Some(format as *const c_void));
            return Ok(());
        }
        let seconds: u64 = first.parse()?;
        if !(1..=3600).contains(&seconds) {
            bail!("usage: webrtc_tone (--format | SECONDS) [ENDPOINT_ID]");
        }
        let init = client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 1_000_000, 0, format, None);
        CoTaskMemFree(Some(format as *const c_void));
        init?;
        if !float || bits != 32 {
            bail!("the test renderer requires a float WASAPI mix format");
        }
        let renderer: IAudioRenderClient = client.GetService()?;
        let buffer_frames = client.GetBufferSize()?;
        let render = Render(client);
        let timer = Timer::new()?;
        let _priority = Priority::new();
        let mut position = 0u64;
        render.0.Start()?;
        let end = Instant::now() + Duration::from_secs(seconds);
        while Instant::now() < end {
            let available = buffer_frames - render.0.GetCurrentPadding()?;
            if available > 0 {
                let data = renderer.GetBuffer(available)?;
                let output = std::slice::from_raw_parts_mut(
                    data.cast::<f32>(),
                    available as usize * channels,
                );
                for frame in output.chunks_exact_mut(channels) {
                    for (channel, sample) in frame.iter_mut().enumerate() {
                        let t = position as f64 / f64::from(rate);
                        *sample =
                            (0.1 * (std::f64::consts::TAU * tone_hz(channel) * t).sin()) as f32;
                    }
                    position += 1;
                }
                renderer.ReleaseBuffer(available, 0)?;
            }
            timer.until(Instant::now() + Duration::from_millis(2));
        }
        println!(
            "{}",
            serde_json::json!({"channels":channels,"rendered_frames":position})
        );
    }
    Ok(())
}
