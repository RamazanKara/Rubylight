//! Unit tests for DMA-BUF descriptor layout, EGL attribute packing, render-node selection, and
//! per-surface import caching.

use super::*;

fn nv12(modifier: u64) -> Descriptor {
    let mut descriptor = Descriptor { fourcc: fourcc(b"NV12"), width: 1920, height: 1088, num_objects: 1, num_layers: 2, ..Default::default() };
    descriptor.objects[0] = Object { fd: 7, size: 3_200_000, drm_format_modifier: modifier };
    descriptor.layers[0] = Layer { drm_format: DRM_FORMAT_R8, num_planes: 1, pitch: [2048, 0, 0, 0], ..Default::default() };
    descriptor.layers[1] = Layer { drm_format: DRM_FORMAT_GR88, num_planes: 1, offset: [2048 * 1088, 0, 0, 0], pitch: [2048, 0, 0, 0], ..Default::default() };
    descriptor
}

#[test]
fn the_descriptor_has_libvas_layout() {
    // va_drmcommon.h: objects at 16, num_layers at 80, layers at 84, 312 bytes in all.
    assert_eq!(std::mem::size_of::<Object>(), 16);
    assert_eq!(std::mem::size_of::<Layer>(), 56);
    assert_eq!(std::mem::offset_of!(Descriptor, objects), 16);
    assert_eq!(std::mem::offset_of!(Descriptor, num_layers), 80);
    assert_eq!(std::mem::offset_of!(Descriptor, layers), 84);
    assert_eq!(std::mem::size_of::<Descriptor>(), 312);
    assert_eq!(DRM_FORMAT_R8, 0x2020_3852);
    assert_eq!(DRM_FORMAT_GR88, 0x3838_5247);
}

#[test]
fn only_nv12_in_two_one_plane_layers_is_imported() {
    assert!(check_nv12_layers(&nv12(0)).is_ok());
    let mut composed = nv12(0);
    composed.num_layers = 1;
    composed.layers[0] = Layer { drm_format: fourcc(b"NV12"), num_planes: 2, ..Default::default() };
    assert!(check_nv12_layers(&composed).unwrap_err().contains("1 layers"));
    let mut p010 = nv12(0);
    p010.layers[0].drm_format = fourcc(b"R16 ");
    p010.layers[1].drm_format = fourcc(b"GR32");
    assert_eq!(check_nv12_layers(&p010).unwrap_err(), "layer 0 is R16 with 1 planes, not R8");
    let mut stray = nv12(0);
    stray.layers[1].object_index[0] = 1;
    assert_eq!(check_nv12_layers(&stray).unwrap_err(), "layer 1 names object 1 of 1");
}

#[test]
fn layer_attributes_size_chroma_by_half_rounded_up_and_carry_the_modifier() {
    let tiled = nv12(0x0100_0000_0000_0002);
    let luma = layer_attributes(&tiled, 0, 1919, 1081, true);
    assert_eq!(
        &*luma,
        [0x3271, DRM_FORMAT_R8 as i32, 0x3057, 1919, 0x3056, 1081, 0x30D2, 1, 0x3272, 7, 0x3273, 0, 0x3274, 2048, 0x3443, 2, 0x3444, 0x0100_0000, 0x3038]
    );
    let chroma = layer_attributes(&tiled, 1, 1919, 1081, true);
    assert_eq!(&chroma[..14], [0x3271, DRM_FORMAT_GR88 as i32, 0x3057, 960, 0x3056, 541, 0x30D2, 1, 0x3272, 7, 0x3273, 2048 * 1088, 0x3274, 2048]);
    // Without the modifiers extension, or with no modifier named, the driver's layout is implied.
    assert_eq!(layer_attributes(&tiled, 0, 1920, 1080, false).len(), 15);
    assert_eq!(layer_attributes(&nv12(DRM_FORMAT_MOD_INVALID), 0, 1920, 1080, true).len(), 15);
}

#[test]
fn only_the_opengl_renderer_takes_imports() {
    assert!(imports_into("opengl"));
    // GLES2 reads the RG-shader hint per draw: imported chroma would be read from alpha.
    for other in ["opengles2", "gpu", "vulkan", "software", "direct3d11"] {
        assert!(!imports_into(other), "{other}");
    }
}

#[test]
fn render_nodes_are_read_from_paths_and_sysfs_names() {
    assert_eq!(render_node_minor("/dev/dri/renderD129"), Some(129));
    assert_eq!(render_node_minor("renderD128"), Some(128));
    for other in ["/dev/dri/card1", "card0", "controlD64", "renderD", "/dev/dri/renderDx", "by-path"] {
        assert_eq!(render_node_minor(other), None, "{other}");
    }
}

/// A hybrid PC (a user's Iris Xe at renderD128 beside an RX 9060 XT at renderD129, the
/// monitor on the Radeon): VAAPI opened on FFmpeg's default device, the Intel GPU, and the
/// Radeon's EGL refused every picture's Y-tiled layout (modifier 0x0100000000000002), so each
/// one went through memory and the decoder fell behind above 60 fps.
#[test]
fn vaapi_opens_on_the_opengl_renderers_gpu_when_it_is_not_the_default_device() {
    let (node, line) = display_vaapi_node(Ok(129), &[128, 129]);
    assert_eq!(node, Some(129));
    assert!(line.contains("/dev/dri/renderD129") && line.contains("default device /dev/dri/renderD128"), "{line}");
    // The display on the integrated GPU: the default device already is the renderer's.
    let (node, line) = display_vaapi_node(Ok(128), &[128, 129]);
    assert_eq!(node, None);
    assert!(line.contains("default device /dev/dri/renderD128, the OpenGL renderer's GPU"), "{line}");
}

/// One GPU, AMD's or Intel's, is FFmpeg's default device: VAAPI opens as it always has.
#[test]
fn a_single_gpu_keeps_the_default_device() {
    for single in [[128], [129]] {
        let (node, line) = display_vaapi_node(Ok(single[0]), &single);
        assert_eq!(node, None, "{line}");
        assert!(line.contains("the OpenGL renderer's GPU"), "{line}");
    }
}

/// Unknown or unlisted, the display's GPU changes nothing, and the log says why.
#[test]
fn an_unknown_display_gpu_keeps_the_default_device_and_says_why() {
    let (node, line) = display_vaapi_node(Err("EGL does not name its devices (no EGL_EXT_device_query)".into()), &[128, 129]);
    assert_eq!(node, None);
    assert!(line.contains("unknown (EGL does not name its devices (no EGL_EXT_device_query))"), "{line}");
    let (node, line) = display_vaapi_node(Ok(130), &[128, 129]);
    assert_eq!(node, None);
    assert!(line.contains("/dev/dri/renderD130, is not among the render nodes here (/dev/dri/renderD128, /dev/dri/renderD129)"), "{line}");
    assert_eq!(display_vaapi_node(Ok(128), &[]).0, None, "no render node listed (a sandbox without /dev/dri)");
}

#[test]
fn imports_are_cached_unless_the_environment_says_0() {
    assert!(cache_switched_on(None));
    assert!(cache_switched_on(Some("1".as_ref())));
    assert!(!cache_switched_on(Some("0".as_ref())));
}

const POOL: Generation = Generation { frames: 0x1000, width: 1920, height: 1080 };

/// A surface the decoder hands out again finds its import; the pool's other surfaces get
/// their own, and nothing is given back while the pool stays.
#[test]
fn a_surface_seen_again_finds_its_import() {
    let mut cache = SurfaceCache::new(CACHE_LIMIT);
    assert_eq!(cache.retarget(POOL), (true, vec![]), "the first pool is a change, with nothing to drop");
    assert_eq!(cache.get(7), None);
    assert!(cache.insert(7, "seven").is_empty());
    assert!(cache.insert(8, "eight").is_empty());
    assert_eq!(cache.retarget(POOL), (false, vec![]), "the same pool keeps its imports");
    assert_eq!(cache.get(7), Some(&"seven"));
    assert_eq!(cache.get(8), Some(&"eight"));
    assert_eq!(cache.insert(7, "seven again"), ["seven"], "a surface imported anew gives its old import back");
    assert_eq!(cache.len(), 2);
}

/// A new frames context (a decoder reset) or another picture size drops every import, as
/// its surfaces are other buffers even where their ids repeat.
#[test]
fn another_pool_or_size_drops_every_import() {
    let mut cache = SurfaceCache::new(CACHE_LIMIT);
    cache.retarget(POOL);
    cache.insert(1, 'a');
    cache.insert(2, 'b');
    let (changed, mut gone) = cache.retarget(Generation { frames: 0x2000, ..POOL });
    gone.sort_unstable();
    assert!(changed);
    assert_eq!(gone, ['a', 'b']);
    assert_eq!(cache.get(1), None, "surface 1 of the new pool is another buffer");
    cache.insert(1, 'c');
    let (changed, gone) = cache.retarget(Generation { width: 1280, height: 720, frames: 0x2000 });
    assert!(changed && gone == ['c'], "another size: the imports were made for the old one");
    assert!(cache.drain().is_empty());
}

/// More surfaces than the limit: the oldest import goes, never the one just made.
#[test]
fn the_cache_keeps_at_most_its_limit_oldest_out_first() {
    let mut cache = SurfaceCache::new(2);
    cache.retarget(POOL);
    assert!(cache.insert(1, 1).is_empty());
    assert!(cache.insert(2, 2).is_empty());
    assert_eq!(cache.insert(3, 3), [1]);
    assert_eq!((cache.get(1), cache.get(2), cache.get(3)), (None, Some(&2), Some(&3)));
    let mut rest = cache.drain();
    rest.sort_unstable();
    assert_eq!(rest, [2, 3]);
    assert_eq!(cache.retarget(POOL), (false, vec![]), "draining keeps the pool");
}

#[test]
fn zero_copy_is_on_unless_the_environment_says_0() {
    assert!(enabled_by_environment(None));
    assert!(enabled_by_environment(Some("1".as_ref())));
    assert!(!enabled_by_environment(Some("0".as_ref())));
}
