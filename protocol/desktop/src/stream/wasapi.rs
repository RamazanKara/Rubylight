//! 7.1.4 to Windows' default output through WASAPI in shared mode. SDL opens at most 8 channels,
//! so a 12-channel stream plays here and every other stream stays on SDL. It runs as SDL's WASAPI
//! backend does: event driven, one device period written per wake into a buffer of the same size,
//! at the same thread priority, so it adds no latency over SDL. What runs dry between packets is
//! concealed from the playout, as SDL's request callback does.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rubylight_protocol::audio_ext::{CHANNELS_714, CHANNEL_MASK_714};
use windows::core::{GUID, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioClient, IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, WAVEFORMATEX, WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE_0,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW, GetCurrentThread,
    SetThreadPriority, WaitForSingleObject, THREAD_PRIORITY_TIME_CRITICAL,
};

use super::pipeline::lock;
use super::playout::Playout;

/// 7.1.4's channels, in the stream's order FL FR FC LFE BL BR SL SR TFL TFR TBL TBR. Windows
/// interleaves by mask bit, so `CHANNEL_MASK_714` puts them where they belong as they are.
pub const CHANNELS: usize = CHANNELS_714 as usize;
const RATE: u32 = 48_000;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
/// How long the offer's probe of the output is trusted.
const OFFER_FOR: Duration = Duration::from_secs(2);

/// The stream's format: 12 float channels at 48 kHz in 7.1.4's mask. Windows converts it to the
/// output's own format (`AUTOCONVERTPCM`), as it converts SDL's rate.
pub(super) fn format() -> WAVEFORMATEXTENSIBLE {
    let block = (CHANNELS * 4) as u16;
    WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_EXTENSIBLE,
            nChannels: CHANNELS as u16,
            nSamplesPerSec: RATE,
            nAvgBytesPerSec: RATE * u32::from(block),
            nBlockAlign: block,
            wBitsPerSample: 32,
            cbSize: (size_of::<WAVEFORMATEXTENSIBLE>() - size_of::<WAVEFORMATEX>()) as u16,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 { wValidBitsPerSample: 32 },
        dwChannelMask: CHANNEL_MASK_714,
        SubFormat: SUBTYPE_IEEE_FLOAT,
    }
}

/// COM on this thread for as long as it lives.
struct Com(bool);

impl Com {
    fn init() -> Self {
        // SAFETY: paired with CoUninitialize in drop when it succeeded.
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: initialised in init on this thread.
            unsafe { CoUninitialize() };
        }
    }
}

fn default_client() -> windows::core::Result<IAudioClient> {
    // SAFETY: COM is initialised on the calling thread.
    unsafe {
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?.Activate(CLSCTX_ALL, None)
    }
}

/// The default output's mix format has 12 or more channels. Run on a thread of its own so the
/// caller's COM apartment is never touched.
pub fn plays_714() -> bool {
    std::thread::spawn(|| {
        let _com = Com::init();
        let Ok(client) = default_client() else { return 0 };
        // SAFETY: the mix format is read and freed once.
        unsafe {
            let Ok(mix) = client.GetMixFormat() else { return 0 };
            let channels = (*mix).nChannels;
            CoTaskMemFree(Some(mix.cast_const().cast()));
            channels
        }
    })
    .join()
    .is_ok_and(|channels| usize::from(channels) >= CHANNELS)
}

struct Offer {
    plays: bool,
    checked: Option<Instant>,
    probing: bool,
}

static OFFER: Mutex<Offer> = Mutex::new(Offer { plays: false, checked: None, probing: false });

/// [`plays_714`] for the audio setting, which asks every frame it is shown: the last answer,
/// probed again in the background once it is older than `OFFER_FOR`.
pub fn offered() -> bool {
    let mut offer = lock(&OFFER);
    if !offer.probing && offer.checked.is_none_or(|at| at.elapsed() >= OFFER_FOR) {
        offer.probing = true;
        std::thread::spawn(|| {
            let plays = plays_714();
            let mut offer = lock(&OFFER);
            *offer = Offer { plays, checked: Some(Instant::now()), probing: false };
        });
    }
    offer.plays
}

/// What the render thread plays, under the lock `play` holds while it feeds it.
#[derive(Default)]
pub struct Queue {
    /// Interleaved 12-channel samples waiting for the device.
    pub pcm: VecDeque<f32>,
    /// What runs dry is concealed from (`Playout::starve`); none before playback starts.
    pub playout: Option<Arc<Mutex<Playout>>>,
}

/// Fills `out` (interleaved, `CHANNELS` per frame) from the queue, concealing what it lacks
/// first, and silence after that.
pub(super) fn fill(queue: &mut Queue, out: &mut [f32]) {
    let needed = out.len() / CHANNELS;
    let queued = queue.pcm.len() / CHANNELS;
    if queued < needed {
        if let Some(playout) = queue.playout.clone() {
            queue.pcm.extend(lock(&playout).starve(needed - queued));
        }
    }
    let n = out.len().min(queue.pcm.len());
    for (sample, queued) in out.iter_mut().zip(queue.pcm.drain(..n)) {
        *sample = queued;
    }
    out[n..].fill(0.0);
}

struct Event(HANDLE);

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: created in Device::open and closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

struct Device {
    client: IAudioClient,
    render: IAudioRenderClient,
    event: Event,
    /// Frames written per wake: the device period, as SDL's WASAPI backend writes.
    period: u32,
    buffer: u32,
}

impl Device {
    fn open() -> windows::core::Result<Self> {
        let client = default_client()?;
        let format = format();
        // SAFETY: COM is initialised on this thread; the format outlives the call.
        unsafe {
            let mut default_period = 0i64;
            client.GetDevicePeriod(Some(&raw mut default_period), None)?;
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                    | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                0,
                0,
                (&raw const format).cast::<WAVEFORMATEX>(),
                None,
            )?;
            let event = Event(CreateEventW(None, false, false, PCWSTR::null())?);
            client.SetEventHandle(event.0)?;
            let buffer = client.GetBufferSize()?;
            let render = client.GetService::<IAudioRenderClient>()?;
            // The period is in 100 ns units.
            let period = ((default_period as f64 / 10_000.0 * f64::from(RATE) / 1000.0).ceil() as u32).clamp(1, buffer);
            Ok(Self { client, render, event, period, buffer })
        }
    }

    /// Plays until `stop`, or until the device fails (unplugged, or the default output changed).
    fn render(&self, queue: &Mutex<Queue>, stop: &AtomicBool) -> windows::core::Result<()> {
        // SAFETY: the client and render client are this device's; the buffer WASAPI hands out
        // holds `frames` frames of the stream's format until it is released.
        unsafe {
            self.client.Start()?;
            let played = (|| {
                while !stop.load(Ordering::Relaxed) {
                    let waited = WaitForSingleObject(self.event.0, 200);
                    if waited == WAIT_TIMEOUT {
                        continue;
                    }
                    if waited != WAIT_OBJECT_0 {
                        return Err(windows::core::Error::from_thread());
                    }
                    let padding = self.client.GetCurrentPadding()?;
                    if padding > self.period {
                        continue;
                    }
                    let frames = self.period.min(self.buffer - padding);
                    let data = self.render.GetBuffer(frames)?;
                    let out = std::slice::from_raw_parts_mut(data.cast::<f32>(), frames as usize * CHANNELS);
                    fill(&mut lock(queue), out);
                    self.render.ReleaseBuffer(frames, 0)?;
                }
                Ok(())
            })();
            let _ = self.client.Stop();
            played
        }
    }
}

/// The render thread and what it plays.
pub struct Output {
    pub queue: Arc<Mutex<Queue>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// Frames the device takes at once, at 48 kHz.
    pub period_frames: usize,
}

impl Output {
    /// Opens the default output and starts playing silence until the queue has some.
    pub fn open() -> Result<Self, String> {
        let queue = Arc::new(Mutex::new(Queue::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (opened, first) = mpsc::channel();
        let thread = {
            let (queue, stop) = (queue.clone(), stop.clone());
            std::thread::Builder::new()
                .name("nightjar-wasapi".into())
                .spawn(move || run(&queue, &stop, opened))
                .map_err(|e| e.to_string())?
        };
        match first.recv() {
            Ok(Ok(period)) => Ok(Self { queue, stop, thread: Some(thread), period_frames: period as usize }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err("the output thread ended".into()),
        }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The render thread: opens the default output, and opens it again (the default output may have
/// changed) when it fails mid-stream, until stopped.
fn run(queue: &Mutex<Queue>, stop: &AtomicBool, opened: mpsc::Sender<Result<u32, String>>) {
    let _com = Com::init();
    // As SDL's audio threads: the Pro Audio task, or time-critical priority without it.
    let mut task_index = 0;
    // SAFETY: the task name is a static string; the handle is reverted below on this thread.
    let task = unsafe { AvSetMmThreadCharacteristicsW(windows::core::w!("Pro Audio"), &mut task_index) };
    if task.is_err() {
        // SAFETY: this thread's own pseudo handle.
        let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) };
    }
    let mut opened = Some(opened);
    let mut failing = false;
    while !stop.load(Ordering::Relaxed) {
        match Device::open() {
            Ok(device) => {
                match opened.take() {
                    Some(tx) => {
                        let _ = tx.send(Ok(device.period));
                    }
                    None => eprintln!("nightjar: audio: 7.1.4 output open again"),
                }
                failing = false;
                if let Err(e) = device.render(queue, stop) {
                    eprintln!("nightjar: audio: the 7.1.4 output failed ({e}); opening the default output again");
                }
            }
            Err(e) => {
                if let Some(tx) = opened.take() {
                    let _ = tx.send(Err(e.to_string()));
                    break;
                }
                if !failing {
                    eprintln!("nightjar: audio: no 7.1.4 output ({e}); trying again");
                    failing = true;
                }
            }
        }
        // Not straight back at a device that just failed.
        for _ in 0..5 {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    if let Ok(task) = task {
        // SAFETY: set above on this thread.
        let _ = unsafe { AvRevertMmThreadCharacteristics(task) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_format_is_twelve_float_channels_in_7_1_4_order() {
        let format = format();
        let header = format.Format;
        assert_eq!({ header.wFormatTag }, WAVE_FORMAT_EXTENSIBLE);
        assert_eq!(usize::from(header.nChannels), CHANNELS);
        assert_eq!({ header.nBlockAlign }, 48);
        assert_eq!({ header.nAvgBytesPerSec }, 48_000 * 48);
        assert_eq!({ header.cbSize }, 22);
        assert_eq!({ format.SubFormat }, SUBTYPE_IEEE_FLOAT);
        // Windows interleaves by mask bit: FL FR FC LFE BL BR SL SR TFL TFR TBL TBR, the
        // stream's order (KSAUDIO_SPEAKER_7POINT1POINT4).
        let order = [0x1, 0x2, 0x4, 0x8, 0x10, 0x20, 0x200, 0x400, 0x1000, 0x4000, 0x8000, 0x20000];
        let mask = { format.dwChannelMask };
        let bits: Vec<u32> = (0..32).map(|bit| 1u32 << bit).filter(|bit| mask & bit != 0).collect();
        assert_eq!(bits, order);
    }

    #[test]
    fn fill_keeps_the_interleaving_and_pads_with_silence() {
        let mut queue = Queue::default();
        // Two frames, each channel numbered by its place.
        queue.pcm.extend((0..2 * CHANNELS).map(|i| (i % CHANNELS) as f32 + 1.0));
        let mut out = vec![-1.0; 3 * CHANNELS];
        fill(&mut queue, &mut out);
        for (i, sample) in out[..2 * CHANNELS].iter().enumerate() {
            assert_eq!(*sample, (i % CHANNELS) as f32 + 1.0, "sample {i}");
        }
        assert!(out[2 * CHANNELS..].iter().all(|s| *s == 0.0), "nothing to conceal from: silence");
        assert!(queue.pcm.is_empty());
        // What the device does not take yet stays queued, in order.
        queue.pcm.extend((0..3 * CHANNELS).map(|i| i as f32));
        let mut one = vec![0.0; CHANNELS];
        fill(&mut queue, &mut one);
        assert_eq!(one, (0..CHANNELS).map(|i| i as f32).collect::<Vec<_>>());
        assert_eq!(queue.pcm.front(), Some(&(CHANNELS as f32)));
    }

    #[test]
    fn a_short_queue_is_concealed_from_the_playout_first() {
        use super::super::pipeline::AudioDecoder;
        let playout = Arc::new(Mutex::new(Playout::new(AudioDecoder::Pcm { channels: CHANNELS }, 5, 480)));
        let packet = vec![0x40u8; 2 * CHANNELS * 240];
        assert!(matches!(lock(&playout).packet(Some(&packet), 0), super::super::playout::Fed::Play { .. }));
        let mut queue = Queue { pcm: VecDeque::new(), playout: Some(playout.clone()) };
        let mut out = vec![1.0; 480 * CHANNELS];
        fill(&mut queue, &mut out);
        // PCM conceals with silence, but as whole packets the playout counts.
        assert_eq!(lock(&playout).take_concealed(), 2);
        assert!(out.iter().all(|s| *s == 0.0));
    }

    /// Opens this PC's default output with the 7.1.4 format and plays silence for a moment.
    #[test]
    #[ignore = "needs an audio output"]
    fn the_default_output_takes_the_7_1_4_format() {
        eprintln!("plays 7.1.4: {}", plays_714());
        let output = Output::open().expect("the default output opens");
        assert!(output.period_frames > 0 && output.period_frames <= 4800, "{}", output.period_frames);
        std::thread::sleep(Duration::from_millis(300));
        assert!(output.thread.as_ref().is_some_and(|t| !t.is_finished()), "still rendering");
        drop(output);
    }
}
