//! Sound quality of the host half of a client microphone on the real
//! devices: decoded microphone audio (raw 48 kHz mono f32) is queued as the
//! host queues it, 20 ms a packet in real time, played into Steam Streaming
//! Microphone, and recorded back from "Microphone (Steam Streaming
//! Microphone)" as a game or chat app would. The recording is written as raw
//! f32 (first channel) at the endpoint's rate, which is printed.
//!
//! With MIC_OPUS=DIR (a folder with libopus-0.dll) the audio goes through
//! Opus as a client sends it (64 kb/s voice with in-band FEC) and the host's
//! decoder, which also conceals dry spells as the host does
//! (MIC_NO_CONCEAL=1 plays silence instead). MIC_BURSTS=SEED delivers the
//! packets as a jittery network does: up to 3 ms late, and now and then held
//! for 40-80 ms and released at once.
//!
//! usage: mic_quality IN.f32 OUT.f32
#![allow(clippy::undocumented_unsafe_blocks)]
use anyhow::{Context, Result, bail};
use butterpollo_core::mic::Playout;
use butterpollo_windows::{
    audio::OpusDecoder,
    audio_route,
    capture::{ComGuard, Priority},
    mic::Render,
    timing::Timer,
};
use std::{
    ffi::c_void,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::PROPERTYKEY,
        Media::Audio::*,
        System::Com::{StructuredStorage::*, *},
    },
    core::GUID,
};

const FRIENDLY: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};
const PACKET: usize = 960;

type EncoderCreate = unsafe extern "C" fn(i32, i32, i32, *mut i32) -> *mut c_void;
type EncoderCtl = unsafe extern "C" fn(*mut c_void, i32, ...) -> i32;
type Encode = unsafe extern "C" fn(*mut c_void, *const f32, i32, *mut u8, i32) -> i32;

/// Nightjar's microphone encoder (client/app/src/main/jni/moonlight-core/android_microphone.c).
struct ClientEncoder {
    state: *mut c_void,
    encode: Encode,
    _library: libloading::Library,
}
impl ClientEncoder {
    fn new(directory: &std::path::Path) -> Result<Self> {
        unsafe {
            let library = libloading::Library::new(directory.join("libopus-0.dll"))?;
            let create = *library.get::<EncoderCreate>(b"opus_encoder_create\0")?;
            let ctl = *library.get::<EncoderCtl>(b"opus_encoder_ctl\0")?;
            let encode = *library.get::<Encode>(b"opus_encode_float\0")?;
            let mut error = 0;
            // OPUS_APPLICATION_VOIP, then bitrate, voice signal, FEC and expected loss.
            let state = create(48000, 1, 2048, &mut error);
            if state.is_null() || error != 0 {
                bail!("Opus encoder creation failed: {error}");
            }
            for (request, value) in [(4002, 64_000), (4024, 3001), (4012, 1), (4014, 10)] {
                if ctl(state, request, value) != 0 {
                    bail!("Opus control {request} failed");
                }
            }
            Ok(Self {
                state,
                encode,
                _library: library,
            })
        }
    }
    fn encode(&mut self, pcm: &[f32]) -> Result<Vec<u8>> {
        let mut out = vec![0; 1500];
        let n = unsafe {
            (self.encode)(
                self.state,
                pcm.as_ptr(),
                pcm.len() as i32,
                out.as_mut_ptr(),
                1500,
            )
        };
        if n <= 0 {
            bail!("Opus encoding failed: {n}");
        }
        out.truncate(n as usize);
        Ok(out)
    }
}

/// When each of `count` 20 ms packets arrives, in ms from the start: on
/// time, or (with a seed) up to 3 ms late and about once in 100 packets held
/// for 40-80 ms, released together with the ones due meanwhile.
fn schedule(count: usize, seed: Option<u32>) -> Vec<f64> {
    let Some(mut seed) = seed else {
        return (0..count).map(|n| 20. * n as f64).collect();
    };
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let mut held_until = 0.;
    (0..count)
        .map(|n| {
            let due = 20. * n as f64;
            if due >= held_until && next() % 100 == 0 {
                held_until = due + 40. + f64::from(next() % 41);
            }
            due.max(held_until) + f64::from(next() % 31) / 10.
        })
        .collect()
}

fn capture_endpoint() -> Result<IMMDevice> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let list = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
        for index in 0..list.GetCount()? {
            let device = list.Item(index)?;
            let store = device.OpenPropertyStore(STGM_READ)?;
            let mut value = store.GetValue(&FRIENDLY)?;
            let name = PropVariantToStringAlloc(&value)
                .ok()
                .and_then(|text| {
                    let name = text.to_string().ok();
                    CoTaskMemFree(Some(text.0.cast()));
                    name
                })
                .unwrap_or_default();
            let _ = PropVariantClear(&mut value);
            if name
                .to_ascii_lowercase()
                .contains("steam streaming microphone")
            {
                return Ok(device);
            }
        }
    }
    bail!("Microphone (Steam Streaming Microphone) is not present")
}

/// Records the Steam microphone's first channel; returns (rate, samples).
fn record(stop: &AtomicBool, ready: &AtomicBool) -> Result<(u32, Vec<f32>)> {
    let _com = ComGuard::new()?;
    let device = capture_endpoint()?;
    let _priority = Priority::new();
    let mut out = Vec::new();
    unsafe {
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let format = client.GetMixFormat()?;
        let f = *format;
        let float = f.wFormatTag == 3
            || (f.wFormatTag == 65534
                && f.cbSize >= 22
                && (*(format as *const WAVEFORMATEXTENSIBLE)).SubFormat.data1 == 3);
        let init = client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 200_000, 0, format, None);
        CoTaskMemFree(Some(format as *const c_void));
        init?;
        let bits = f.wBitsPerSample;
        if !(float && bits == 32) && bits != 16 {
            bail!("unsupported capture format: {bits} bits");
        }
        let channels = usize::from(f.nChannels);
        let rate = f.nSamplesPerSec;
        eprintln!("capture mix format: {rate} Hz, {channels} ch, {bits} bit, float={float}");
        let capture: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        ready.store(true, Ordering::Release);
        while !stop.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(5));
            loop {
                let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0, 0);
                if capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .is_err()
                    || frames == 0
                {
                    break;
                }
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                for i in 0..frames as usize {
                    out.push(if silent {
                        0.
                    } else if float {
                        *(data as *const f32).add(i * channels)
                    } else {
                        f32::from(*(data as *const i16).add(i * channels)) / 32768.
                    });
                }
                capture.ReleaseBuffer(frames)?;
            }
        }
        let _ = client.Stop();
        Ok((rate, out))
    }
}

fn main() -> Result<()> {
    let _com = ComGuard::new()?;
    let mut args = std::env::args().skip(1);
    let input = args.next().context("usage: mic_quality IN.f32 OUT.f32")?;
    let output = args.next().context("usage: mic_quality IN.f32 OUT.f32")?;
    let bytes = std::fs::read(&input)?;
    let pcm: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&b| f32::from_le_bytes(b))
        .collect();
    let id = audio_route::steam_microphone(&audio_route::endpoints()?)
        .map(|e| e.id.clone())
        .context("Steam Streaming Microphone is not installed")?;
    // As the host does before it plays (MIC_NO_MATCH=1 measures without).
    if std::env::var_os("MIC_NO_MATCH").is_none() {
        eprintln!(
            "formats: {:?}",
            audio_route::match_steam_microphone_format(&id)?
        );
    }
    // As the host does before it plays (MIC_NO_GAIN_CAP=1 measures without).
    if std::env::var_os("MIC_NO_GAIN_CAP").is_none() {
        eprintln!(
            "recording level lowered from (dB): {:?}",
            audio_route::SteamMicrophoneGain::open()?.limit()?
        );
    }
    let stop = Arc::new(AtomicBool::new(false));
    let ready = Arc::new(AtomicBool::new(false));
    let recorder = {
        let (stop, ready) = (stop.clone(), ready.clone());
        std::thread::spawn(move || record(&stop, &ready))
    };
    let playout = Arc::new(Mutex::new(Playout::default()));
    let opus = std::env::var_os("MIC_OPUS").map(std::path::PathBuf::from);
    let mut encoder = opus.as_deref().map(ClientEncoder::new).transpose()?;
    // Lock order as in the host: the decoder before the playout.
    let decoder = Arc::new(Mutex::new(
        opus.as_deref().map(OpusDecoder::new).transpose()?,
    ));
    let conceal = std::env::var_os("MIC_NO_CONCEAL").is_none();
    let renderer = {
        let (playout, decoder) = (playout.clone(), decoder.clone());
        let stop = stop.clone();
        std::thread::spawn(move || -> Result<()> {
            let _com = ComGuard::new()?;
            let _priority = Priority::input();
            let mut render = Render::open(&id)?;
            while !stop.load(Ordering::Acquire) {
                render.wait(Duration::from_millis(50));
                let mut decoder = decoder.lock().unwrap();
                let mut playout = playout.lock().unwrap();
                render.fill_concealing(&mut playout, &mut |out| {
                    decoder
                        .as_mut()
                        .filter(|_| conceal)
                        .and_then(|decoder| decoder.decode(None, false, out).ok())
                        .unwrap_or(0)
                })?;
            }
            Ok(())
        })
    };
    while !ready.load(Ordering::Acquire) {
        if recorder.is_finished() {
            recorder.join().unwrap()?;
            bail!("recorder ended");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(300));
    let packets: Vec<&[f32]> = pcm
        .as_chunks::<PACKET>()
        .0
        .iter()
        .map(|packet| &packet[..])
        .collect();
    let encoded = match &mut encoder {
        Some(encoder) => Some(
            packets
                .iter()
                .map(|packet| encoder.encode(packet))
                .collect::<Result<Vec<_>>>()?,
        ),
        None => None,
    };
    let seed = std::env::var("MIC_BURSTS")
        .ok()
        .map(|s| s.parse())
        .transpose()?;
    let times = schedule(packets.len(), seed);
    let timer = Timer::new()?;
    let start = Instant::now();
    let mut decoded = Vec::new();
    for (n, packet) in packets.iter().enumerate() {
        timer.until(start + Duration::from_secs_f64(times[n] / 1000.));
        let mut decoder = decoder.lock().unwrap();
        let samples = match (&encoded, decoder.as_mut()) {
            (Some(encoded), Some(decoder)) => {
                decoded.clear();
                decoder.decode(Some(&encoded[n]), false, &mut decoded)?;
                &decoded[..]
            }
            _ => packet,
        };
        playout.lock().unwrap().push_batch(samples, samples.len());
    }
    std::thread::sleep(Duration::from_millis(500));
    stop.store(true, Ordering::Release);
    renderer.join().unwrap()?;
    let (rate, recorded) = recorder.join().unwrap()?;
    let playout = playout.lock().unwrap();
    let mut out = Vec::with_capacity(recorded.len() * 4);
    for sample in &recorded {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(&output, out)?;
    println!(
        "{}",
        serde_json::json!({
            "rate": rate,
            "recorded": recorded.len(),
            "trimmed_ms": playout.trimmed * 1000 / 48000,
            "underruns": playout.underruns,
            "underrun_ms": playout.underrun_samples * 1000 / 48000,
        })
    );
    Ok(())
}
