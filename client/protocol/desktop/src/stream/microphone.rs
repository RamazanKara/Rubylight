use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;

use sdl3_sys::everything::*;

use super::pipeline::{Notice, Shared};
use crate::config::MicEcho;
use crate::strings::tr;

extern "C" {
    fn opus_encoder_create(
        rate: i32,
        channels: i32,
        application: i32,
        error: *mut i32,
    ) -> *mut c_void;
    fn opus_encoder_destroy(encoder: *mut c_void);
    fn opus_encode_float(
        encoder: *mut c_void,
        pcm: *const f32,
        samples: i32,
        data: *mut u8,
        length: i32,
    ) -> i32;
}

struct Encoder(NonNull<c_void>);

// SAFETY: an Opus encoder is plain memory, used by one thread at a time.
unsafe impl Send for Encoder {}

impl Encoder {
    /// A mono VoIP encoder taking 10 ms frames at `rate` (48 kHz from SDL, 16 kHz from Windows'
    /// echo canceller); the PC's decoder runs at 48 kHz whatever rate the packets were made at.
    fn new(rate: i32) -> Result<Self, String> {
        let mut error = 0;
        let raw = unsafe { opus_encoder_create(rate, 1, 2048, &mut error) };
        NonNull::new(raw)
            .map(Self)
            .ok_or_else(|| format!("Microphone encoder failed ({error})."))
    }

    fn encode(&mut self, samples: &[f32]) -> Result<Vec<u8>, String> {
        let mut packet = vec![0; 1275];
        let length = unsafe {
            opus_encode_float(
                self.0.as_ptr(),
                samples.as_ptr(),
                samples.len() as i32,
                packet.as_mut_ptr(),
                packet.len() as i32,
            )
        };
        if length < 0 {
            return Err(format!("Microphone encoding failed ({length})."));
        }
        packet.truncate(length as usize);
        Ok(packet)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { opus_encoder_destroy(self.0.as_ptr()) };
    }
}

struct Capture {
    encoder: Encoder,
    sender: SyncSender<Vec<u8>>,
    shared: Arc<Shared>,
    failed: bool,
}

unsafe extern "C" fn record(userdata: *mut c_void, stream: *mut SDL_AudioStream, _: i32, _: i32) {
    // SDL serializes callbacks and DestroyAudioStream waits for the current one.
    let capture = &mut *userdata.cast::<Capture>();
    if capture.failed {
        return;
    }
    let mut samples = [0f32; 480];
    let bytes = std::mem::size_of_val(&samples) as i32;
    while SDL_GetAudioStreamAvailable(stream) >= bytes {
        let result = if SDL_GetAudioStreamData(stream, samples.as_mut_ptr().cast(), bytes) != bytes
        {
            Err(format!(
                "Microphone capture failed: {}",
                crate::sdl::error()
            ))
        } else {
            capture.encoder.encode(&samples)
        };
        match result {
            Ok(packet) => {
                if capture.sender.try_send(packet).is_ok() {
                    capture.shared.wake();
                }
            }
            Err(error) => {
                capture.failed = true;
                capture.shared.notify(Notice::MicrophoneFailed(error));
                break;
            }
        }
    }
}

/// Where the microphone's audio comes from (`plan`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The default recording device as it is.
    Default,
    /// Windows' Voice Capture DSP: the default microphone with the default speakers' sound taken
    /// out (`voice_capture`).
    VoiceCapture,
    /// The recording device at this index, which PipeWire's or PulseAudio's echo-cancel module
    /// made: the default microphone with the default output's sound taken out.
    EchoCanceller(usize),
}

/// Whether a recording device is an echo-cancel module's source: PipeWire's is "Echo-Cancel
/// Source", PulseAudio's "… (echo cancelled with …)".
fn is_echo_cancel_source(name: &str) -> bool {
    let name = name.to_lowercase();
    ["echo-cancel", "echo cancel"].iter().any(|mark| name.contains(mark))
}

/// The capture for `echo_wanted` (`MicEcho::wanted`): Windows' echo canceller there, elsewhere an
/// echo-cancel source among the `recording` devices, else the default device without one (the
/// caller says so in the log).
pub fn plan(echo_wanted: bool, windows: bool, recording: &[String]) -> Source {
    if !echo_wanted {
        Source::Default
    } else if windows {
        Source::VoiceCapture
    } else {
        recording.iter().position(|name| is_echo_cancel_source(name)).map_or(Source::Default, Source::EchoCanceller)
    }
}

/// SDL's recording devices and their names.
fn recording_devices() -> Vec<(SDL_AudioDeviceID, String)> {
    let mut count = 0;
    // SAFETY: SDL returns an array of `count` ids or null; it is freed below.
    unsafe {
        let ids = SDL_GetAudioRecordingDevices(&mut count);
        if ids.is_null() {
            return Vec::new();
        }
        let devices = std::slice::from_raw_parts(ids, count.max(0) as usize)
            .iter()
            .map(|&id| {
                let name = SDL_GetAudioDeviceName(id);
                (id, if name.is_null() { String::new() } else { std::ffi::CStr::from_ptr(name).to_string_lossy().into_owned() })
            })
            .collect();
        SDL_free(ids.cast());
        devices
    }
}

/// Says, once a stream, that the microphone is sent without the echo cancellation that was asked
/// for: a toast, besides the log line.
fn without_echo_cancellation(shared: &Shared, text: String) {
    if !shared.echo_cancel_noted.swap(true, Ordering::Relaxed) {
        shared.notify(Notice::EchoCancelOff(text));
    }
}

/// Windows: what opening the echo canceller came to, on its own thread.
#[cfg(windows)]
enum Opened {
    Voice(super::voice_capture::VoiceCapture),
    /// The plain microphone instead: echo cancellation not wanted (headphones), or, with the
    /// reason, it could not run.
    Plain(Option<String>),
}

#[cfg(windows)]
enum Voice {
    None,
    /// Asking Windows about the speakers and opening the DSP take a moment; this answers.
    Opening(Receiver<Opened>),
    /// Runs until dropped.
    Running { _capture: super::voice_capture::VoiceCapture },
}

pub struct Microphone {
    stream: Option<NonNull<SDL_AudioStream>>,
    _capture: Option<Box<Capture>>,
    #[cfg(windows)]
    voice: Voice,
    shared: Arc<Shared>,
    sender: SyncSender<Vec<u8>>,
    pub packets: Receiver<Vec<u8>>,
}

impl Microphone {
    fn new(shared: Arc<Shared>) -> Self {
        let (sender, packets) = sync_channel(4);
        Self {
            stream: None,
            _capture: None,
            #[cfg(windows)]
            voice: Voice::None,
            shared,
            sender,
            packets,
        }
    }

    /// Opens the microphone with echo cancellation as `echo` asks, where this system has it. On
    /// Windows the echo canceller opens on a thread of its own, so the caller (the render thread,
    /// when the microphone is turned on mid-stream) never waits for it: [`Self::poll`] takes its
    /// answer.
    pub fn open(shared: Arc<Shared>, echo: MicEcho) -> Result<Self, String> {
        #[cfg(windows)]
        if echo != MicEcho::Off {
            return Self::open_voice(shared, echo);
        }
        let wanted = !cfg!(windows) && echo.wanted(false);
        let devices = if wanted { recording_devices() } else { Vec::new() };
        let names: Vec<String> = devices.iter().map(|(_, name)| name.clone()).collect();
        let mut microphone = Self::new(shared);
        match plan(wanted, false, &names) {
            Source::EchoCanceller(index) => {
                let (id, name) = &devices[index];
                eprintln!("nightjar: the microphone records from {name:?}, the echo-cancel source");
                microphone.start_sdl(*id)?;
                return Ok(microphone);
            }
            Source::Default if wanted => {
                eprintln!("nightjar: the microphone is sent without echo cancellation: no echo-cancel source (PipeWire's libpipewire-module-echo-cancel or PulseAudio's module-echo-cancel)");
                without_echo_cancellation(&microphone.shared, tr("No echo cancellation: this system has no echo-cancel source. Load PipeWire's or PulseAudio's echo-cancel module to have one.").to_owned());
            }
            Source::Default | Source::VoiceCapture => {}
        }
        microphone.start_sdl(SDL_AUDIO_DEVICE_DEFAULT_RECORDING)?;
        Ok(microphone)
    }

    /// Takes the answer of the echo canceller's opening (Windows): it runs from now on, or the
    /// plain microphone starts. Call before reading `packets`; an error ends the microphone.
    pub fn poll(&mut self) -> Result<(), String> {
        #[cfg(windows)]
        if let Voice::Opening(answer) = &self.voice {
            match answer.try_recv() {
                Ok(Opened::Voice(voice)) => self.voice = Voice::Running { _capture: voice },
                Ok(Opened::Plain(why)) => {
                    self.voice = Voice::None;
                    if let Some(why) = why {
                        without_echo_cancellation(&self.shared, crate::strings::trf("Echo cancellation could not start: {}. The microphone is sent without it.", &[&why]));
                    }
                    self.start_sdl(SDL_AUDIO_DEVICE_DEFAULT_RECORDING)?;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.voice = Voice::None;
                    return Err("The microphone's echo cancellation ended before it started.".into());
                }
            }
        }
        Ok(())
    }

    #[cfg(windows)]
    fn open_voice(shared: Arc<Shared>, echo: MicEcho) -> Result<Self, String> {
        let mut microphone = Self::new(shared.clone());
        let (answer, opening) = std::sync::mpsc::channel();
        let sender = microphone.sender.clone();
        let spawned = std::thread::Builder::new().name("nightjar-mic-open".into()).spawn(move || {
            let wanted = echo.wanted(echo == MicEcho::Auto && super::voice_capture::private_output());
            let opened = if plan(wanted, true, &[]) == Source::VoiceCapture {
                match Self::start_voice(shared.clone(), sender) {
                    Ok(voice) => Opened::Voice(voice),
                    Err(error) => {
                        eprintln!("nightjar: Windows' echo cancellation cannot run ({error}); the microphone is sent without it");
                        Opened::Plain(Some(error))
                    }
                }
            } else {
                Opened::Plain(None)
            };
            // A microphone turned off meanwhile is gone: its capture stops with this answer.
            if answer.send(opened).is_ok() {
                shared.wake();
            }
        });
        match spawned {
            Ok(_) => microphone.voice = Voice::Opening(opening),
            Err(error) => {
                eprintln!("nightjar: Windows' echo cancellation cannot start a thread ({error}); the microphone is sent without it");
                without_echo_cancellation(&microphone.shared, crate::strings::trf("Echo cancellation could not start: {}. The microphone is sent without it.", &[&error]));
                microphone.start_sdl(SDL_AUDIO_DEVICE_DEFAULT_RECORDING)?;
            }
        }
        Ok(microphone)
    }

    #[cfg(windows)]
    fn start_voice(shared: Arc<Shared>, sender: SyncSender<Vec<u8>>) -> Result<super::voice_capture::VoiceCapture, String> {
        let mut encoder = Encoder::new(super::voice_capture::RATE as i32)?;
        let frames_shared = shared.clone();
        let frames = move |samples: &[f32]| match encoder.encode(samples) {
            Ok(packet) => {
                if sender.try_send(packet).is_ok() {
                    frames_shared.wake();
                }
                true
            }
            Err(error) => {
                frames_shared.notify(Notice::MicrophoneFailed(error));
                false
            }
        };
        let voice = super::voice_capture::VoiceCapture::start(frames, move |error| shared.notify(Notice::MicrophoneFailed(error)))?;
        eprintln!("nightjar: the microphone runs through Windows' echo cancellation");
        Ok(voice)
    }

    /// Records `device` through SDL into `packets`.
    fn start_sdl(&mut self, device: SDL_AudioDeviceID) -> Result<(), String> {
        let mut capture = Box::new(Capture {
            encoder: Encoder::new(48000)?,
            sender: self.sender.clone(),
            shared: self.shared.clone(),
            failed: false,
        });
        let spec = SDL_AudioSpec {
            format: SDL_AUDIO_F32,
            channels: 1,
            freq: 48000,
        };
        let stream = unsafe {
            SDL_OpenAudioDeviceStream(
                device,
                &spec,
                Some(record),
                (&mut *capture as *mut Capture).cast(),
            )
        };
        let stream = NonNull::new(stream)
            .ok_or_else(|| format!("Could not open the microphone: {}", crate::sdl::error()))?;
        // Kept before resuming: Drop destroys the stream whatever happens next.
        (self.stream, self._capture) = (Some(stream), Some(capture));
        if !unsafe { SDL_ResumeAudioStreamDevice(stream.as_ptr()) } {
            return Err(format!(
                "Could not start the microphone: {}",
                crate::sdl::error()
            ));
        }
        Ok(())
    }
}

impl Drop for Microphone {
    fn drop(&mut self) {
        if let Some(stream) = self.stream {
            unsafe { SDL_DestroyAudioStream(stream.as_ptr()) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microphone_packets_decode_as_ten_milliseconds_of_mono_audio() {
        let mut encoder = Encoder::new(48000).unwrap();
        let mut decoder = crate::media::OpusDecoder::new(1, 1, 0, &[0]).unwrap();
        let samples: [f32; 480] = std::array::from_fn(|i| (i as f32 * 0.04).sin() * 0.1);
        let packet = encoder.encode(&samples).unwrap();
        let mut decoded = [0.0; 480];
        assert_eq!(decoder.decode(Some(&packet), &mut decoded), 480);
        assert!(decoded.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn echo_cancelled_sixteen_kilohertz_packets_decode_as_ten_milliseconds_at_48_khz() {
        let mut encoder = Encoder::new(16000).unwrap();
        let mut decoder = crate::media::OpusDecoder::new(1, 1, 0, &[0]).unwrap();
        let samples: [f32; 160] = std::array::from_fn(|i| (i as f32 * 0.12).sin() * 0.1);
        let packet = encoder.encode(&samples).unwrap();
        let mut decoded = [0.0; 480];
        assert_eq!(decoder.decode(Some(&packet), &mut decoded), 480);
    }

    #[test]
    fn echo_cancellation_picks_the_system_canceller_or_says_there_is_none() {
        let names = |list: &[&str]| list.iter().map(|name| name.to_string()).collect::<Vec<_>>();
        let plain = names(&["Built-in Audio Analog Stereo", "USB Microphone"]);
        let pipewire = names(&["Built-in Audio Analog Stereo", "Echo-Cancel Source"]);
        let pulse = names(&["Built-in Audio Analog Stereo (echo cancelled with Built-in Audio Analog Stereo)"]);
        assert_eq!(plan(true, true, &[]), Source::VoiceCapture, "Windows: the Voice Capture DSP");
        assert_eq!(plan(false, true, &[]), Source::Default, "off, or automatic with headphones");
        assert_eq!(plan(true, false, &pipewire), Source::EchoCanceller(1));
        assert_eq!(plan(true, false, &pulse), Source::EchoCanceller(0));
        assert_eq!(plan(true, false, &plain), Source::Default, "no echo-cancel source: none, said in the log");
        assert_eq!(plan(false, false, &pipewire), Source::Default, "off records the default device as it is");
    }

    #[test]
    fn a_microphone_without_the_asked_echo_cancellation_says_so_once_a_stream() {
        let shared = Shared::new(Arc::new(|| {}), 2);
        without_echo_cancellation(&shared, "first".into());
        without_echo_cancellation(&shared, "again, the microphone turned on a second time".into());
        let notices = std::mem::take(&mut *shared.notices.lock().unwrap());
        assert!(matches!(&notices[..], [Notice::EchoCancelOff(text)] if text == "first"), "one toast a stream");
        let next = Shared::new(Arc::new(|| {}), 2);
        without_echo_cancellation(&next, "next stream".into());
        assert_eq!(next.notices.lock().unwrap().len(), 1, "the next stream says it again");
    }

    #[test]
    fn automatic_echo_cancellation_stays_off_only_for_headphones() {
        assert_eq!(MicEcho::default(), MicEcho::Auto, "on by default with the microphone, as on Android");
        assert!(MicEcho::Auto.wanted(false) && !MicEcho::Auto.wanted(true));
        assert!(MicEcho::On.wanted(true) && !MicEcho::Off.wanted(false));
    }
}
