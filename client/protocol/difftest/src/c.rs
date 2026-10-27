//! The C side: Nightjar's patched moonlight-common-c through `c/shim_*.c`.

use std::os::raw::c_int;

#[repr(C)]
struct VideoConfig {
    format: c_int,
    packet_size: c_int,
    app_version: [c_int; 4],
    rfi: c_int,
    records: c_int,
    colorspace: c_int,
    encrypted: c_int,
    key: [u8; 16],
    queue_bound: c_int,
}

#[repr(C)]
struct AudioConfig {
    app_version: [c_int; 4],
    packet_duration: c_int,
    encrypted: c_int,
    key: [u8; 16],
    iv: [u8; 16],
}

extern "C" {
    fn dt_log(length: *mut usize) -> *const u8;
    fn dt_log_clear();
    fn dt_video_init(config: *const VideoConfig);
    fn dt_video_destroy();
    fn dt_video_receive(data: *const u8, length: c_int, now_us: u64);
    fn dt_video_complete_all(status: c_int);
    fn dt_video_set_hdr(active: c_int);
    fn dt_video_current_frame() -> u32;
    fn dt_video_stats(video: *mut u32, fec: *mut u32);
    fn dt_audio_init(config: *const AudioConfig);
    fn dt_audio_destroy();
    fn dt_audio_receive(data: *const u8, length: c_int, now_us: u64);
    fn dt_audio_timeout();
    fn dt_audio_stats(out: *mut u32);

    fn reed_solomon_new(data_shards: c_int, parity_shards: c_int) -> *mut std::ffi::c_void;
    fn reed_solomon_release(rs: *mut std::ffi::c_void);
    fn reed_solomon_encode(
        rs: *mut std::ffi::c_void,
        shards: *mut *mut u8,
        count: c_int,
        size: c_int,
    ) -> c_int;
    fn reed_solomon_decode(
        rs: *mut std::ffi::c_void,
        shards: *mut *mut u8,
        marks: *mut u8,
        count: c_int,
        size: c_int,
    ) -> c_int;
}

/// Takes and clears the event log.
pub fn take_log() -> Vec<u8> {
    // SAFETY: the shim returns its log buffer and length; it is copied before clearing.
    unsafe {
        let mut length = 0;
        let data = dt_log(&mut length);
        let log = if length == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(data, length).to_vec()
        };
        dt_log_clear();
        log
    }
}

pub struct CVideo(());

impl CVideo {
    pub fn new(config: &nightjar_core::video::VideoConfig, queue_bound: usize) -> Self {
        let c = VideoConfig {
            format: config.format as c_int,
            packet_size: config.packet_size as c_int,
            app_version: config.app_version,
            rfi: config.rfi.into(),
            records: config.pyrowave_records.into(),
            colorspace: config.colorspace.into(),
            encrypted: config.encryption_key.is_some().into(),
            key: config.encryption_key.unwrap_or_default(),
            queue_bound: queue_bound as c_int,
        };
        // SAFETY: plain configuration struct; the shim copies it.
        unsafe { dt_video_init(&c) };
        Self(())
    }

    pub fn receive(&mut self, datagram: &[u8], now_us: u64) {
        // SAFETY: the shim reads `length` bytes and copies them.
        unsafe { dt_video_receive(datagram.as_ptr(), datagram.len() as c_int, now_us) }
    }

    /// Completes queued frames: `DR_OK` (0) or `DR_NEED_IDR` (-1).
    pub fn complete_all(&mut self, ok: bool) {
        // SAFETY: no pointers involved.
        unsafe { dt_video_complete_all(if ok { 0 } else { -1 }) }
    }

    pub fn set_hdr(&mut self, active: bool) {
        // SAFETY: no pointers involved.
        unsafe { dt_video_set_hdr(active.into()) }
    }

    pub fn current_frame(&self) -> u32 {
        // SAFETY: no pointers involved.
        unsafe { dt_video_current_frame() }
    }

    /// `packetCountVideo` and `packetCountFec`.
    pub fn stats(&self) -> (u32, u32) {
        let (mut video, mut fec) = (0, 0);
        // SAFETY: both pointers are valid for the call.
        unsafe { dt_video_stats(&mut video, &mut fec) };
        (video, fec)
    }
}

impl Drop for CVideo {
    fn drop(&mut self) {
        // SAFETY: tears down what new() initialized.
        unsafe { dt_video_destroy() }
    }
}

pub struct CAudio(());

impl CAudio {
    pub fn new(config: &nightjar_core::audio::AudioConfig) -> Self {
        let (encrypted, key, iv) = match &config.encryption {
            Some(e) => (1, e.key, e.iv),
            None => (0, [0; 16], [0; 16]),
        };
        let c = AudioConfig {
            app_version: config.app_version,
            packet_duration: config.packet_duration_ms as c_int,
            encrypted,
            key,
            iv,
        };
        // SAFETY: plain configuration struct; the shim copies it.
        unsafe { dt_audio_init(&c) };
        Self(())
    }

    pub fn receive(&mut self, datagram: &[u8], now_us: u64) {
        // SAFETY: the shim reads `length` bytes and copies them.
        unsafe { dt_audio_receive(datagram.as_ptr(), datagram.len() as c_int, now_us) }
    }

    pub fn timeout(&mut self) {
        // SAFETY: no pointers involved.
        unsafe { dt_audio_timeout() }
    }

    pub fn stats(&self) -> [u32; 7] {
        let mut out = [0; 7];
        // SAFETY: the shim writes seven values.
        unsafe { dt_audio_stats(out.as_mut_ptr()) };
        out
    }
}

impl Drop for CAudio {
    fn drop(&mut self) {
        // SAFETY: tears down what new() initialized.
        unsafe { dt_audio_destroy() }
    }
}

/// nanors encode: fills `shards[data..]`.
pub fn rs_encode(data: usize, shards: &mut [Vec<u8>], size: usize) -> bool {
    let parity = shards.len() - data;
    // SAFETY: every shard pointer is valid for `size` bytes; nanors only touches those.
    unsafe {
        let rs = reed_solomon_new(data as c_int, parity as c_int);
        if rs.is_null() {
            return false;
        }
        let mut pointers: Vec<*mut u8> = shards.iter_mut().map(|s| s.as_mut_ptr()).collect();
        let result = reed_solomon_encode(
            rs,
            pointers.as_mut_ptr(),
            shards.len() as c_int,
            size as c_int,
        );
        reed_solomon_release(rs);
        result == 0
    }
}

/// nanors decode of the shards marked missing.
pub fn rs_decode(data: usize, shards: &mut [Vec<u8>], missing: &[bool], size: usize) -> bool {
    let parity = shards.len() - data;
    // SAFETY: as in rs_encode; `marks` has one byte per shard.
    unsafe {
        let rs = reed_solomon_new(data as c_int, parity as c_int);
        if rs.is_null() {
            return false;
        }
        let mut pointers: Vec<*mut u8> = shards.iter_mut().map(|s| s.as_mut_ptr()).collect();
        let mut marks: Vec<u8> = missing.iter().map(|&m| m.into()).collect();
        let result = reed_solomon_decode(
            rs,
            pointers.as_mut_ptr(),
            marks.as_mut_ptr(),
            shards.len() as c_int,
            size as c_int,
        );
        reed_solomon_release(rs);
        result == 0
    }
}
