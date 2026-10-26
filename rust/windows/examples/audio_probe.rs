//! Render a quiet test tone to an explicit endpoint and verify real WASAPI samples.
use anyhow::{Context, Result, bail};
use butterpollo_windows::{
    audio::Loopback,
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
/// What the probe plays, from `BUTTERPOLLO_TEST_TONE_PATTERN`.
enum Pattern {
    /// A quiet 960 Hz tone on every channel, without a break.
    Tone,
    /// A 300 ms noise burst on one channel at a time, one a second, in the endpoint's channel
    /// order. Each burst logs when it plays on the QueryPerformanceCounter clock.
    Sweep,
    /// A 1 kHz tone on every channel, 3 s at -6 dBFS then 3 s at -40 dBFS, repeating.
    Levels,
}
impl Pattern {
    fn from_env() -> Result<Self> {
        Ok(
            match std::env::var("BUTTERPOLLO_TEST_TONE_PATTERN").as_deref() {
                Err(_) | Ok("tone") => Self::Tone,
                Ok("sweep") => Self::Sweep,
                Ok("levels") => Self::Levels,
                Ok(other) => bail!(
                    "unknown BUTTERPOLLO_TEST_TONE_PATTERN {other}; use tone, sweep or levels"
                ),
            },
        )
    }
    /// Fills one frame, `position` frames into the pattern.
    fn fill(&self, frame: &mut [f32], position: u64, sample_rate: u32) {
        let rate = u64::from(sample_rate);
        let phase = (position % rate) as f64 / rate as f64;
        match self {
            Self::Tone => frame.fill((0.05 * (std::f64::consts::TAU * 960. * phase).sin()) as f32),
            Self::Sweep => {
                frame.fill(0.);
                if position % rate < rate * 3 / 10 {
                    // Uniform noise of 0.3 peak, the same burst each second.
                    let state = (position % rate)
                        .wrapping_add(1)
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    let state = (state ^ (state >> 29)).wrapping_mul(0xbf58476d1ce4e5b9);
                    let noise = ((state >> 40) as f64 / (1u64 << 24) as f64 - 0.5) * 0.6;
                    frame[((position / rate) % frame.len() as u64) as usize] = noise as f32;
                }
            }
            Self::Levels => {
                let level = if (position / rate / 3).is_multiple_of(2) {
                    0.5
                } else {
                    0.01
                };
                frame.fill((level * (std::f64::consts::TAU * 1000. * phase).sin()) as f32);
            }
        }
    }
}
struct Output {
    render: Render,
    renderer: IAudioRenderClient,
    channels: usize,
    sample_rate: u32,
    buffer_frames: u32,
}
/// A started shared-mode float render stream on the endpoint `id`, in its mix format.
fn open(id: &[u16]) -> Result<Output> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDevice(PCWSTR(id.as_ptr()))?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let format = client.GetMixFormat()?;
        let f = *format;
        let extensible = f.wFormatTag == 65534 && f.cbSize >= 22;
        let float = f.wFormatTag == 3
            || (extensible && (*(format as *const WAVEFORMATEXTENSIBLE)).SubFormat.data1 == 3);
        let mask = if extensible {
            (*(format as *const WAVEFORMATEXTENSIBLE)).dwChannelMask
        } else {
            0
        };
        let init = client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 1_000_000, 0, format, None);
        CoTaskMemFree(Some(format as *const c_void));
        init?;
        if !float || f.wBitsPerSample != 32 {
            bail!("the test renderer requires a float WASAPI mix format");
        }
        let renderer: IAudioRenderClient = client.GetService()?;
        let buffer_frames = client.GetBufferSize()?;
        let render = Render(client);
        render.0.Start()?;
        let output = Output {
            render,
            renderer,
            channels: f.nChannels as usize,
            sample_rate: f.nSamplesPerSec,
            buffer_frames,
        };
        eprintln!(
            "AUDIO_RENDER buffer_frames={buffer_frames} sample_rate={} channels={} mask={mask:#x}",
            output.sample_rate, output.channels
        );
        Ok(output)
    }
}
impl Output {
    /// Tops the buffer up with the pattern from `position` on.
    fn render(&self, pattern: &Pattern, position: &mut u64, start: Instant) -> Result<()> {
        unsafe {
            let padding = self.render.0.GetCurrentPadding()?;
            let available = self.buffer_frames - padding;
            if *position > 0 && available == self.buffer_frames {
                eprintln!(
                    "AUDIO_RENDER_UNDERRUN elapsed_seconds={:.3}",
                    start.elapsed().as_secs_f64()
                );
            }
            if available > 0 {
                let now = butterpollo_windows::timing::qpc_ms();
                let data = self.renderer.GetBuffer(available)?;
                let output = std::slice::from_raw_parts_mut(
                    data.cast::<f32>(),
                    available as usize * self.channels,
                );
                let rate = u64::from(self.sample_rate);
                for (index, frame) in output.chunks_exact_mut(self.channels).enumerate() {
                    if matches!(pattern, Pattern::Sweep) && (*position).is_multiple_of(rate) {
                        // The frame plays once what is queued ahead of it has.
                        let ahead = f64::from(padding) + index as f64;
                        eprintln!(
                            "AUDIO_SWEEP channel={} play_qpc_ms={:.3}",
                            (*position / rate) % self.channels as u64,
                            now + ahead * 1000. / f64::from(self.sample_rate)
                        );
                    }
                    pattern.fill(frame, *position, self.sample_rate);
                    *position += 1;
                }
                self.renderer.ReleaseBuffer(available, 0)?;
            }
        }
        Ok(())
    }
}
fn main() -> Result<()> {
    let _com = ComGuard::new()?;
    let mut args = std::env::args().skip(1);
    let duration = args.next().unwrap_or_else(|| "2".into());
    if duration == "--list" {
        println!("{}", serde_json::to_string(&audio_route::endpoints()?)?);
        return Ok(());
    }
    if duration == "--default-cost" {
        return default_cost(args.next().as_deref().unwrap_or("100").parse()?);
    }
    let seconds: u64 = duration.parse()?;
    let selected = args.next();
    let render_only = args.next().as_deref() == Some("--render-only");
    if !(1..=3600).contains(&seconds) || args.next().is_some() {
        bail!(
            "usage: audio_probe SECONDS [ENDPOINT_ID [--render-only]], with BUTTERPOLLO_TEST_TONE_PATTERN=tone|sweep|levels"
        );
    }
    let pattern = Pattern::from_env()?;
    let endpoint = audio_route::endpoints()?
        .into_iter()
        .find(|e| e.virtual_sink && selected.as_ref().is_none_or(|id| id == &e.id))
        .context("Steam Streaming Speakers is required for this isolated audio test")?;
    let id: Vec<u16> = endpoint.id.encode_utf16().chain(Some(0)).collect();
    let mut output = open(&id)?;
    let mut capture = if render_only {
        None
    } else {
        Some(Loopback::new_sink(2, &endpoint.id)?)
    };
    let timer = Timer::new()?;
    let mut position = 0u64;
    let mut peak = 0f32;
    let mut energy = 0f64;
    let mut samples = 0usize;
    // CPU contention must not starve the source of a host continuity test.
    let _priority = Priority::new();
    let start = Instant::now();
    let end = start + Duration::from_secs(seconds);
    while Instant::now() < end {
        if let Err(error) = output.render(&pattern, &mut position, start) {
            // A host switching the sink's speaker format invalidates this stream: reopen it in
            // the new format and carry on with the pattern.
            eprintln!("AUDIO_RENDER_REOPEN error={error:#}");
            drop(output);
            std::thread::sleep(Duration::from_millis(50));
            output = open(&id)?;
        }
        if let Some(capture) = &mut capture {
            while let Some(packet) = capture.read(240)? {
                for sample in packet {
                    peak = peak.max(sample.abs());
                    energy += f64::from(sample).powi(2);
                    samples += 1;
                }
            }
        }
        timer.until(Instant::now() + Duration::from_millis(1));
    }
    let rms = (energy / samples.max(1) as f64).sqrt();
    println!(
        "{}",
        serde_json::json!({"endpoint":endpoint.name,"rendered_frames":position,"render_only":render_only,"samples":samples,"peak":peak,"rms":rms})
    );
    if !render_only && (samples < 48000 || peak < 0.01 || rms < 0.005) {
        bail!("WASAPI did not capture the rendered tone");
    }
    Ok(())
}
/// Time what the audio route's once-a-second upkeep asks Windows: a fresh
/// device enumerator and the default playback device for each role. Until
/// rc.29 the audio sender did this before reading. Reads only.
fn default_cost(rounds: usize) -> Result<()> {
    let mut costs = Vec::with_capacity(rounds);
    for _ in 0..rounds.max(1) {
        let start = Instant::now();
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            for role in [eConsole, eMultimedia, eCommunications] {
                if let Ok(device) = enumerator.GetDefaultAudioEndpoint(eRender, role) {
                    let id = device.GetId()?;
                    CoTaskMemFree(Some(id.0.cast()));
                }
            }
        }
        costs.push(start.elapsed().as_secs_f64() * 1000.);
        std::thread::sleep(Duration::from_millis(20));
    }
    costs.sort_by(f64::total_cmp);
    let at = |q: f64| costs[((costs.len() - 1) as f64 * q).round() as usize];
    println!(
        "{}",
        serde_json::json!({"rounds":costs.len(),"avg_ms":costs.iter().sum::<f64>() / costs.len() as f64,"p50_ms":at(0.5),"p95_ms":at(0.95),"max_ms":at(1.)})
    );
    Ok(())
}
