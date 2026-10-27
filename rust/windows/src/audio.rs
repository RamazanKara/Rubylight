use anyhow::{Context, Result, bail};
use std::{
    ffi::c_void,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::Path,
};
use windows::Win32::{Media::Audio::*, System::Com::*};
mod process_loopback;
pub use process_loopback::Activation as ProcessActivation;

struct MixFormat(*mut WAVEFORMATEX);
impl Drop for MixFormat {
    fn drop(&mut self) {
        // SAFETY: GetMixFormat allocated this pointer; this guard frees it exactly once.
        unsafe { CoTaskMemFree(Some(self.0.cast::<c_void>())) };
    }
}
pub struct Loopback {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    /// Set by Windows when captured audio is ready; None where event mode
    /// could not be opened, and the caller polls.
    ready: Option<OwnedHandle>,
    samples: CaptureSamples,
    /// Audio Windows holds for us; what arrives while it is full is lost.
    buffer: std::time::Duration,
    drained: Option<std::time::Instant>,
    rate: u32,
    /// Where the last packet started in the device's stream, and where the
    /// next should; a later start skipped what Windows dropped.
    position: Option<(u64, u64)>,
    /// Whether audio arrived since `take_delivered`, and the frames
    /// skipped before it.
    delivered: Option<u64>,
    /// Keeps a process identity alive for game-only capture; system capture
    /// owns no process and retains its existing device-routing behavior.
    process: Option<crate::process::AudioProcess>,
    /// The endpoint's own channel count and speaker mask.
    source: (usize, u32),
    /// Keeps an endpoint capture delivering through silence; None for game-only capture.
    _keepalive: Option<KeepAlive>,
}
/// A started render stream of silence on the captured endpoint. Loopback capture delivers
/// nothing while no stream renders to its endpoint, so when the sound stopped the host went
/// quiet until it noticed (50 ms) and the client heard an underrun; with this the endpoint keeps
/// delivering silent audio at its own pace, and sound turning to silence leaves no gap. It
/// renders one buffer of silence and is left to starve, which keeps the stream active (OBS does
/// the same); the client gets the silent Opus packets it got after the 50 ms anyway.
struct KeepAlive(IAudioClient);
impl KeepAlive {
    /// # Safety
    /// COM is initialized on this thread and `format` is the endpoint's mix format.
    unsafe fn new(device: &IMMDevice, format: *const WAVEFORMATEX) -> Result<Self> {
        // SAFETY: as the caller promises; the buffer is released before Start.
        unsafe {
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 100_000, 0, format, None)?;
            let frames = client.GetBufferSize()?;
            let render: IAudioRenderClient = client.GetService()?;
            render.GetBuffer(frames)?;
            render.ReleaseBuffer(frames, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)?;
            client.Start()?;
            Ok(Self(client))
        }
    }
}
impl Drop for KeepAlive {
    fn drop(&mut self) {
        // SAFETY: the client is live and started.
        unsafe {
            let _ = self.0.Stop();
        }
    }
}
impl Loopback {
    pub fn new(output_channels: usize) -> Result<Self> {
        Self::new_sink(output_channels, "")
    }
    pub fn new_sink(output_channels: usize, sink: &str) -> Result<Self> {
        // SAFETY: COM calls only fail if the thread lacks COM, `id` outlives GetDevice, and the mix
        // format is read within its header plus cbSize bytes and freed once.
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = if sink.is_empty() {
                enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?
            } else {
                let id: Vec<u16> = sink.encode_utf16().chain(Some(0)).collect();
                enumerator.GetDevice(windows::core::PCWSTR(id.as_ptr()))?
            };
            let mut client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let format = MixFormat(client.GetMixFormat()?);
            if format.0.is_null() {
                bail!("empty WASAPI mix format");
            }
            let f = *format.0;
            let float = f.wFormatTag == 3
                || (f.wFormatTag == 65534
                    && f.cbSize >= 22
                    && (*(format.0 as *const WAVEFORMATEXTENSIBLE)).SubFormat.data1 == 3);
            let mask = if f.wFormatTag == 65534 && f.cbSize >= 22 {
                (*(format.0 as *const WAVEFORMATEXTENSIBLE)).dwChannelMask
            } else {
                0
            };
            let matrix =
                butterpollo_core::audio::mix_matrix(f.nChannels as usize, mask, output_channels);
            let resampler =
                butterpollo_core::audio::Resampler::new(f.nSamplesPerSec, output_channels);
            // Event mode wakes the sender the moment audio is captured, as the
            // C++ host does; a 1 ms poll cost 0.6 ms on average. Where Windows
            // refuses it for loopback, a fresh client polls as before.
            let ready = windows::Win32::System::Threading::CreateEventW(
                None,
                false,
                false,
                windows::core::PCWSTR::null(),
            )
            .ok()
            .map(|handle| OwnedHandle::from_raw_handle(handle.0));
            let mut result = Err(windows::core::Error::empty());
            if let Some(event) = &ready {
                result = client
                    .Initialize(
                        AUDCLNT_SHAREMODE_SHARED,
                        AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                        100000,
                        0,
                        format.0,
                        None,
                    )
                    .and_then(|()| {
                        client.SetEventHandle(windows::Win32::Foundation::HANDLE(
                            event.as_raw_handle(),
                        ))
                    });
            }
            let ready = if result.is_ok() {
                ready
            } else {
                drop(ready);
                client = device.Activate(CLSCTX_ALL, None)?;
                result = client.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_LOOPBACK,
                    100000,
                    0,
                    format.0,
                    None,
                );
                None
            };
            result?;
            let matrix = matrix?;
            let resampler = resampler?;
            if !matches!(f.wBitsPerSample, 16 | 24 | 32) || (float && f.wBitsPerSample != 32) {
                bail!("unsupported WASAPI sample format");
            }
            // Unknown when Windows will not say; loss is then not estimated.
            let rate = f.nSamplesPerSec.max(1);
            let buffer = client
                .GetBufferSize()
                .map_or(std::time::Duration::ZERO, |frames| {
                    std::time::Duration::from_secs_f64(f64::from(frames) / f64::from(rate))
                });
            let capture = client.GetService()?;
            client.Start()?;
            // Without it the capture still works and goes quiet through silence, as before.
            let keepalive = KeepAlive::new(&device, format.0)
                .inspect_err(|error| {
                    tracing::warn!(
                        "audio keep-alive render stream failed, silence will start late: {error:#}"
                    )
                })
                .ok();
            Ok(Self {
                client,
                capture,
                ready,
                samples: CaptureSamples {
                    channels: f.nChannels as usize,
                    bits: f.wBitsPerSample,
                    float,
                    resampler,
                    matrix,
                    input: vec![0.; f.nChannels as usize],
                    output: vec![0.; output_channels],
                    trimmed: 0,
                },
                buffer,
                drained: None,
                rate,
                position: None,
                delivered: None,
                process: None,
                source: (f.nChannels as usize, mask),
                _keepalive: keepalive,
            })
        }
    }
    pub fn process_identity(&self) -> Option<butterpollo_core::audio_scope::Identity> {
        self.process.as_ref().map(|p| p.identity)
    }
    /// Captures the verified game process and its children, across playback
    /// devices. It never switches/mutes endpoints or captures other processes.
    fn from_process_client(
        output_channels: usize,
        process: crate::process::AudioProcess,
        client: IAudioClient,
    ) -> Result<Self> {
        anyhow::ensure!(
            process.alive(),
            "game process exited during audio activation"
        );
        // An explicit 48 kHz float stereo format works independently of an
        // endpoint mix format. Surround streams receive a documented stereo
        // upmix until process-loopback surround is independently validated.
        let format = WAVEFORMATEX {
            wFormatTag: 3,
            nChannels: 2,
            nSamplesPerSec: 48_000,
            nAvgBytesPerSec: 48_000 * 8,
            nBlockAlign: 8,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        // SAFETY: COM is initialized by the caller; the format lives through
        // Initialize, and the owned event outlives the client capture.
        unsafe {
            let event = windows::Win32::System::Threading::CreateEventW(
                None,
                false,
                false,
                windows::core::PCWSTR::null(),
            )?;
            let ready = OwnedHandle::from_raw_handle(event.0);
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK
                    | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                0,
                0,
                &format,
                None,
            )?;
            client.SetEventHandle(event)?;
            let buffer = client
                .GetBufferSize()
                .map_or(std::time::Duration::ZERO, |n| {
                    std::time::Duration::from_secs_f64(f64::from(n) / 48_000.)
                });
            let capture = client.GetService()?;
            let matrix = butterpollo_core::audio::mix_matrix(2, 3, output_channels)?;
            let resampler = butterpollo_core::audio::Resampler::new(48_000, output_channels)?;
            client.Start()?;
            Ok(Self {
                client,
                capture,
                ready: Some(ready),
                samples: CaptureSamples {
                    channels: 2,
                    bits: 32,
                    float: true,
                    resampler,
                    matrix,
                    input: vec![0.; 2],
                    output: vec![0.; output_channels],
                    trimmed: 0,
                },
                buffer,
                drained: None,
                rate: 48_000,
                position: None,
                delivered: None,
                process: Some(process),
                source: (2, 3),
                _keepalive: None,
            })
        }
    }
    /// Wait up to `timeout` for captured audio, or the whole timeout without
    /// event mode. An idle endpoint sends no events; the timeout keeps the
    /// caller's silence on time.
    pub fn wait(&self, timeout: std::time::Duration) {
        match &self.ready {
            // SAFETY: `event` is the event this struct owns, closed only in Drop.
            Some(event) => unsafe {
                let _ = windows::Win32::System::Threading::WaitForSingleObject(
                    windows::Win32::Foundation::HANDLE(event.as_raw_handle()),
                    timeout.as_millis().clamp(1, u128::from(u32::MAX)) as u32,
                );
            },
            None => std::thread::sleep(timeout),
        }
    }
    pub fn event_driven(&self) -> bool {
        self.ready.is_some()
    }
    /// The endpoint's own channel count and speaker mask, before they are
    /// mixed to the output channels; a mask of 0 means the standard order.
    pub fn source(&self) -> (usize, u32) {
        self.source
    }
    /// How much audio Windows holds between reads; zero when unknown.
    pub fn buffer(&self) -> std::time::Duration {
        self.buffer
    }
    /// The time since the previous call, None on the first. Call it before
    /// reading every captured packet: Windows lost what arrived after the
    /// buffer filled in between.
    pub fn since_drained(&mut self) -> Option<std::time::Duration> {
        let now = std::time::Instant::now();
        self.drained
            .replace(now)
            .map(|at| now.saturating_duration_since(at))
    }
    /// None when no audio arrived since the last call, else how much
    /// Windows skipped before it because it had no room.
    pub fn take_delivered(&mut self) -> Option<std::time::Duration> {
        self.delivered
            .take()
            .map(|frames| std::time::Duration::from_secs_f64(frames as f64 / f64::from(self.rate)))
    }
    /// How much captured audio this host discarded since the last call (a backlog trimmed
    /// after a late wake, or the 100 ms cap), in 48 kHz samples. Windows delivered it, so
    /// `take_delivered` does not count it; it is a gap in the stream all the same.
    pub fn take_trimmed(&mut self) -> u64 {
        self.samples.take_trimmed(self.rate)
    }
    /// What this capture holds that will never be sent because it is being dropped: the
    /// audio it trimmed and the queue the resampler never got to read, in 48 kHz samples.
    pub fn take_discarded(&mut self) -> u64 {
        self.samples.take_trimmed(self.rate) + self.samples.queued(self.rate)
    }
    pub fn read(&mut self, frames: usize) -> Result<Option<Vec<f32>>> {
        anyhow::ensure!(
            self.process.as_ref().is_none_or(|process| process.alive()),
            "game audio process exited; waiting for a verified surviving game process"
        );
        // SAFETY: GetBuffer returns `count` frames of `channels` samples of `bits` bits each, read
        // only before ReleaseBuffer; each read is unaligned and inside that packet.
        unsafe {
            loop {
                let n = self.capture.GetNextPacketSize()?;
                if n == 0 {
                    break;
                }
                let mut data = std::ptr::null_mut();
                let mut count = 0;
                let mut flags = 0;
                let mut position = 0;
                self.capture.GetBuffer(
                    &mut data,
                    &mut count,
                    &mut flags,
                    Some(&mut position),
                    None,
                )?;
                // A packet split at the same position continues the last one.
                let delivered = self.delivered.get_or_insert(0);
                let start = match self.position {
                    Some((last, next)) if last == position => next,
                    Some((_, next)) => {
                        *delivered += position.saturating_sub(next);
                        position
                    }
                    None => position,
                };
                self.position = Some((position, start + u64::from(count)));
                self.samples.push_packet(data, count as usize, flags);
                self.capture.ReleaseBuffer(count)?;
            }
            Ok(self.samples.read(frames))
        }
    }
}

/// PCM queued across WASAPI packets, independent of the native capture client.
struct CaptureSamples {
    channels: usize,
    bits: u16,
    float: bool,
    resampler: butterpollo_core::audio::Resampler,
    matrix: Vec<Vec<f32>>,
    input: Vec<f32>,
    output: Vec<f32>,
    /// Frames, at the resampler's input rate, this host discarded from the queue since
    /// they were last taken: audio Windows delivered and nothing sent.
    trimmed: u64,
}
impl CaptureSamples {
    /// The audio discarded since the last call, as 48 kHz samples of a device running at `rate`.
    fn take_trimmed(&mut self, rate: u32) -> u64 {
        std::mem::take(&mut self.trimmed) * 48_000 / u64::from(rate)
    }
    /// The audio queued and not yet read, as 48 kHz samples of a device running at `rate`.
    fn queued(&self, rate: u32) -> u64 {
        self.resampler.queued() as u64 * 48_000 / u64::from(rate)
    }
    /// # Safety
    /// Unless silent or null, `data` holds `count` interleaved frames in this
    /// capture's validated sample format and remains readable throughout.
    unsafe fn push_packet(&mut self, data: *const u8, count: usize, flags: u32) {
        let step = (self.bits / 8) as usize;
        let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
        // DATA_DISCONTINUITY marks a stream-position discontinuity, not invalid
        // earlier samples. Clearing them here also erased whole packets drained
        // earlier in this read, or a partial Opus packet from the previous read.
        // Keep those samples and the fractional resampling phase; read() still
        // bounds the entire backlog.
        for frame in 0..count {
            for (source, sample) in self.input.iter_mut().enumerate() {
                let value = if silent || data.is_null() {
                    0.
                } else {
                    // SAFETY: The caller supplies count complete frames in this
                    // format; each unaligned access stays within its sample.
                    unsafe {
                        let p = data.add((frame * self.channels + source) * step);
                        if self.float {
                            std::ptr::read_unaligned(p as *const f32)
                        } else {
                            match self.bits {
                                16 => std::ptr::read_unaligned(p as *const i16) as f32 / 32768.,
                                24 => {
                                    let v = ((*p as i32)
                                        | ((*p.add(1) as i32) << 8)
                                        | ((*p.add(2) as i32) << 16))
                                        << 8;
                                    v as f32 / 2147483648.
                                }
                                _ => std::ptr::read_unaligned(p as *const i32) as f32 / 2147483648.,
                            }
                        }
                    }
                };
                *sample = if value.is_finite() {
                    value.clamp(-1., 1.)
                } else {
                    0.
                };
            }
            for (dest, row) in self.output.iter_mut().zip(&self.matrix) {
                *dest = self.input.iter().zip(row).map(|(v, gain)| v * gain).sum();
            }
            self.trimmed += self.resampler.push(&self.output) as u64;
        }
    }
    fn read(&mut self, frames: usize) -> Option<Vec<f32>> {
        // Steady state queues under one WASAPI period plus a packet;
        // keep two packets once a stall left more than 30 ms.
        let packet_ms = (frames / 48).max(1) as u32;
        let dropped = self.resampler.bound((packet_ms * 3).max(30), packet_ms * 2);
        if dropped > 0 {
            self.trimmed += dropped as u64;
            tracing::debug!("audio capture backlog dropped");
        }
        self.resampler.read(frames)
    }
}
impl Drop for Loopback {
    fn drop(&mut self) {
        // SAFETY: `self.client` is live. The owned event closes after capture stops.
        unsafe {
            let _ = self.client.Stop();
        }
    }
}
type Create = unsafe extern "C" fn(i32, i32, i32, i32, *const u8, i32, *mut i32) -> *mut c_void;
type Encode = unsafe extern "C" fn(*mut c_void, *const f32, i32, *mut u8, i32) -> i32;
type Destroy = unsafe extern "C" fn(*mut c_void);
type Ctl = unsafe extern "C" fn(*mut c_void, i32, ...) -> i32;
pub struct Opus {
    state: *mut c_void,
    encode: Encode,
    destroy: Destroy,
    _library: libloading::Library,
    channels: usize,
}
impl Opus {
    pub fn new(directory: &Path, channels: usize, quality: bool) -> Result<Self> {
        Self::new_layout(
            directory,
            &butterpollo_core::audio::OpusLayout::select(channels, quality, None)?,
        )
    }
    pub fn new_layout(
        directory: &Path,
        layout: &butterpollo_core::audio::OpusLayout,
    ) -> Result<Self> {
        Self::new_layout_duration(directory, layout, 5)
    }
    pub fn new_layout_duration(
        directory: &Path,
        layout: &butterpollo_core::audio::OpusLayout,
        packet_ms: u8,
    ) -> Result<Self> {
        if !matches!(packet_ms, 5 | 10 | 20) {
            bail!("unsupported Opus packet duration");
        }
        // High-quality surround at 10/20ms otherwise exceeds Moonlight's
        // fixed UDP buffer. Opus can silently starve the last coded stream
        // when only its output buffer is capped. Bound CBR before encoding,
        // reserving space for each self-delimited stream's length fields.
        let packet_bitrate = (1360 - layout.streams * 4) * 8000 / i32::from(packet_ms);
        let bitrate = layout.bitrate.min(packet_bitrate);
        // SAFETY: the symbol types match libopus, `layout.mapping` has `channels` entries, and
        // `library` is kept in `Self` with its function pointers.
        unsafe {
            let path = directory.join("libopus-0.dll");
            let library = libloading::Library::new(&path)
                .with_context(|| format!("loading {}", path.display()))?;
            let create: libloading::Symbol<Create> =
                library.get(b"opus_multistream_encoder_create\0")?;
            let encode = *library.get::<Encode>(b"opus_multistream_encode_float\0")?;
            let destroy = *library.get::<Destroy>(b"opus_multistream_encoder_destroy\0")?;
            let ctl = *library.get::<Ctl>(b"opus_multistream_encoder_ctl\0")?;
            let mut error = 0;
            let state = create(
                48000,
                layout.channels as i32,
                layout.streams,
                layout.coupled,
                layout.mapping.as_ptr(),
                2051,
                &mut error,
            );
            if error != 0 || state.is_null() {
                bail!("Opus initialization failed: {error}");
            }
            let encoder = Self {
                state,
                encode,
                destroy,
                _library: library,
                channels: layout.channels,
            };
            for (request, value) in [(4002, bitrate), (4006, 0i32)] {
                let result = ctl(state, request, value);
                if result != 0 {
                    bail!("Opus control {request} failed: {result}");
                }
            }
            Ok(encoder)
        }
    }
    pub fn encode(&mut self, samples: &[f32]) -> Result<Vec<u8>> {
        if !samples.len().is_multiple_of(self.channels) {
            bail!("incomplete audio frame");
        }
        // Leave room for RTP, AES padding and parity headers within the
        // previous Moonlight receiver's fixed 1400-byte UDP buffer.
        let mut output = vec![0; 1360];
        // SAFETY: `self.state` is a live encoder, `samples` holds whole frames for `channels`, and
        // `output` is writable for the length passed.
        let n = unsafe {
            (self.encode)(
                self.state,
                samples.as_ptr(),
                (samples.len() / self.channels) as i32,
                output.as_mut_ptr(),
                output.len() as i32,
            )
        };
        if n < 0 {
            bail!("Opus encoding error {n}");
        }
        output.truncate(n as usize);
        Ok(output)
    }
}
impl Drop for Opus {
    fn drop(&mut self) {
        // SAFETY: `self.state` came from opus_multistream_encoder_create and is destroyed once,
        // here.
        unsafe {
            (self.destroy)(self.state);
        }
    }
}

type DecoderCreate = unsafe extern "C" fn(i32, i32, *mut i32) -> *mut c_void;
type Decode = unsafe extern "C" fn(*mut c_void, *const u8, i32, *mut f32, i32, i32) -> i32;
type PacketSamples = unsafe extern "C" fn(*const c_void, *const u8, i32) -> i32;
/// Longest Opus frame, 120 ms at 48 kHz.
const MAX_OPUS_FRAMES: usize = 5760;
/// A mono 48 kHz Opus decoder, for a client's microphone.
pub struct OpusDecoder {
    state: *mut c_void,
    decode: Decode,
    samples: PacketSamples,
    ctl: Ctl,
    destroy: Destroy,
    _library: libloading::Library,
    /// The last frame's length, for concealing the next one.
    frames: usize,
}
// SAFETY: the decoder state is used only through `&mut self`.
unsafe impl Send for OpusDecoder {}
impl OpusDecoder {
    pub fn new(directory: &Path) -> Result<Self> {
        // SAFETY: the symbol types match libopus, and `library` is kept in `Self` with its
        // function pointers.
        unsafe {
            let path = directory.join("libopus-0.dll");
            let library = libloading::Library::new(&path)
                .with_context(|| format!("loading {}", path.display()))?;
            let create = *library.get::<DecoderCreate>(b"opus_decoder_create\0")?;
            let decode = *library.get::<Decode>(b"opus_decode_float\0")?;
            let samples = *library.get::<PacketSamples>(b"opus_decoder_get_nb_samples\0")?;
            let ctl = *library.get::<Ctl>(b"opus_decoder_ctl\0")?;
            let destroy = *library.get::<Destroy>(b"opus_decoder_destroy\0")?;
            let mut error = 0;
            let state = create(48000, 1, &mut error);
            if error != 0 || state.is_null() {
                bail!("Opus decoder initialization failed: {error}");
            }
            Ok(Self {
                state,
                decode,
                samples,
                ctl,
                destroy,
                _library: library,
                frames: 960,
            })
        }
    }
    /// Forgets the previous speech, for a stream that starts again.
    pub fn reset(&mut self) {
        // SAFETY: `self.state` is a live decoder; OPUS_RESET_STATE takes no argument.
        unsafe {
            (self.ctl)(self.state, 4028);
        }
    }
    /// Appends a frame to `out` and returns its length: `packet` decoded,
    /// the frame before it from its in-band FEC when `fec` is set, or a
    /// concealed frame when there is no packet.
    pub fn decode(
        &mut self,
        packet: Option<&[u8]>,
        fec: bool,
        out: &mut Vec<f32>,
    ) -> Result<usize> {
        let (data, length) = packet.map_or((std::ptr::null(), 0), |p| (p.as_ptr(), p.len() as i32));
        let frames = match packet {
            // SAFETY: `self.state` is a live decoder and `data` is readable for `length` bytes.
            Some(_) => match unsafe { (self.samples)(self.state, data, length) } {
                n if n > 0 && (n as usize) <= MAX_OPUS_FRAMES => n as usize,
                n => bail!("invalid Opus packet ({n})"),
            },
            None => self.frames,
        };
        let start = out.len();
        out.resize(start + frames, 0.);
        // SAFETY: `self.state` is a live decoder, `data` is null or readable for `length` bytes,
        // and `out` has room for `frames` mono samples after `start`.
        let n = unsafe {
            (self.decode)(
                self.state,
                data,
                length,
                out[start..].as_mut_ptr(),
                frames as i32,
                i32::from(fec),
            )
        };
        if n < 0 {
            out.truncate(start);
            bail!("Opus decoding error {n}");
        }
        out.truncate(start + n as usize);
        self.frames = n as usize;
        Ok(n as usize)
    }
}
impl Drop for OpusDecoder {
    fn drop(&mut self) {
        // SAFETY: `self.state` came from opus_decoder_create and is destroyed once, here.
        unsafe {
            (self.destroy)(self.state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "needs the default playback device; proves the fix only while nothing plays"]
    fn an_endpoint_capture_keeps_delivering_through_silence() {
        // SAFETY: no pointers; the thread keeps COM for the test.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap() };
        // The host's 1 ms timer resolution, so 5 ms waits are not 15.6 ms ones.
        // SAFETY: no pointers.
        unsafe { windows::Win32::Media::timeBeginPeriod(1) };
        let mut capture = Loopback::new(2).unwrap();
        let start = std::time::Instant::now();
        let (mut first, mut last, mut worst, mut packets) =
            (None, start, std::time::Duration::ZERO, 0);
        while start.elapsed() < std::time::Duration::from_secs(2) {
            capture.wait(std::time::Duration::from_millis(5));
            while capture.read(240).unwrap().is_some() {
                let now = std::time::Instant::now();
                // The endpoint takes a moment to start delivering.
                if first.is_some_and(|at| now - at > std::time::Duration::from_millis(200)) {
                    worst = worst.max(now - last);
                }
                first.get_or_insert(now);
                last = now;
                packets += 1;
            }
        }
        // An idle endpoint without the keep-alive delivers nothing at all. With it, the audio
        // keeps time, and no wait comes near the 50 ms after which the host sends its own
        // silence (deliveries bunch up to about 40 ms on a busy PC, as they do with sound).
        eprintln!("{packets} packets, worst gap {worst:?}");
        assert!(packets >= 360, "{packets} packets of 5 ms in 2 s");
        assert!(
            worst < std::time::Duration::from_millis(50),
            "a {worst:?} gap between deliveries"
        );
    }

    fn capture_samples(rate: u32) -> CaptureSamples {
        CaptureSamples {
            channels: 2,
            bits: 32,
            float: true,
            resampler: butterpollo_core::audio::Resampler::new(rate, 2).unwrap(),
            matrix: butterpollo_core::audio::mix_matrix(2, 3, 2).unwrap(),
            input: vec![0.; 2],
            output: vec![0.; 2],
            trimmed: 0,
        }
    }

    fn captured(samples: &mut CaptureSamples, values: &[f32], flags: u32) {
        assert_eq!(
            (samples.channels, samples.bits, samples.float),
            (2, 32, true)
        );
        assert!(values.len().is_multiple_of(2));
        // SAFETY: values contains complete float stereo frames and outlives the call.
        unsafe { samples.push_packet(values.as_ptr().cast(), values.len() / 2, flags) };
    }

    #[test]
    fn capture_discontinuity_preserves_valid_packets_from_the_same_drain() {
        let discontinuity = AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32;
        for packet_ms in [5, 10, 20] {
            let mut samples = capture_samples(48_000);
            // Windows delivers two 10 ms packets in one drain. The flag on
            // the later packet must not erase any of the earlier packet.
            let first = vec![0.25; 480 * 2];
            let second = vec![-0.25; 480 * 2];
            captured(&mut samples, &first, 0);
            captured(&mut samples, &second, discontinuity);
            let mut output = Vec::new();
            while let Some(packet) = samples.read(packet_ms * 48) {
                output.extend(packet);
            }
            assert_eq!(output, [first, second].concat(), "{packet_ms} ms packets");
        }
    }

    #[test]
    fn capture_discontinuity_preserves_a_partial_packet_across_reads() {
        let mut samples = capture_samples(48_000);
        let first = vec![0.25; 480 * 2];
        captured(&mut samples, &first, 0);
        assert!(samples.read(960).is_none());
        // A silent discontinuity also carries valid frames, without a data
        // pointer; the preceding half packet must still be delivered.
        // SAFETY: Silent packets do not read the null data pointer.
        unsafe {
            samples.push_packet(
                std::ptr::null(),
                480,
                (AUDCLNT_BUFFERFLAGS_SILENT.0 | AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0) as u32,
            );
        }
        assert_eq!(
            samples.read(960).unwrap(),
            [first, vec![0.; 480 * 2]].concat()
        );
        assert!(samples.read(960).is_none());
    }

    #[test]
    fn capture_discontinuity_keeps_fractional_resampling_position() {
        let mut samples = capture_samples(44_100);
        let ramp = |start| {
            (start..start + 222)
                .flat_map(|frame| [frame as f32 / 1000., -(frame as f32) / 1000.])
                .collect::<Vec<_>>()
        };
        captured(&mut samples, &ramp(0), 0);
        let mut output = samples.read(240).unwrap();
        captured(
            &mut samples,
            &ramp(222),
            AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32,
        );
        output.extend(samples.read(240).unwrap());
        for (frame, stereo) in output.as_chunks::<2>().0.iter().enumerate() {
            let expected = frame as f32 * 44_100. / 48_000. / 1000.;
            assert!((stereo[0] - expected).abs() < 1e-6, "frame {frame}");
            assert!((stereo[1] + expected).abs() < 1e-6, "frame {frame}");
        }
    }

    /// What a late audio thread leaves: `queued_ms` of audio waiting when it wakes, all read in
    /// one pass. Returns the packets read and the samples discarded, both at 48 kHz.
    fn late_wake(rate: u32, queued_ms: usize, packet_ms: usize) -> (usize, u64) {
        let mut samples = capture_samples(rate);
        let frames = rate as usize * queued_ms / 1000;
        let input = vec![0.1; frames * 2];
        // Windows hands over its 10 ms chunks as they come; the thread has not read any yet.
        for chunk in input.chunks(rate as usize / 100 * 2) {
            captured(&mut samples, chunk, 0);
        }
        let mut packets = 0;
        while samples.read(packet_ms * 48).is_some() {
            packets += 1;
        }
        (packets, samples.take_trimmed(rate))
    }

    #[test]
    fn a_late_wake_reports_the_backlog_it_trims_so_the_stream_keeps_its_time() {
        for rate in [48_000, 44_100] {
            for (queued_ms, packet_ms) in [(100, 5), (60, 5), (45, 10), (100, 20)] {
                let (packets, trimmed) = late_wake(rate, queued_ms, packet_ms);
                // Every millisecond woken up late is either a packet or counted as trimmed.
                // At 48 kHz that is exact. A fractional rate may leave up to two packets queued
                // unread (still in the stream, not lost).
                let accounted = packets * packet_ms * 48 + trimmed as usize;
                let queued = queued_ms * 48;
                let unread = if rate == 48_000 {
                    0
                } else {
                    2 * packet_ms * 48
                };
                assert!(
                    accounted <= queued && queued - accounted <= unread,
                    "{rate} Hz, {queued_ms} ms, {packet_ms} ms packets: {packets} packets, {trimmed} trimmed"
                );
                assert!(trimmed > 0, "{rate} Hz, {queued_ms} ms");
            }
        }
    }

    #[test]
    fn a_wake_within_the_limit_trims_nothing() {
        for (queued_ms, packet_ms) in [(25, 5), (30, 5), (30, 10), (60, 20)] {
            let (packets, trimmed) = late_wake(48_000, queued_ms, packet_ms);
            assert_eq!(trimmed, 0, "{queued_ms} ms, {packet_ms} ms packets");
            assert_eq!(packets, queued_ms / packet_ms);
        }
    }

    #[test]
    fn the_hundred_millisecond_cap_is_reported_as_trimmed_too() {
        let mut samples = capture_samples(48_000);
        // 150 ms pushed before any read: the cap keeps the newest 100 ms.
        for _ in 0..15 {
            captured(&mut samples, &vec![0.1; 480 * 2], 0);
        }
        assert_eq!(samples.take_trimmed(48_000), 50 * 48);
        // Taken once: a second call has nothing new.
        assert_eq!(samples.take_trimmed(48_000), 0);
    }

    #[test]
    fn capture_discontinuities_keep_the_combined_backlog_bounded() {
        for packet_ms in [5, 10, 20] {
            let limit_ms = (packet_ms * 3).max(30);
            for captured_ms in [limit_ms, limit_ms + 10] {
                let mut samples = capture_samples(48_000);
                let input = (0..captured_ms * 48)
                    .flat_map(|frame| [frame as f32 / 10_000.; 2])
                    .collect::<Vec<_>>();
                for packet in input.as_chunks::<960>().0 {
                    captured(
                        &mut samples,
                        packet,
                        AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32,
                    );
                }
                let mut output = Vec::new();
                while let Some(packet) = samples.read(packet_ms * 48) {
                    output.extend(packet);
                }
                let kept_ms = if captured_ms > limit_ms {
                    packet_ms * 2
                } else {
                    captured_ms
                };
                assert_eq!(
                    output,
                    input[input.len() - kept_ms * 48 * 2..],
                    "{packet_ms} ms packets, {captured_ms} ms captured"
                );
            }
        }
    }

    #[test]
    #[ignore = "requires the packaged libopus DLL"]
    fn native_opus_round_trip_preserves_every_surround_channel_and_quality_layout() {
        type DecoderCreate =
            unsafe extern "C" fn(i32, i32, i32, i32, *const u8, *mut i32) -> *mut c_void;
        type Decode = unsafe extern "C" fn(*mut c_void, *const u8, i32, *mut f32, i32, i32) -> i32;
        struct Decoder {
            state: *mut c_void,
            destroy: Destroy,
            _library: libloading::Library,
        }
        impl Drop for Decoder {
            fn drop(&mut self) {
                // SAFETY: `self.state` came from opus_multistream_decoder_create and is destroyed
                // once, here.
                unsafe {
                    (self.destroy)(self.state);
                }
            }
        }
        let root = std::path::PathBuf::from(
            std::env::var_os("BUTTERPOLLO_TEST_OPUS_ROOT").expect("set BUTTERPOLLO_TEST_OPUS_ROOT"),
        );
        // LFE is deliberately band-limited by Opus surround layouts. Exercise
        // that channel with bass, while the other channels use distinct tones.
        fn frequency(channel: usize, channels: usize) -> f64 {
            if channels > 2 && channel == 3 {
                80.0
            } else {
                320.0 + 160.0 * channel as f64
            }
        }
        let mut reports = vec![];
        for (channels, quality, custom) in [
            (2, false, None),
            (2, true, None),
            (6, false, None),
            (6, true, None),
            (8, false, None),
            (8, true, None),
            (12, false, None),
            (12, true, None),
            (6, true, Some("642543210")),
        ] {
            for packet_ms in [5, 10, 20] {
                let layout =
                    butterpollo_core::audio::OpusLayout::select(channels, quality, custom).unwrap();
                let mut encoder =
                    Opus::new_layout_duration(&root, &layout, packet_ms as u8).unwrap();
                // SAFETY: the symbol types match libopus, `layout.mapping` has `channels` entries,
                // and the library is kept in Decoder with its pointers.
                let (decoder, decode) = unsafe {
                    let library = libloading::Library::new(root.join("libopus-0.dll")).unwrap();
                    let create = *library
                        .get::<DecoderCreate>(b"opus_multistream_decoder_create\0")
                        .unwrap();
                    let destroy = *library
                        .get::<Destroy>(b"opus_multistream_decoder_destroy\0")
                        .unwrap();
                    let decode = *library
                        .get::<Decode>(b"opus_multistream_decode_float\0")
                        .unwrap();
                    let mut error = 0;
                    let state = create(
                        48000,
                        channels as i32,
                        layout.streams,
                        layout.coupled,
                        layout.mapping.as_ptr(),
                        &mut error,
                    );
                    assert_eq!(error, 0);
                    assert!(!state.is_null());
                    (
                        Decoder {
                            state,
                            destroy,
                            _library: library,
                        },
                        decode,
                    )
                };
                let frames = packet_ms * 48;
                let mut tones = vec![vec![]; channels];
                let mut bytes = 0;
                for packet in 0..80 {
                    let samples: Vec<_> = (0..frames)
                        .flat_map(|frame| {
                            (0..channels).map(move |channel| {
                                let frequency = frequency(channel, channels) as f32;
                                ((packet * frames + frame) as f32
                                    * std::f32::consts::TAU
                                    * frequency
                                    / 48000.0)
                                    .sin()
                                    * 0.2
                            })
                        })
                        .collect();
                    let encoded = encoder.encode(&samples).unwrap();
                    assert!(encoded.len() <= 1360);
                    bytes += encoded.len();
                    let mut decoded = vec![0.0; frames * channels];
                    // SAFETY: `decoder.state` is a live decoder, `encoded` is read for its length,
                    // and `decoded` holds `frames` frames of `channels` samples.
                    let result = unsafe {
                        decode(
                            decoder.state,
                            encoded.as_ptr(),
                            encoded.len() as i32,
                            decoded.as_mut_ptr(),
                            frames as i32,
                            0,
                        )
                    };
                    assert_eq!(result, frames as i32);
                    if packet >= 8 {
                        for frame in decoded.chunks_exact(channels) {
                            for (channel, sample) in frame.iter().enumerate() {
                                assert!(sample.is_finite());
                                tones[channel].push(*sample);
                            }
                        }
                    }
                }
                for (channel, signal) in tones.iter().enumerate() {
                    let energy = (0..channels)
                        .map(|tone| {
                            let frequency = frequency(tone, channels);
                            let (real, imaginary) = signal.iter().enumerate().fold(
                                (0.0, 0.0),
                                |(real, imaginary), (at, sample)| {
                                    let phase =
                                        at as f64 * std::f64::consts::TAU * frequency / 48000.0;
                                    (
                                        real + f64::from(*sample) * phase.cos(),
                                        imaginary + f64::from(*sample) * phase.sin(),
                                    )
                                },
                            );
                            real * real + imaginary * imaginary
                        })
                        .collect::<Vec<_>>();
                    let strongest = energy
                        .iter()
                        .enumerate()
                        .max_by(|a, b| a.1.total_cmp(b.1))
                        .unwrap()
                        .0;
                    assert_eq!(
                        strongest, channel,
                        "{channels} channels, quality={quality}, packet={packet_ms}ms"
                    );
                    assert!(energy[channel] > 100.0);
                }
                reports.push(serde_json::json!({"channels":channels,"quality":quality,"custom":custom,"packet_ms":packet_ms,"encoded_kbps":bytes as f64 * 8.0 / (80.0 * packet_ms as f64),"channel_errors":0}));
            }
        }
        if let Some(path) = std::env::var_os("BUTTERPOLLO_TEST_AUDIO_REPORT") {
            std::fs::write(path, serde_json::to_vec_pretty(&reports).unwrap()).unwrap();
        }
        eprintln!(
            "Validated {} native Opus encode/decode layouts and packet durations",
            reports.len()
        );
    }
}
