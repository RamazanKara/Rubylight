//! FFmpeg's AC3 and E-AC3 encoders for audio passthrough, loaded at run time from
//! `avutil-60.dll` and `avcodec-62.dll` beside the host: an LGPL build of nothing but those
//! codecs and their decoders (`rust/tools/build_ffmpeg.sh`). FFmpeg keeps its structs' layout
//! within a major version, so both majors are checked before any field is touched. The codec
//! context is set through its AVOptions where it has them; the rest of it, frames and packets
//! use FFmpeg 8's field offsets.
use anyhow::{Context, Result, bail};
use std::{
    ffi::{CStr, c_char, c_int, c_void},
    path::{Path, PathBuf},
};

/// avcodec imports avutil, so avutil loads first and Windows reuses it for avcodec.
const AVUTIL: &str = "avutil-60.dll";
const AVCODEC: &str = "avcodec-62.dll";
const AVUTIL_MAJOR: u32 = 60;
const AVCODEC_MAJOR: u32 = 62;
/// `AV_SAMPLE_FMT_FLTP`: what the AC3 and E-AC3 encoders take and their decoders give.
const SAMPLE_FMT_FLTP: c_int = 8;
/// `AVERROR(EAGAIN)`: the codec wants more input before it has output.
const AGAIN: c_int = -11;
/// Samples per AC3 and E-AC3 frame.
pub const FRAME_SAMPLES: usize = 1536;
/// Field offsets in FFmpeg 8's `AVCodecContext`, `AVFrame` and `AVPacket` (avutil 60,
/// avcodec 62).
const CONTEXT_SAMPLE_FMT: usize = 348;
const CONTEXT_FRAME_SIZE: usize = 376;
const FRAME_DATA: usize = 0;
const FRAME_NB_SAMPLES: usize = 112;
const FRAME_FORMAT: usize = 116;
const FRAME_SAMPLE_RATE: usize = 180;
const FRAME_CH_LAYOUT: usize = 384;
const PACKET_DATA: usize = 24;
const PACKET_SIZE: usize = 32;

/// `AVChannelLayout`.
#[repr(C)]
struct ChannelLayout {
    order: c_int,
    nb_channels: c_int,
    mask: u64,
    opaque: *mut c_void,
}

/// The FFmpeg channel mask for a stream's channel count: stereo, 5.1 and 7.1 in the capture's
/// interleaved WAVEFORMATEXTENSIBLE order, which is also FFmpeg's native order.
fn channel_mask(channels: u8) -> Option<u64> {
    match channels {
        2 => Some(0x3),
        6 => Some(0x3F),
        8 => Some(0x63F),
        _ => None,
    }
}

/// The absolute path of `name` in `directory`, which `LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR` needs.
fn beside(directory: &Path, name: &str) -> Result<PathBuf> {
    std::path::absolute(directory.join(name))
        .with_context(|| format!("locating {name} in {}", directory.display()))
}

/// Loads the library at the absolute `path`, its imports (avcodec's avutil) from its own folder
/// and System32 only: never from the current directory or `PATH`, where another FFmpeg or a
/// planted DLL could stand in.
///
/// # Safety
/// Loading runs the library's initialisers.
unsafe fn load_beside(path: &Path) -> Result<libloading::Library> {
    use windows::Win32::System::LibraryLoader::{
        LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
    };
    let flags = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR.0 | LOAD_LIBRARY_SEARCH_SYSTEM32.0;
    // SAFETY: the caller accepts the library's initialisers; `path` is absolute.
    Ok(unsafe { libloading::os::windows::Library::load_with_flags(path, flags)? }.into())
}

type Version = unsafe extern "C" fn() -> u32;
type Alloc = unsafe extern "C" fn() -> *mut c_void;
type Free = unsafe extern "C" fn(*mut *mut c_void);
type Call = unsafe extern "C" fn(*mut c_void) -> c_int;
type CallWith = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
type Exchange = unsafe extern "C" fn(*mut c_void, *const c_void) -> c_int;
type FindCodec = unsafe extern "C" fn(*const c_char) -> *const c_void;
type AllocContext = unsafe extern "C" fn(*const c_void) -> *mut c_void;
type Open = unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void) -> c_int;
type LayoutFromMask = unsafe extern "C" fn(*mut ChannelLayout, u64) -> c_int;
type LayoutCopy = unsafe extern "C" fn(*mut c_void, *const ChannelLayout) -> c_int;
type SetInt = unsafe extern "C" fn(*mut c_void, *const c_char, i64, c_int) -> c_int;
type SetLayout =
    unsafe extern "C" fn(*mut c_void, *const c_char, *const ChannelLayout, c_int) -> c_int;
type StrError = unsafe extern "C" fn(c_int, *mut c_char, usize) -> c_int;
type Unref = unsafe extern "C" fn(*mut c_void);

/// The FFmpeg functions this module calls, and the libraries that hold them.
struct Library {
    frame_alloc: Alloc,
    frame_free: Free,
    frame_get_buffer: CallWith,
    frame_make_writable: Call,
    layout_from_mask: LayoutFromMask,
    layout_copy: LayoutCopy,
    set_int: SetInt,
    set_layout: SetLayout,
    strerror: StrError,
    find_encoder: FindCodec,
    #[cfg(test)]
    find_decoder: FindCodec,
    alloc_context: AllocContext,
    free_context: Free,
    open: Open,
    send_frame: Exchange,
    receive_packet: Exchange,
    #[cfg(test)]
    send_packet: Exchange,
    #[cfg(test)]
    receive_frame: Exchange,
    #[cfg(test)]
    new_packet: CallWith,
    packet_alloc: Alloc,
    packet_free: Free,
    packet_unref: Unref,
    // Dropped after the function pointers, avcodec before the avutil it imports.
    _avcodec: libloading::Library,
    _avutil: libloading::Library,
}

impl Library {
    fn load(directory: &Path) -> Result<Self> {
        // SAFETY: the symbol types match FFmpeg 8's declarations, the majors are checked before
        // anything else is called, and both libraries are kept in `Self` with their pointers.
        unsafe {
            let open = |name: &str| {
                let path = beside(directory, name)?;
                load_beside(&path).with_context(|| format!("loading {}", path.display()))
            };
            let avutil = open(AVUTIL)?;
            let avcodec = open(AVCODEC)?;
            for (library, symbol, name, major) in [
                (&avutil, &b"avutil_version\0"[..], AVUTIL, AVUTIL_MAJOR),
                (&avcodec, &b"avcodec_version\0"[..], AVCODEC, AVCODEC_MAJOR),
            ] {
                let version = (*library.get::<Version>(symbol)?)();
                if version >> 16 != major {
                    bail!(
                        "{name} is version {}.{}, not {major}",
                        version >> 16,
                        (version >> 8) & 0xFF
                    );
                }
            }
            Ok(Self {
                frame_alloc: *avutil.get(b"av_frame_alloc\0")?,
                frame_free: *avutil.get(b"av_frame_free\0")?,
                frame_get_buffer: *avutil.get(b"av_frame_get_buffer\0")?,
                frame_make_writable: *avutil.get(b"av_frame_make_writable\0")?,
                layout_from_mask: *avutil.get(b"av_channel_layout_from_mask\0")?,
                layout_copy: *avutil.get(b"av_channel_layout_copy\0")?,
                set_int: *avutil.get(b"av_opt_set_int\0")?,
                set_layout: *avutil.get(b"av_opt_set_chlayout\0")?,
                strerror: *avutil.get(b"av_strerror\0")?,
                find_encoder: *avcodec.get(b"avcodec_find_encoder_by_name\0")?,
                #[cfg(test)]
                find_decoder: *avcodec.get(b"avcodec_find_decoder_by_name\0")?,
                alloc_context: *avcodec.get(b"avcodec_alloc_context3\0")?,
                free_context: *avcodec.get(b"avcodec_free_context\0")?,
                open: *avcodec.get(b"avcodec_open2\0")?,
                send_frame: *avcodec.get(b"avcodec_send_frame\0")?,
                receive_packet: *avcodec.get(b"avcodec_receive_packet\0")?,
                #[cfg(test)]
                send_packet: *avcodec.get(b"avcodec_send_packet\0")?,
                #[cfg(test)]
                receive_frame: *avcodec.get(b"avcodec_receive_frame\0")?,
                #[cfg(test)]
                new_packet: *avcodec.get(b"av_new_packet\0")?,
                packet_alloc: *avcodec.get(b"av_packet_alloc\0")?,
                packet_free: *avcodec.get(b"av_packet_free\0")?,
                packet_unref: *avcodec.get(b"av_packet_unref\0")?,
                _avcodec: avcodec,
                _avutil: avutil,
            })
        }
    }

    /// Fails with `what` and FFmpeg's text for a negative `code`.
    fn check(&self, code: c_int, what: &str) -> Result<c_int> {
        if code >= 0 {
            return Ok(code);
        }
        let mut text = [0 as c_char; 128];
        // SAFETY: `text` is writable for the length passed, and av_strerror terminates it.
        let message = unsafe {
            (self.strerror)(code, text.as_mut_ptr(), text.len());
            CStr::from_ptr(text.as_ptr()).to_string_lossy().into_owned()
        };
        bail!("{what} failed: {message} ({code})")
    }
}

/// An open FFmpeg codec with the frame and packet it exchanges.
struct Codec {
    ff: Library,
    name: &'static str,
    context: *mut c_void,
    frame: *mut c_void,
    packet: *mut c_void,
    channels: usize,
}

impl Codec {
    /// Loads the libraries and allocates a context for `codec`, the encoder or decoder `name`.
    fn alloc(
        directory: &Path,
        name: &'static str,
        codec: impl Fn(&Library) -> *const c_void,
    ) -> Result<(Self, *const c_void)> {
        let ff = Library::load(directory)?;
        let found = codec(&ff);
        if found.is_null() {
            bail!("this FFmpeg build has no {name}");
        }
        // SAFETY: `found` is an AVCodec of `ff`, and every allocation is freed by `Drop`,
        // which also takes the null pointers of a failed allocation.
        unsafe {
            let this = Self {
                context: (ff.alloc_context)(found),
                frame: (ff.frame_alloc)(),
                packet: (ff.packet_alloc)(),
                ff,
                name,
                channels: 0,
            };
            if this.context.is_null() || this.frame.is_null() || this.packet.is_null() {
                bail!("FFmpeg could not allocate the {name}");
            }
            Ok((this, found))
        }
    }

    fn option_int(&self, key: &CStr, value: i64) -> Result<()> {
        // SAFETY: `self.context` is a live AVCodecContext, whose first field is its AVClass.
        let code = unsafe { (self.ff.set_int)(self.context, key.as_ptr(), value, 0) };
        self.ff
            .check(
                code,
                &format!("setting {} {}", self.name, key.to_string_lossy()),
            )
            .map(drop)
    }

    /// Hands every packet the encoder has ready to `take`.
    fn drain(&mut self, take: &mut dyn FnMut(&[u8])) -> Result<()> {
        loop {
            // SAFETY: the context and packet are live; the packet's data and size are read at
            // FFmpeg 8's offsets and released by av_packet_unref before the next receive.
            unsafe {
                let code = (self.ff.receive_packet)(self.context, self.packet);
                if code == AGAIN {
                    return Ok(());
                }
                self.ff.check(code, &format!("{} output", self.name))?;
                let packet = self.packet.cast::<u8>();
                let data = *packet.add(PACKET_DATA).cast::<*const u8>();
                let size = *packet.add(PACKET_SIZE).cast::<c_int>();
                take(std::slice::from_raw_parts(data, size as usize));
                (self.ff.packet_unref)(self.packet);
            }
        }
    }
}

impl Drop for Codec {
    fn drop(&mut self) {
        // SAFETY: each pointer came from its FFmpeg allocator (or is null, which the freeing
        // functions accept) and is freed once, here.
        unsafe {
            (self.ff.packet_free)(&mut self.packet);
            (self.ff.frame_free)(&mut self.frame);
            (self.ff.free_context)(&mut self.context);
        }
    }
}

/// FFmpeg's AC3 (Dolby Digital) or E-AC3 (Dolby Digital Plus) encoder for 48 kHz audio: one
/// 1536-sample frame in, one frame of the bitrate's size out.
pub struct Ac3Encoder {
    codec: Codec,
}

impl Ac3Encoder {
    /// Opens the encoder from `directory`'s FFmpeg, failing with FFmpeg's reason for a channel
    /// count or bitrate it can't encode (FFmpeg encodes E-AC3 only up to 5.1).
    pub fn new(directory: &Path, eac3: bool, channels: u8, bitrate: u32) -> Result<Self> {
        let name = if eac3 { "E-AC3 encoder" } else { "AC3 encoder" };
        let codec_name = if eac3 { c"eac3" } else { c"ac3" };
        let (mut codec, found) = Codec::alloc(directory, name, |ff| {
            // SAFETY: the name is a C string.
            unsafe { (ff.find_encoder)(codec_name.as_ptr()) }
        })?;
        let Some(mask) = channel_mask(channels) else {
            bail!("the {name} takes 2, 6 or 8 channels, not {channels}");
        };
        codec.option_int(c"b", i64::from(bitrate))?;
        codec.option_int(c"ar", 48_000)?;
        // SAFETY: the context is live and not open yet, `layout` is a native layout without
        // allocations, and the frame is a fresh AVFrame written at FFmpeg 8's offsets before
        // av_frame_get_buffer allocates its planes.
        unsafe {
            let ff = &codec.ff;
            let context = codec.context.cast::<u8>();
            *context.add(CONTEXT_SAMPLE_FMT).cast::<c_int>() = SAMPLE_FMT_FLTP;
            let mut layout = ChannelLayout {
                order: 0,
                nb_channels: 0,
                mask: 0,
                opaque: std::ptr::null_mut(),
            };
            ff.check(
                (ff.layout_from_mask)(&mut layout, mask),
                "making a channel layout",
            )?;
            let code = (ff.set_layout)(codec.context, c"ch_layout".as_ptr(), &layout, 0);
            ff.check(code, &format!("setting the {name}'s channel layout"))?;
            let code = (ff.open)(codec.context, found, std::ptr::null_mut());
            ff.check(
                code,
                &format!(
                    "opening the {name} for {channels} channels at {} kbit/s",
                    bitrate / 1000
                ),
            )?;
            let frame_size = *context.add(CONTEXT_FRAME_SIZE).cast::<c_int>();
            if frame_size != FRAME_SAMPLES as c_int {
                bail!("the {name} takes frames of {frame_size} samples, not {FRAME_SAMPLES}");
            }
            let frame = codec.frame.cast::<u8>();
            *frame.add(FRAME_NB_SAMPLES).cast::<c_int>() = FRAME_SAMPLES as c_int;
            *frame.add(FRAME_FORMAT).cast::<c_int>() = SAMPLE_FMT_FLTP;
            *frame.add(FRAME_SAMPLE_RATE).cast::<c_int>() = 48_000;
            let code = (ff.layout_copy)(frame.add(FRAME_CH_LAYOUT).cast(), &layout);
            ff.check(code, "copying the channel layout")?;
            ff.check(
                (ff.frame_get_buffer)(codec.frame, 0),
                "allocating an audio frame",
            )?;
        }
        codec.channels = usize::from(channels);
        Ok(Self { codec })
    }

    /// Encodes one frame of interleaved samples ([`FRAME_SAMPLES`] per channel).
    pub fn encode(&mut self, samples: &[f32]) -> Result<Vec<u8>> {
        let codec = &mut self.codec;
        let channels = codec.channels;
        if samples.len() != FRAME_SAMPLES * channels {
            bail!(
                "the {} takes {FRAME_SAMPLES} samples per channel, not {}",
                codec.name,
                samples.len() as f64 / channels as f64
            );
        }
        // SAFETY: the frame was allocated for `channels` planes of FRAME_SAMPLES floats, made
        // writable first in case the encoder still references the last one.
        unsafe {
            let code = (codec.ff.frame_make_writable)(codec.frame);
            codec.ff.check(code, "reusing the audio frame")?;
            let planes = codec.frame.cast::<u8>().add(FRAME_DATA).cast::<*mut f32>();
            for channel in 0..channels {
                let plane = std::slice::from_raw_parts_mut(*planes.add(channel), FRAME_SAMPLES);
                for (sample, value) in plane
                    .iter_mut()
                    .zip(samples.iter().skip(channel).step_by(channels))
                {
                    *sample = *value;
                }
            }
            let code = (codec.ff.send_frame)(codec.context, codec.frame);
            codec.ff.check(code, &format!("{} input", codec.name))?;
        }
        let mut frames = Vec::new();
        codec.drain(&mut |packet| frames.push(packet.to_vec()))?;
        match <[Vec<u8>; 1]>::try_from(frames) {
            Ok([frame]) => Ok(frame),
            Err(frames) => bail!(
                "the {} gave {} frames for one frame of audio",
                codec.name,
                frames.len()
            ),
        }
    }
}

/// Whether `directory`'s FFmpeg encodes stereo AC3 and E-AC3, before the host offers them.
pub fn probe(directory: &Path) -> Result<()> {
    Ac3Encoder::new(directory, false, 2, 192_000)?;
    Ac3Encoder::new(directory, true, 2, 256_000)?;
    Ok(())
}

#[cfg(test)]
mod tests;
