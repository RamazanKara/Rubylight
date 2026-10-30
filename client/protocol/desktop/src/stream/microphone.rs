use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;

use sdl3_sys::everything::*;

use super::pipeline::{Notice, Shared};
use crate::config::MicEcho;

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

pub struct Microphone {
    stream: Option<NonNull<SDL_AudioStream>>,
    _capture: Option<Box<Capture>>,
    #[cfg(windows)]
    _voice: Option<super::voice_capture::VoiceCapture>,
    pub packets: Receiver<Vec<u8>>,
}

impl Microphone {
    /// Opens the microphone with echo cancellation as `echo` asks, where this system has it.
    pub fn open(shared: Arc<Shared>, echo: MicEcho) -> Result<Self, String> {
        #[cfg(windows)]
        let private_output = echo == MicEcho::Auto && super::voice_capture::private_output();
        #[cfg(not(windows))]
        let private_output = false;
        let wanted = echo.wanted(private_output);
        let devices = if wanted && !cfg!(windows) { recording_devices() } else { Vec::new() };
        let names: Vec<String> = devices.iter().map(|(_, name)| name.clone()).collect();
        match plan(wanted, cfg!(windows), &names) {
            Source::VoiceCapture => {
                #[cfg(windows)]
                match Self::open_voice(shared.clone()) {
                    Ok(microphone) => return Ok(microphone),
                    Err(error) => eprintln!("nightjar: Windows' echo cancellation cannot run ({error}); the microphone is sent without it"),
                }
            }
            Source::EchoCanceller(index) => {
                let (id, name) = &devices[index];
                eprintln!("nightjar: the microphone records from {name:?}, the echo-cancel source");
                return Self::open_sdl(shared, *id);
            }
            Source::Default if wanted => eprintln!(
                "nightjar: the microphone is sent without echo cancellation: no echo-cancel source (PipeWire's libpipewire-module-echo-cancel or PulseAudio's module-echo-cancel)"
            ),
            Source::Default => {}
        }
        Self::open_sdl(shared, SDL_AUDIO_DEVICE_DEFAULT_RECORDING)
    }

    #[cfg(windows)]
    fn open_voice(shared: Arc<Shared>) -> Result<Self, String> {
        let (sender, packets) = sync_channel(4);
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
        Ok(Self { stream: None, _capture: None, _voice: Some(voice), packets })
    }

    fn open_sdl(shared: Arc<Shared>, device: SDL_AudioDeviceID) -> Result<Self, String> {
        let (sender, packets) = sync_channel(4);
        let mut capture = Box::new(Capture {
            encoder: Encoder::new(48000)?,
            sender,
            shared,
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
        let microphone = Self {
            stream: Some(stream),
            _capture: Some(capture),
            #[cfg(windows)]
            _voice: None,
            packets,
        };
        if !unsafe { SDL_ResumeAudioStreamDevice(stream.as_ptr()) } {
            return Err(format!(
                "Could not start the microphone: {}",
                crate::sdl::error()
            ));
        }
        Ok(microphone)
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
    fn automatic_echo_cancellation_stays_off_only_for_headphones() {
        assert_eq!(MicEcho::default(), MicEcho::Auto, "on by default with the microphone, as on Android");
        assert!(MicEcho::Auto.wanted(false) && !MicEcho::Auto.wanted(true));
        assert!(MicEcho::On.wanted(true) && !MicEcho::Off.wanted(false));
    }
}
