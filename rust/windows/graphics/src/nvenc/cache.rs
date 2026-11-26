//! NVENC output-event state and static repeat-frame conversion caching.
//!
//! When constant-rate capture or frame pacing repeats an unchanged capture
//! snapshot (`Encoder::set_repeat(true)`), keeping the last converted surface
//! lets NVENC re-submit the already-converted D3D12/D3D11 slot without running
//! color conversion, cursor/menu compositing, HDR statistics, or the CUDA
//! 10-bit 4:4:4 plane copy again.

use super::Kept;
use crate::capture::{GpuImage, Pixel};
use crate::overlay::Overlay;
use std::sync::Arc;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::core::Interface;

/// Output completion notification mode for [`super::Encoder::output_event`],
/// gated by `nvenc_output_event`.
pub(super) enum OutputEvent {
    Off,
    /// D3D11 or CUDA asynchronous session: returns the oldest pending slot's
    /// manual-reset completion event.
    Async,
    /// D3D12 session: single manual-reset event armed on the D3D12 output
    /// fence.
    D3d12(crate::timing::Signal),
}

impl OutputEvent {
    /// Take the D3D12 fence event on an unclean close so the caller can forget
    /// it instead of closing a handle a hung GPU may still signal.
    pub(super) fn take_d3d12(&mut self) -> Option<crate::timing::Signal> {
        match std::mem::replace(self, Self::Off) {
            Self::D3d12(event) => Some(event),
            Self::Off | Self::Async => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CursorKey {
    position: [i32; 2],
    width: u32,
    height: u32,
    logic: bool,
    pixels_ptr: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OverlayKey {
    revision: u64,
    origin: [u32; 2],
    size: [u32; 2],
    scale: u32,
    rgba_ptr: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MetaKey {
    source: (u32, u32, Pixel),
    display_in_hdr: Option<bool>,
    origin: u64,
    ready: Option<(usize, u64)>,
    luminance: [u32; 2],
    cursor: Option<CursorKey>,
    overlay: Option<OverlayKey>,
}

impl MetaKey {
    fn from_inputs(image: &GpuImage, luminance: [f32; 2], overlay: Option<&Overlay>) -> Self {
        Self {
            source: (image.width, image.height, image.pixel),
            display_in_hdr: image.gpu.display_in_hdr(),
            origin: image.origin,
            ready: image
                .ready
                .as_ref()
                .map(|ready| (ready.fence.as_raw() as usize, ready.value)),
            luminance: [luminance[0].to_bits(), luminance[1].to_bits()],
            cursor: image.cursor.as_ref().map(|cursor| CursorKey {
                position: cursor.position,
                width: cursor.width,
                height: cursor.height,
                logic: cursor.logic,
                pixels_ptr: Arc::as_ptr(&cursor.pixels).cast::<u8>() as usize,
            }),
            overlay: overlay.map(|overlay| OverlayKey {
                revision: overlay.revision,
                origin: overlay.origin,
                size: overlay.size,
                scale: overlay.scale,
                rgba_ptr: Arc::as_ptr(&overlay.rgba).cast::<u8>() as usize,
            }),
        }
    }
}

/// Last converted input surface retained for static repeat-frame reuse.
pub(super) struct CachedConversion {
    /// Keeps the capture texture pinned (`Arc::strong_count >= 2`) so
    /// `GpuPool` cannot overwrite it while cached.
    source_texture: Arc<ID3D11Texture2D>,
    /// Keeps cursor and overlay pixel allocations pinned so their pointer
    /// addresses cannot be recycled while cached.
    _cursor_pixels: Option<Arc<[u8]>>,
    _overlay_rgba: Option<Arc<[u8]>>,
    meta: MetaKey,
    pub(super) slot: usize,
    pub(super) slot_key: usize,
    pub(super) kept: Kept,
}

impl CachedConversion {
    pub(super) fn new(
        image: &GpuImage,
        luminance: [f32; 2],
        overlay: Option<&Overlay>,
        slot: usize,
        slot_key: usize,
        kept: Kept,
    ) -> Self {
        Self {
            source_texture: image.texture.clone(),
            _cursor_pixels: image.cursor.as_ref().map(|c| c.pixels.clone()),
            _overlay_rgba: overlay.map(|o| o.rgba.clone()),
            meta: MetaKey::from_inputs(image, luminance, overlay),
            slot,
            slot_key,
            kept,
        }
    }

    pub(super) fn matches(
        &self,
        image: &GpuImage,
        luminance: [f32; 2],
        overlay: Option<&Overlay>,
    ) -> bool {
        Arc::ptr_eq(&self.source_texture, &image.texture)
            && self.meta == MetaKey::from_inputs(image, luminance, overlay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_meta() -> MetaKey {
        MetaKey {
            source: (1920, 1080, Pixel::Bgra8),
            display_in_hdr: Some(true),
            origin: 42,
            ready: Some((0x1000, 7)),
            luminance: [100.0_f32.to_bits(), 1.0_f32.to_bits()],
            cursor: Some(CursorKey {
                position: [10, 20],
                width: 32,
                height: 32,
                logic: false,
                pixels_ptr: 0x2000,
            }),
            overlay: Some(OverlayKey {
                revision: 3,
                origin: [0, 0],
                size: [320, 160],
                scale: 1,
                rgba_ptr: 0x3000,
            }),
        }
    }

    #[test]
    fn meta_key_detects_every_conversion_input_change() {
        let base = sample_meta();
        assert_eq!(base, sample_meta());

        let mut changed = base;
        changed.source.0 = 2560;
        assert_ne!(base, changed);

        let mut changed = base;
        changed.display_in_hdr = Some(false);
        assert_ne!(base, changed);

        let mut changed = base;
        changed.origin = 43;
        assert_ne!(base, changed);

        let mut changed = base;
        changed.ready = Some((0x1000, 8));
        assert_ne!(base, changed);

        let mut changed = base;
        changed.luminance[0] = 200.0_f32.to_bits();
        assert_ne!(base, changed);

        let mut changed = base;
        changed.cursor.as_mut().unwrap().position = [11, 20];
        assert_ne!(base, changed);

        let mut changed = base;
        changed.overlay.as_mut().unwrap().revision = 4;
        assert_ne!(base, changed);
    }

    #[test]
    fn output_event_take_d3d12_leaves_off() {
        let mut event = OutputEvent::D3d12(crate::timing::Signal::new().unwrap());
        assert!(event.take_d3d12().is_some());
        assert!(matches!(event, OutputEvent::Off));
        assert!(event.take_d3d12().is_none());

        let mut async_ev = OutputEvent::Async;
        assert!(async_ev.take_d3d12().is_none());
        assert!(matches!(async_ev, OutputEvent::Off));
    }
}
