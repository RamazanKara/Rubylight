//! Echo-cancelled microphone capture on Windows: the Voice Capture DSP (CLSID_CWMAudioAEC) in
//! source mode records the default microphone and takes the default speakers' output out of it,
//! the system echo canceller Windows has had since Vista. Its output is 16 kHz mono PCM, which the
//! stream's Opus encoder takes as it is (an Opus decoder at 48 kHz decodes it like any packet).

use std::cell::{Cell, UnsafeCell};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use windows::core::{implement, Interface, GUID};
use windows::Win32::Foundation::{E_INVALIDARG, PROPERTYKEY};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, EDataFlow, IAudioSessionControl2, IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, DEVICE_STATE_ACTIVE, PKEY_AudioEndpoint_FormFactor, WAVEFORMATEX, WAVE_FORMAT_PCM, Headphones, Headset,
};
use windows::Win32::Media::DxMediaObjects::{
    IMediaBuffer, IMediaBuffer_Impl, IMediaObject, DMO_MEDIA_TYPE, DMO_OUTPUT_DATA_BUFFER, DMO_OUTPUT_DATA_BUFFERF_INCOMPLETE,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, STGM_READ};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

/// The DSP's output rate; it gives 8, 11.025, 16 or 22.05 kHz.
pub const RATE: u32 = 16_000;
const CLSID_CWMAUDIOAEC: GUID = GUID::from_u128(0x745057c7_f353_4f2d_a7ee_58434477730e);
const MEDIATYPE_AUDIO: GUID = GUID::from_u128(0x73647561_0000_0010_8000_00aa00389b71);
const MEDIASUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
const FORMAT_WAVEFORMATEX: GUID = GUID::from_u128(0x05589f81_c356_11ce_bf01_00aa0055595a);
/// wmcodecdsp.h's MFPKEY_WMAAECMA_* keys share this format id.
const WMAAECMA: GUID = GUID::from_u128(0x6f52c567_0360_4bd2_9617_ccbf1421c939);
const SYSTEM_MODE: PROPERTYKEY = PROPERTYKEY { fmtid: WMAAECMA, pid: 2 };
const DEVICE_INDEXES: PROPERTYKEY = PROPERTYKEY { fmtid: WMAAECMA, pid: 4 };
/// AEC_SYSTEM_MODE's SINGLE_CHANNEL_AEC: one microphone, echo cancellation on.
const SINGLE_CHANNEL_AEC: i32 = 0;

/// MFPKEY_WMAAECMA_DEVICE_INDEXES's value: the speakers' index in the high word, the
/// microphone's in the low word, each in `EnumAudioEndpoints`'s order of active endpoints.
pub fn device_indexes(render: u32, capture: u32) -> i32 {
    ((render << 16) | (capture & 0xffff)) as i32
}

/// The default playback device keeps the stream's sound out of the microphone: headphones or a
/// headset, as the endpoint's form factor says. False when Windows cannot say.
pub fn private_output() -> bool {
    let _com = Com::init();
    // SAFETY: COM calls on this thread's apartment.
    unsafe {
        let result = (|| -> windows::core::Result<bool> {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let speakers = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let form = i32::try_from(&speakers.OpenPropertyStore(STGM_READ)?.GetValue(&PKEY_AudioEndpoint_FormFactor)?)?;
            Ok(form == Headphones.0 || form == Headset.0)
        })();
        result.unwrap_or_else(|error| {
            eprintln!("nightjar: cannot tell whether the sound plays through headphones ({error}); echo cancellation stays on");
            false
        })
    }
}

struct Com(bool);

impl Com {
    fn init() -> Self {
        // SAFETY: balanced by Drop on the same thread when it succeeded.
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: CoInitializeEx succeeded on this thread.
            unsafe { CoUninitialize() };
        }
    }
}

/// The DMO's output buffer: the caller owns the memory (IMediaBuffer).
#[implement(IMediaBuffer)]
struct Buffer {
    data: UnsafeCell<Vec<u8>>,
    length: Cell<u32>,
}

impl IMediaBuffer_Impl for Buffer_Impl {
    fn SetLength(&self, length: u32) -> windows::core::Result<()> {
        // SAFETY: only this thread touches the buffer.
        if length as usize > unsafe { &*self.data.get() }.len() {
            return Err(E_INVALIDARG.into());
        }
        self.length.set(length);
        Ok(())
    }

    fn GetMaxLength(&self) -> windows::core::Result<u32> {
        // SAFETY: as above.
        Ok(unsafe { &*self.data.get() }.len() as u32)
    }

    fn GetBufferAndLength(&self, buffer: *mut *mut u8, length: *mut u32) -> windows::core::Result<()> {
        // SAFETY: the DMO passes valid out pointers or null.
        unsafe {
            if !buffer.is_null() {
                *buffer = (*self.data.get()).as_mut_ptr();
            }
            if !length.is_null() {
                *length = self.length.get();
            }
        }
        Ok(())
    }
}

/// The index of the default endpoint of `flow` among the active ones.
unsafe fn default_index(enumerator: &IMMDeviceEnumerator, flow: EDataFlow) -> windows::core::Result<(u32, IMMDevice)> {
    let id = |device: &IMMDevice| -> windows::core::Result<String> {
        let raw = device.GetId()?;
        let text = raw.to_string();
        CoTaskMemFree(Some(raw.0 as *const _));
        text.map_err(|_| E_INVALIDARG.into())
    };
    let default = enumerator.GetDefaultAudioEndpoint(flow, eConsole)?;
    let wanted = id(&default)?;
    let all = enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
    for index in 0..all.GetCount()? {
        if id(&all.Item(index)?)? == wanted {
            return Ok((index, default));
        }
    }
    Err(windows::core::Error::new(E_INVALIDARG, "the default device is not among the active ones"))
}

/// Opts this process's session on `device`, the speakers the stream plays on, out of Windows'
/// ducking, which would turn the stream's sound down by 80% while a communications capture runs
/// (the Sound panel's default). Ducking applies to playback sessions, so it is the render device's.
unsafe fn keep_other_sounds(device: &IMMDevice) -> windows::core::Result<()> {
    let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
    manager.GetAudioSessionControl(None, 0)?.cast::<IAudioSessionControl2>()?.SetDuckingPreference(true)
}

/// Sets up the DSP for the default microphone and speakers, 16 kHz mono 16-bit output.
unsafe fn open_dsp() -> windows::core::Result<IMediaObject> {
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let (capture, _) = default_index(&enumerator, eCapture)?;
    let (render, speakers) = default_index(&enumerator, eRender)?;
    if let Err(error) = keep_other_sounds(&speakers) {
        eprintln!("nightjar: Windows may turn the stream's sound down while the microphone runs ({error})");
    }
    let dsp: IMediaObject = CoCreateInstance(&CLSID_CWMAUDIOAEC, None, CLSCTX_INPROC_SERVER)?;
    let properties: IPropertyStore = dsp.cast()?;
    properties.SetValue(&SYSTEM_MODE, &PROPVARIANT::from(SINGLE_CHANNEL_AEC))?;
    properties.SetValue(&DEVICE_INDEXES, &PROPVARIANT::from(device_indexes(render, capture)))?;
    let mut format = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM as u16,
        nChannels: 1,
        nSamplesPerSec: RATE,
        nAvgBytesPerSec: RATE * 2,
        nBlockAlign: 2,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    let media_type = DMO_MEDIA_TYPE {
        majortype: MEDIATYPE_AUDIO,
        subtype: MEDIASUBTYPE_PCM,
        bFixedSizeSamples: true.into(),
        bTemporalCompression: false.into(),
        lSampleSize: 2,
        formattype: FORMAT_WAVEFORMATEX,
        pUnk: std::mem::ManuallyDrop::new(None),
        cbFormat: std::mem::size_of::<WAVEFORMATEX>() as u32,
        pbFormat: (&mut format as *mut WAVEFORMATEX).cast(),
    };
    dsp.SetOutputType(0, Some(&media_type), 0)?;
    dsp.AllocateStreamingResources()?;
    Ok(dsp)
}

/// The capture thread: started once the DSP is set up, stopped and joined on drop.
pub struct VoiceCapture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl VoiceCapture {
    /// Starts capturing; `frames` gets the DSP's output in 10 ms frames until it returns false,
    /// and `failed` once if the DSP fails while running.
    pub fn start(
        mut frames: impl FnMut(&[f32]) -> bool + Send + 'static,
        failed: impl FnOnce(String) + Send + 'static,
    ) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let thread_stop = stop.clone();
        let thread = std::thread::Builder::new()
            .name("nightjar-voice-capture".into())
            .spawn(move || {
                let _com = Com::init();
                // SAFETY: the DSP lives and is used on this thread only.
                let dsp = match unsafe { open_dsp() } {
                    Ok(dsp) => {
                        let _ = ready_tx.send(Ok(()));
                        dsp
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.to_string()));
                        return;
                    }
                };
                // 100 ms of output, more than one call ever gives.
                let buffer: IMediaBuffer = Buffer { data: UnsafeCell::new(vec![0; RATE as usize / 5]), length: Cell::new(0) }.into();
                let frame = RATE as usize / 100;
                let mut samples: Vec<f32> = Vec::with_capacity(frame * 4);
                let error = 'run: loop {
                    if thread_stop.load(Ordering::Relaxed) {
                        break None;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                    loop {
                        let mut output = [DMO_OUTPUT_DATA_BUFFER {
                            pBuffer: std::mem::ManuallyDrop::new(Some(buffer.clone())),
                            dwStatus: 0,
                            rtTimestamp: 0,
                            rtTimelength: 0,
                        }];
                        let mut status = 0;
                        // SAFETY: the output buffer and status outlive the call.
                        let result = unsafe { dsp.ProcessOutput(0, &mut output, &mut status) };
                        let more = output[0].dwStatus & DMO_OUTPUT_DATA_BUFFERF_INCOMPLETE.0 as u32 != 0;
                        // SAFETY: the reference given to the DMO above.
                        unsafe { std::mem::ManuallyDrop::drop(&mut output[0].pBuffer) };
                        if let Err(error) = result {
                            break 'run Some(format!("Microphone echo cancellation failed: {error}"));
                        }
                        let (mut data, mut length) = (std::ptr::null_mut(), 0);
                        // SAFETY: our own buffer, read on this thread.
                        unsafe {
                            let _ = buffer.GetBufferAndLength(Some(&mut data), Some(&mut length));
                            let pcm = std::slice::from_raw_parts(data.cast::<i16>(), length as usize / 2);
                            samples.extend(pcm.iter().map(|&s| f32::from(s) / 32768.0));
                            let _ = buffer.SetLength(0);
                        }
                        if !more {
                            break;
                        }
                    }
                    let mut start = 0;
                    while samples.len() - start >= frame {
                        if !frames(&samples[start..start + frame]) {
                            break 'run None;
                        }
                        start += frame;
                    }
                    samples.drain(..start);
                };
                // SAFETY: as above.
                let _ = unsafe { dsp.FreeStreamingResources() };
                if let Some(error) = error {
                    failed(error);
                }
            })
            .map_err(|error| format!("Could not start the microphone: {error}"))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { stop, thread: Some(thread) }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err("the voice capture thread ended before it started".into())
            }
        }
    }
}

impl Drop for VoiceCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_indexes_put_the_speakers_high_and_the_microphone_low() {
        assert_eq!(device_indexes(0, 0), 0);
        assert_eq!(device_indexes(2, 1), 0x0002_0001);
        assert_eq!(device_indexes(1, 3), 0x0001_0003);
    }

    /// Opens the DSP on this PC's default devices for half a second: needs a microphone and
    /// speakers, so it runs only on request (`--ignored`).
    #[test]
    #[ignore]
    fn the_voice_capture_dsp_opens_and_gives_ten_millisecond_frames() {
        let (tx, rx) = std::sync::mpsc::channel();
        let capture = VoiceCapture::start(move |frame| tx.send(frame.len()).is_ok(), |error| panic!("{error}")).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        drop(capture);
        let lengths: Vec<usize> = rx.try_iter().collect();
        assert!(lengths.len() >= 20, "{} frames in 500 ms", lengths.len());
        assert!(lengths.iter().all(|&n| n == RATE as usize / 100));
    }
}
