use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;

use sdl3_sys::everything::*;

use super::pipeline::{Notice, Shared};

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

impl Encoder {
    fn new() -> Result<Self, String> {
        let mut error = 0;
        let raw = unsafe { opus_encoder_create(48000, 1, 2048, &mut error) };
        NonNull::new(raw)
            .map(Self)
            .ok_or_else(|| format!("Microphone encoder failed ({error})."))
    }

    fn encode(&mut self, samples: &[f32; 480]) -> Result<Vec<u8>, String> {
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

pub struct Microphone {
    stream: NonNull<SDL_AudioStream>,
    _capture: Box<Capture>,
    pub packets: Receiver<Vec<u8>>,
}

impl Microphone {
    pub fn open(shared: Arc<Shared>) -> Result<Self, String> {
        let (sender, packets) = sync_channel(4);
        let mut capture = Box::new(Capture {
            encoder: Encoder::new()?,
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
                SDL_AUDIO_DEVICE_DEFAULT_RECORDING,
                &spec,
                Some(record),
                (&mut *capture as *mut Capture).cast(),
            )
        };
        let stream = NonNull::new(stream)
            .ok_or_else(|| format!("Could not open the microphone: {}", crate::sdl::error()))?;
        let microphone = Self {
            stream,
            _capture: capture,
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
        unsafe { SDL_DestroyAudioStream(self.stream.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microphone_packets_decode_as_ten_milliseconds_of_mono_audio() {
        let mut encoder = Encoder::new().unwrap();
        let mut decoder = crate::media::OpusDecoder::new(1, 1, 0, &[0]).unwrap();
        let samples = std::array::from_fn(|i| (i as f32 * 0.04).sin() * 0.1);
        let packet = encoder.encode(&samples).unwrap();
        let mut decoded = [0.0; 480];
        assert_eq!(decoder.decode(Some(&packet), &mut decoded), 480);
        assert!(decoded.iter().all(|sample| sample.is_finite()));
    }
}
