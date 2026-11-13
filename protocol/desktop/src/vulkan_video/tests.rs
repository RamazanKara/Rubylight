use super::*;

#[test]
fn the_environment_puts_vulkan_video_first_or_off_and_anything_else_leaves_it_after_vaapi() {
    assert_eq!(Choice::of(Some("1".as_ref())), Choice::First);
    assert_eq!(Choice::of(Some("0".as_ref())), Choice::Off);
    for other in [None, Some("yes"), Some(""), Some("2")] {
        assert_eq!(
            Choice::of(other.map(AsRef::as_ref)),
            Choice::Default,
            "{other:?}"
        );
    }
}

#[test]
fn video_extensions_decode_a_codec_only_with_both_queue_extensions() {
    let all = Extensions::among(EXTENSIONS);
    assert_eq!(all, Extensions(0b1_1111_1111));
    assert_eq!(all.usable(), Extensions(0b1_1111), "the import extensions are not video ones");
    assert!([Codec::H264, Codec::Hevc, Codec::Av1]
        .into_iter()
        .all(|codec| all.decodes(codec)));
    let hevc = Extensions::among([
        c"VK_KHR_video_queue",
        c"VK_KHR_video_decode_queue",
        c"VK_KHR_video_decode_h265",
        c"VK_KHR_swapchain",
    ]);
    assert!(
        hevc.decodes(Codec::Hevc) && !hevc.decodes(Codec::H264) && !hevc.decodes(Codec::Av1)
    );
    assert_eq!(
        hevc.names(),
        [
            c"VK_KHR_video_queue",
            c"VK_KHR_video_decode_queue",
            c"VK_KHR_video_decode_h265"
        ]
    );
    let no_queue = Extensions::among([c"VK_KHR_video_queue", c"VK_KHR_video_decode_h264"]);
    assert!(no_queue.usable().is_empty() && !no_queue.decodes(Codec::H264));
    let no_codec = Extensions::among([c"VK_KHR_video_queue", c"VK_KHR_video_decode_queue"]);
    assert!(
        no_codec.usable().is_empty(),
        "queues without a codec decode nothing"
    );
}

const IMPORT: [&CStr; 3] = [
    c"VK_KHR_external_memory_fd",
    c"VK_EXT_external_memory_dma_buf",
    c"VK_EXT_image_drm_format_modifier",
];

/// VAAPI pictures went through memory to the GPU renderer (2.5 GB/s at 4K100 10-bit,
/// nightjar#80): they are mapped where the renderer's device imports DMA-BUFs and VAAPI
/// decodes on its GPU, and every other case says why it keeps the readback.
#[test]
fn vaapi_pictures_are_mapped_only_with_the_import_extensions_on_the_renderers_gpu() {
    let import = Extensions::among(IMPORT);
    assert_eq!(import.mapping(), import);
    assert!(import.usable().is_empty(), "importing decodes nothing");
    assert_eq!(map_plan(true, Ok(import), Ok(())), Ok(()));
    let video_and_import = Extensions::among(EXTENSIONS);
    assert_eq!(map_plan(true, Ok(video_and_import), Ok(())), Ok(()));
    assert_eq!(map_plan(false, Ok(import), Ok(())), Err("NIGHTJAR_VAAPI_VULKAN_MAP=0".into()));
    let no_modifier = Extensions::among(IMPORT[..2].iter().copied());
    assert!(no_modifier.mapping().is_empty());
    let why = map_plan(true, Ok(no_modifier), Ok(())).unwrap_err();
    assert!(why.contains("VK_EXT_image_drm_format_modifier") && !why.contains("dma_buf"), "{why}");
    let why = map_plan(true, Ok(Extensions::among(EXTENSIONS[..5].iter().copied())), Ok(())).unwrap_err();
    assert!(IMPORT.iter().all(|name| why.contains(name.to_str().unwrap())), "{why}");
    let why = map_plan(true, Ok(import), Err("VAAPI does not open on /dev/dri/renderD129".into())).unwrap_err();
    assert!(why.starts_with("VAAPI does not decode on the renderer's GPU") && why.contains("renderD129"), "{why}");
    let why = map_plan(true, Err("the renderer draws with \"opengl\", not Vulkan".into()), Ok(())).unwrap_err();
    assert!(why.contains("opengl"), "{why}");
    // The switch and what the renderer asks for.
    assert!(map_switched_on(None) && map_switched_on(Some("1".as_ref())) && !map_switched_on(Some("0".as_ref())));
    assert_eq!(Extensions::wanted(true, true), Extensions::among(EXTENSIONS));
    assert_eq!(Extensions::wanted(false, true), Extensions(import.0 | 1 << 8), "and the semaphore export for CUDA");
    assert_eq!(Extensions::wanted(true, false).mapping(), Extensions(0));
    // The render node VAAPI opens on.
    assert_eq!(render_node_minor(true, 129), Ok(129));
    assert!(render_node_minor(false, 129).is_err());
    assert!(render_node_minor(true, -1).is_err());
}

/// A mapped picture freed right after its copy was submitted would block the presenter until
/// the copy finished and give its VAAPI surface back while the GPU may still read it.
#[test]
fn mapped_pictures_are_kept_until_their_slots_copy_is_done() {
    let mut held = Held::new();
    assert_eq!(held.hold(0, "frame 1"), None);
    assert_eq!(held.hold(1, "frame 2"), None);
    assert!(held.release(|_| false).is_empty(), "nothing goes while copies run");
    assert_eq!(held.release(|slot| slot == 0), ["frame 1"]);
    assert_eq!(held.hold(0, "frame 3"), None, "slot 0's earlier picture went when its copy was done");
    // A slot used again after its fence was waited for gives back what it held.
    assert_eq!(held.take(1), Some("frame 2"));
    assert_eq!(held.hold(0, "frame 4"), Some("frame 3"));
    assert_eq!(held.drain(), ["frame 4"]);
    assert!(held.release(|_| true).is_empty());
}

/// Mapped pictures were kept only while their image still said `VK_QUEUE_FAMILY_EXTERNAL`: one
/// FFmpeg had moved itself was freed at once, blocking on its copy. They are known by their
/// frames context instead.
#[test]
fn mapped_pictures_are_known_by_their_frames_context() {
    // SAFETY: A plain C struct, all-zero is valid.
    let mut context: AVHWFramesContext = unsafe { std::mem::zeroed() };
    assert!(!mapped_frames(&context), "a decoder's frames context");
    context.user_opaque = MAPPED.as_ptr().cast_mut().cast();
    assert!(mapped_frames(&context));
    let other = [0u8];
    context.user_opaque = other.as_ptr().cast_mut().cast();
    assert!(!mapped_frames(&context), "another user's mark");
}

/// NVDEC's pictures crossed system memory on their way to the GPU renderer: CUDA copies them
/// into exportable Vulkan images where the renderer's device has both interop extensions,
/// and every other case says why they keep the trip through memory.
#[test]
fn nvdec_pictures_are_copied_by_cuda_only_with_both_interop_extensions() {
    let interop = Extensions::among([c"VK_KHR_external_memory_fd", c"VK_KHR_external_semaphore_fd"]);
    assert_eq!(interop.interop(), interop);
    assert!(interop.usable().is_empty() && interop.mapping().is_empty(), "interop decodes and maps nothing");
    assert_eq!(cuda_plan(Ok(interop)), Ok(()));
    assert_eq!(cuda_plan(Ok(Extensions::among(EXTENSIONS))), Ok(()));
    // The DMA-BUF import extensions have the memory export, not the semaphore one.
    let import = Extensions::among(IMPORT);
    assert!(import.interop().is_empty());
    let why = cuda_plan(Ok(import)).unwrap_err();
    assert!(why.contains("VK_KHR_external_semaphore_fd") && !why.contains("memory_fd"), "{why}");
    let why = cuda_plan(Ok(Extensions::among(EXTENSIONS[..5].iter().copied()))).unwrap_err();
    assert!(why.contains("VK_KHR_external_memory_fd, VK_KHR_external_semaphore_fd"), "{why}");
    let why = cuda_plan(Err("the renderer draws with \"opengl\", not Vulkan".into())).unwrap_err();
    assert!(why.contains("opengl"), "{why}");
    // Asked for whatever the switches say.
    for (video, map) in [(false, false), (true, false), (false, true), (true, true)] {
        assert_eq!(Extensions::wanted(video, map).interop(), interop, "{video} {map}");
    }
    // Only the two-plane 4:2:0 pictures `Planes` copies: NVDEC's 4:4:4 ones are three planes.
    assert!(mappable(AV_PIX_FMT_NV12) && mappable(AV_PIX_FMT_P010LE));
    assert!(!mappable(crate::media::ffi::AV_PIX_FMT_YUV444P) && !mappable(crate::media::ffi::AV_PIX_FMT_YUV444P16LE));
}

/// NVDEC on another GPU than the renderer's is refused when the copy is set up, both GPUs named.
#[test]
fn cuda_must_run_on_the_renderers_gpu() {
    let renderer = [0x5e, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e, 0x6f, 0x70, 0x81, 0x92, 0xa3, 0xb4, 0xc5, 0xd6, 0xe7, 0xf8];
    assert_eq!(same_gpu(renderer, renderer), Ok(()));
    let mut other = renderer;
    other[15] = 0x09;
    let why = same_gpu(renderer, other).unwrap_err();
    assert!(why.contains("another GPU than the renderer's"), "{why}");
    assert!(why.contains("CUDA device 5e1a2b3c-4d5e-6f70-8192-a3b4c5d6e709"), "{why}");
    assert!(why.contains("Vulkan device 5e1a2b3c-4d5e-6f70-8192-a3b4c5d6e7f8"), "{why}");
    // A decoder without a CUDA device context says so instead of crashing.
    // SAFETY: A null context is the documented refusal.
    assert!(unsafe { cuda_uuid(ptr::null_mut()) }.unwrap_err().contains("no CUDA device context"));
}

/// CUDA imports no multi-plane image, and the decoder's coded size is the copy's.
#[test]
fn the_frames_cuda_copies_into_have_one_image_per_plane() {
    // SAFETY: Plain C structs, all-zero is valid; `vulkan` outlives the context's use.
    let mut vulkan: FramesContext = unsafe { std::mem::zeroed() };
    let mut context: AVHWFramesContext = unsafe { std::mem::zeroed() };
    context.hwctx = (&raw mut vulkan).cast();
    context.initial_pool_size = 20;
    unsafe { interop_frames(&mut context, (1920, 1088, AV_PIX_FMT_P010LE)) };
    assert_eq!((context.format, context.sw_format, context.width, context.height), (AV_PIX_FMT_VULKAN, AV_PIX_FMT_P010LE, 1920, 1088));
    assert_eq!(context.initial_pool_size, 0, "the pool grows as pictures are held");
    assert_eq!(vulkan.flags, DISABLE_MULTIPLANE);
    assert_eq!(DISABLE_MULTIPLANE, 4, "AV_VK_FRAME_FLAG_DISABLE_MULTIPLANE");
}

/// Intel's VAAPI decodes 4:4:4 into one interleaved surface (XYUV or AYUV, Y410), which went
/// through memory and was refused: it maps as one R8G8B8A8 or A2R10G10B10 image that
/// `Planes` copies whole, and FFmpeg's import knows its DRM layer by the alias.
#[test]
fn vaapis_444_surfaces_map_as_one_interleaved_image() {
    assert_eq!(interleaved_444(AV_PIX_FMT_VUYX), Some((AV_PIX_FMT_RGBA, false)));
    assert_eq!(interleaved_444(AV_PIX_FMT_VUYA), Some((AV_PIX_FMT_RGBA, false)));
    assert_eq!(interleaved_444(AV_PIX_FMT_XV30LE), Some((AV_PIX_FMT_XV30LE, true)));
    for other in [AV_PIX_FMT_NV12, AV_PIX_FMT_P010LE, AV_PIX_FMT_NV24, crate::media::ffi::AV_PIX_FMT_XV36LE] {
        assert_eq!(interleaved_444(other), None, "{other}");
    }
    assert!(!mappable(AV_PIX_FMT_VUYX) && !mappable(AV_PIX_FMT_XV30LE), "not two planes");
    for ten_bit in [false, true] {
        assert_eq!(interleaved_sw_format(ten_bit), interleaved_444(if ten_bit { AV_PIX_FMT_XV30LE } else { AV_PIX_FMT_VUYX }).unwrap().0);
    }
    // The same bytes under the names hwcontext_vulkan's DMA-BUF table has.
    assert_eq!(drm_alias(fourcc(b"AYUV")), fourcc(b"XYUV"));
    assert_eq!(drm_alias(fourcc(b"Y410")), fourcc(b"XV30"));
    for kept in [fourcc(b"XYUV"), fourcc(b"XV30"), fourcc(b"NV12"), fourcc(b"P010")] {
        assert_eq!(drm_alias(kept), kept);
    }
    assert_eq!(fourcc(b"AYUV"), 0x5655_5941, "DRM_FORMAT_AYUV");
    // One image, Y, Cb and Cr in one texel; the 10-bit one in SDL's only 2:10:10:10 format.
    let raw = |(vulkan, sdl): (vk::Format, SDL_GPUTextureFormat)| (vulkan.as_raw(), sdl.0);
    assert_eq!(raw(plane_format(0, false, true)), raw((vk::Format::R8G8B8A8_UNORM, SDL_GPU_TEXTUREFORMAT_R8G8B8A8_UNORM)));
    assert_eq!(raw(plane_format(0, true, true)), raw((vk::Format::A2B10G10R10_UNORM_PACK32, SDL_GPU_TEXTUREFORMAT_R10G10B10A2_UNORM)));
    assert_eq!(raw(plane_format(1, true, false)), raw((vk::Format::R16G16_UNORM, SDL_GPU_TEXTUREFORMAT_R16G16_UNORM)));
    assert_eq!(raw(plane_format(0, false, false)), raw((vk::Format::R8_UNORM, SDL_GPU_TEXTUREFORMAT_R8_UNORM)));
    // FFmpeg's AVDRMFrameDescriptor, read from the DRM PRIME frame.
    assert_eq!(std::mem::size_of::<DrmLayer>(), 104);
    assert_eq!(std::mem::offset_of!(DrmFrameDescriptor, nb_layers), 104);
    assert_eq!(std::mem::offset_of!(DrmFrameDescriptor, layers), 112);
    assert_eq!(std::mem::size_of::<DrmFrameDescriptor>(), 528);
}

/// Release builds' FFmpeg had no libdrm, so `av_hwframe_map` was ENOSYS and every shipped
/// Linux build read VAAPI pictures back.
#[test]
fn the_shipped_ffmpeg_is_built_with_libdrm_for_the_mapping() {
    let script = include_str!("../../ffmpeg-decode-only.sh");
    let linux = script.split("platform=(").nth(1).expect("the Linux options");
    let linux = &linux[..linux.find(')').expect("their end")];
    assert!(linux.split_whitespace().any(|option| option == "--enable-libdrm"), "{linux}");
    assert!(script.contains("does not link libdrm"), "the build checks libavutil links it");
}

/// An imported image is FFmpeg's `VK_QUEUE_FAMILY_EXTERNAL` in UNDEFINED layout: the copy
/// acquires it from outside Vulkan without dropping its contents; decoder images move no family.
#[test]
fn an_imported_picture_is_acquired_from_outside_vulkan_and_a_decoded_one_is_not() {
    let (layout, from, to) = acquire(vk::QUEUE_FAMILY_EXTERNAL, vk::ImageLayout::UNDEFINED, 0);
    assert_eq!((layout.as_raw(), from, to), (vk::ImageLayout::GENERAL.as_raw(), vk::QUEUE_FAMILY_EXTERNAL, 0));
    let (layout, from, to) = acquire(vk::QUEUE_FAMILY_IGNORED, vk::ImageLayout::VIDEO_DECODE_DPB_KHR, 0);
    assert_eq!((layout.as_raw(), from, to), (vk::ImageLayout::VIDEO_DECODE_DPB_KHR.as_raw(), vk::QUEUE_FAMILY_IGNORED, vk::QUEUE_FAMILY_IGNORED));
    assert_eq!(acquire(2, vk::ImageLayout::GENERAL, 2).1, vk::QUEUE_FAMILY_IGNORED);
}

/// An imported image went on to belong to the renderer's family after its copy; a kept import is
/// read again for its surface's next picture, so each copy gives it back to VA-API, in the
/// layout the next acquire starts from.
#[test]
fn an_imported_picture_is_released_back_to_vaapi_after_its_copy() {
    let image = vk::Image::from_raw(0x51);
    let barrier = release(image, 3);
    assert_eq!((barrier.src_queue_family_index, barrier.dst_queue_family_index), (3, vk::QUEUE_FAMILY_EXTERNAL));
    assert!((barrier.old_layout, barrier.new_layout) == (vk::ImageLayout::TRANSFER_SRC_OPTIMAL, vk::ImageLayout::GENERAL));
    assert!((barrier.src_stage_mask, barrier.src_access_mask) == (vk::PipelineStageFlags2::COPY, vk::AccessFlags2::TRANSFER_READ), "after the copy's reads");
    assert_eq!(barrier.image.as_raw(), image.as_raw());
    assert!((barrier.subresource_range.aspect_mask, barrier.subresource_range.level_count, barrier.subresource_range.layer_count) == (vk::ImageAspectFlags::COLOR, 1, 1));
    // The frame says so afterwards: the next copy acquires it from where the release left it.
    assert!(after_copy(vk::QUEUE_FAMILY_EXTERNAL) == (vk::ImageLayout::GENERAL, vk::QUEUE_FAMILY_EXTERNAL));
    let (layout, from, to) = acquire(after_copy(vk::QUEUE_FAMILY_EXTERNAL).1, after_copy(vk::QUEUE_FAMILY_EXTERNAL).0, 3);
    assert!((layout, from, to) == (barrier.new_layout, barrier.dst_queue_family_index, barrier.src_queue_family_index));
    // FFmpeg's own images stay where the copy left them, with their owner.
    assert!(after_copy(vk::QUEUE_FAMILY_IGNORED) == (vk::ImageLayout::TRANSFER_SRC_OPTIMAL, vk::QUEUE_FAMILY_IGNORED));
    assert!(after_copy(3) == (vk::ImageLayout::TRANSFER_SRC_OPTIMAL, 3));
}

/// Intel's iHD exports an NV12 surface as one Y-tiled object, chroma after luma in it.
fn exported(ten_bit: bool) -> crate::dmabuf::Descriptor {
    use crate::dmabuf::{Descriptor, Layer, Object};
    let (luma, chroma) = if ten_bit { (DRM_FORMAT_R16, DRM_FORMAT_GR1616) } else { (crate::dmabuf::DRM_FORMAT_R8, crate::dmabuf::DRM_FORMAT_GR88) };
    let pitch = if ten_bit { 4096 } else { 2048 };
    let mut descriptor = Descriptor { fourcc: fourcc(if ten_bit { b"P010" } else { b"NV12" }), width: 1920, height: 1088, num_objects: 1, num_layers: 2, ..Default::default() };
    descriptor.objects[0] = Object { fd: 7, size: 6_291_456, drm_format_modifier: 0x0100_0000_0000_0002 };
    descriptor.layers[0] = Layer { drm_format: luma, num_planes: 1, pitch: [pitch, 0, 0, 0], ..Default::default() };
    descriptor.layers[1] = Layer { drm_format: chroma, num_planes: 1, offset: [pitch * 1088, 0, 0, 0], pitch: [pitch, 0, 0, 0], ..Default::default() };
    descriptor
}

/// A kept import is made of the two layers `Planes` copies, in the formats its plane images
/// are in; anything else stays with FFmpeg's own mapping.
#[test]
fn only_two_one_plane_layers_of_the_pictures_depth_are_imported() {
    assert!(layer_format(crate::dmabuf::DRM_FORMAT_R8) == Some(vk::Format::R8_UNORM));
    assert!(layer_format(crate::dmabuf::DRM_FORMAT_GR88) == Some(vk::Format::R8G8_UNORM));
    assert!(layer_format(DRM_FORMAT_R16) == Some(vk::Format::R16_UNORM));
    assert!(layer_format(DRM_FORMAT_GR1616) == Some(vk::Format::R16G16_UNORM));
    assert_eq!((DRM_FORMAT_R16, DRM_FORMAT_GR1616), (0x2036_3152, 0x3233_5247), "drm_fourcc.h");
    for other in [fourcc(b"NV12"), fourcc(b"P010"), fourcc(b"RG88"), fourcc(b"XYUV")] {
        assert!(layer_format(other).is_none(), "{other:#x}");
    }
    for ten_bit in [false, true] {
        let formats = import_layers(&exported(ten_bit), ten_bit).unwrap();
        assert!([plane_format(0, ten_bit, false).0, plane_format(1, ten_bit, false).0] == formats, "the plane images' formats, so the copy is bit for bit");
        let why = import_layers(&exported(ten_bit), !ten_bit).err().expect("another depth is refused");
        assert!(why.starts_with("layer 0 is DRM format"), "another depth than the frames context's: {why}");
    }
    let mut composed = exported(false);
    composed.num_layers = 1;
    assert!(import_layers(&composed, false).err().is_some_and(|why| why.contains("1 layers in 1 objects")));
    let mut two_planes = exported(false);
    two_planes.layers[1].num_planes = 2;
    assert!(import_layers(&two_planes, false).err().is_some_and(|why| why.contains("with 2 planes")));
    let mut stray = exported(false);
    stray.layers[1].object_index[0] = 1;
    assert_eq!(import_layers(&stray, false).err().as_deref(), Some("layer 1 names object 1 of 1"));
    let mut none = exported(false);
    none.num_objects = 0;
    assert!(import_layers(&none, false).is_err());
}

/// Each layer's image is laid out in its object as the export says, chroma half size rounded up.
#[test]
fn a_layer_is_imported_at_its_offset_and_pitch_in_its_object() {
    let mut descriptor = exported(true);
    let (object, modifier, layout) = layer_layout(&descriptor, 1);
    assert_eq!((object, modifier), (0, 0x0100_0000_0000_0002));
    assert_eq!((layout.offset, layout.row_pitch), (4096 * 1088, 4096));
    assert_eq!((layout.size, layout.array_pitch, layout.depth_pitch), (0, 0, 0), "the specification asks for 0");
    // AMD's radeonsi may export each plane as an object of its own.
    descriptor.num_objects = 2;
    descriptor.objects[1] = crate::dmabuf::Object { fd: 8, size: 2_000_000, drm_format_modifier: 0x0200_0000_0000_0001 };
    descriptor.layers[1].object_index[0] = 1;
    descriptor.layers[1].offset[0] = 0;
    let (object, modifier, layout) = layer_layout(&descriptor, 1);
    assert_eq!((object, modifier, layout.offset), (1, 0x0200_0000_0000_0001, 0));
    assert_eq!(layer_layout(&descriptor, 0).0, 0);
    assert_eq!(layer_extent(0, 1919, 1081), (1919, 1081));
    assert_eq!(layer_extent(1, 1919, 1081), (960, 541));
}

/// The import's memory is of a type both the DMA-BUF and the image allow, device-local first;
/// none when they share none (FFmpeg's own mapping is used then).
#[test]
fn imported_memory_is_of_a_type_the_dmabuf_and_the_image_both_take() {
    let types = [
        vk::MemoryPropertyFlags::HOST_VISIBLE,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        vk::MemoryPropertyFlags::DEVICE_LOCAL | vk::MemoryPropertyFlags::HOST_VISIBLE,
    ];
    assert_eq!(import_memory_type(0b1111, 0b1111, &types), Some(1), "device-local first");
    assert_eq!(import_memory_type(0b1101, 0b1111, &types), Some(3));
    assert_eq!(import_memory_type(0b0101, 0b0111, &types), Some(0), "else the lowest both take");
    assert_eq!(import_memory_type(0b0010, 0b1101, &types), None);
    assert_eq!(import_memory_type(0b1_0000, 0b1_0000, &types), None, "a type the device does not have");
}

/// A kept import is handed to the copy as VA-API's again, whatever an earlier picture left in
/// its frame, with the semaphore values the last copy signalled.
#[test]
fn a_kept_import_is_handed_out_owned_by_vaapi_with_its_semaphore_values() {
    // In both libavutils' layouts: the larger frame's memory holds either.
    for layout in [StructLayout::V60, StructLayout::V61] {
        // SAFETY: A plain C struct, all-zero is valid, and nothing else uses it.
        let mut memory: VkFrame61 = unsafe { std::mem::zeroed() };
        let raw = std::ptr::addr_of_mut!(memory).cast::<VkFrame>();
        let mut frame = unsafe { layout.fields(raw) };
        for index in 0..2 {
            frame.img[index] = vk::Image::from_raw(0x10 + index as u64);
            frame.sem[index] = vk::Semaphore::from_raw(0x20 + index as u64);
            frame.sem_value[index] = 41;
            frame.layout[index] = vk::ImageLayout::TRANSFER_SRC_OPTIMAL;
            frame.queue_family[index] = 3;
            frame.access.set(index, vk::AccessFlags::TRANSFER_READ);
            assert!(frame.access.get(index) == vk::AccessFlags2::TRANSFER_READ, "{layout:?}");
        }
        frame.layout[0] = vk::ImageLayout::UNDEFINED;
        ready_for_reuse(&mut frame);
        // SAFETY: As above; the view is no longer used.
        let frame = unsafe { layout.read(raw) };
        for index in 0..2 {
            assert!(frame.layout[index] == vk::ImageLayout::GENERAL, "{layout:?} layer {index}");
            assert_eq!(frame.queue_family[index], vk::QUEUE_FAMILY_EXTERNAL, "{layout:?} layer {index}");
            assert!(frame.access[index] == vk::AccessFlags2::empty(), "{layout:?} layer {index}");
            assert_eq!(frame.sem_value[index], 41, "the copy waits for the last signal");
            assert!(acquire(frame.queue_family[index], frame.layout[index], 3) == (vk::ImageLayout::GENERAL, vk::QUEUE_FAMILY_EXTERNAL, 3));
        }
        assert!((frame.layout[2], frame.queue_family[2]) == (vk::ImageLayout::UNDEFINED, 0), "no image, nothing set");
        assert!(frame.internal.is_null() && frame.mem[0] == vk::DeviceMemory::null());
    }
}

#[test]
fn only_libavutil_59_34_to_61_has_the_structs_declared_here() {
    let version = |major: u32, minor: u32, micro: u32| major << 16 | minor << 8 | micro;
    assert!(ffmpeg_supported(version(59, 39, 100)).is_ok(), "FFmpeg 7.1");
    assert!(ffmpeg_supported(version(60, 26, 103)).is_ok(), "FFmpeg 8.1");
    let old = ffmpeg_supported(version(59, 8, 100)).unwrap_err();
    assert!(
        old.contains("59.8.100") && old.contains("FFmpeg 7.1"),
        "{old}"
    );
    assert!(
        ffmpeg_supported(version(58, 29, 100)).is_err(),
        "FFmpeg 6.1"
    );
    assert!(ffmpeg_supported(version(61, 1, 102)).is_ok(), "FFmpeg 9.0");
    assert!(ffmpeg_supported(version(61, 20, 100)).is_ok(), "61 keeps its layout");
    // 60.34 already had 61's access flags, in a frame that was neither layout.
    let between = ffmpeg_supported(version(60, 34, 100)).unwrap_err();
    assert!(between.contains("60.34.100") && between.contains("development version"), "{between}");
    let new = ffmpeg_supported(version(62, 0, 100)).unwrap_err();
    assert!(new.contains("62.0.100"), "{new}");
    assert_eq!(StructLayout::of(version(59, 39, 100)), StructLayout::V60);
    assert_eq!(StructLayout::of(version(60, 26, 103)), StructLayout::V60);
    assert_eq!(StructLayout::of(version(61, 1, 102)), StructLayout::V61);
    assert_eq!(StructLayout::V60.frame_size(), 504);
    assert_eq!(StructLayout::V61.frame_size(), 536);
}

#[test]
fn the_structs_have_libavutils_layout() {
    use std::mem::{offset_of, size_of};
    // Each field's offset (or size) here, then offsetof and sizeof with gcc on x86-64 in the
    // hwcontext_vulkan.h of FFmpeg 7.1 (libavutil 59.39) and of FFmpeg 8.1.2 (60.26), which
    // the release ships: 60 still has the fixed queue fields (FF_API_VULKAN_FIXED_QUEUES
    // lasts until 61) and lock_queue (FF_API_VULKAN_SYNC_QUEUES until 62). The third number
    // is the 61 struct's, against FFmpeg 9.0.2 (libavutil 61.1).
    let fields = [
        (
            "AVVulkanDeviceContext.get_proc_addr",
            offset_of!(DeviceContext60, get_proc_addr),
            8,
            8,
        ),
        (
            "AVVulkanDeviceContext.act_dev",
            offset_of!(DeviceContext60, act_dev),
            32,
            32,
        ),
        (
            "AVVulkanDeviceContext.device_features",
            offset_of!(DeviceContext60, device_features),
            40,
            40,
        ),
        (
            "AVVulkanDeviceContext.enabled_dev_extensions",
            offset_of!(DeviceContext60, enabled_dev_extensions),
            296,
            296,
        ),
        (
            "AVVulkanDeviceContext.nb_enabled_dev_extensions",
            offset_of!(DeviceContext60, nb_enabled_dev_extensions),
            304,
            304,
        ),
        (
            "AVVulkanDeviceContext.lock_queue",
            offset_of!(DeviceContext60, lock_queue),
            352,
            352,
        ),
        (
            "AVVulkanDeviceContext.unlock_queue",
            offset_of!(DeviceContext60, unlock_queue),
            360,
            360,
        ),
        (
            "AVVulkanDeviceContext.qf",
            offset_of!(DeviceContext60, qf),
            368,
            368,
        ),
        (
            "AVVulkanDeviceContext.nb_qf",
            offset_of!(DeviceContext60, nb_qf),
            1392,
            1392,
        ),
        (
            "sizeof AVVulkanDeviceQueueFamily",
            size_of::<QueueFamily>(),
            16,
            16,
        ),
        (
            "AVVulkanFramesContext.usage",
            offset_of!(FramesContext, usage),
            4,
            4,
        ),
        (
            "AVVulkanFramesContext.img_flags",
            offset_of!(FramesContext, img_flags),
            84,
            84,
        ),
        (
            "AVVulkanFramesContext.format",
            offset_of!(FramesContext, format),
            88,
            88,
        ),
        (
            "AVVulkanFramesContext.nb_layers",
            offset_of!(FramesContext, nb_layers),
            120,
            120,
        ),
        (
            "AVVulkanFramesContext.lock_frame",
            offset_of!(FramesContext, lock_frame),
            128,
            128,
        ),
        (
            "AVVulkanFramesContext.unlock_frame",
            offset_of!(FramesContext, unlock_frame),
            136,
            136,
        ),
        // The fields a kept VAAPI import fills (`imports::CachedImport`).
        ("AVVkFrame.img", offset_of!(VkFrame60, img), 0, 0),
        ("AVVkFrame.tiling", offset_of!(VkFrame60, tiling), 64, 64),
        ("AVVkFrame.mem", offset_of!(VkFrame60, mem), 72, 72),
        ("AVVkFrame.size", offset_of!(VkFrame60, size), 136, 136),
        ("AVVkFrame.flags", offset_of!(VkFrame60, flags), 200, 200),
        ("AVVkFrame.internal", offset_of!(VkFrame60, internal), 400, 400),
        ("AVVkFrame.access", offset_of!(VkFrame60, access), 204, 204),
        ("AVVkFrame.layout", offset_of!(VkFrame60, layout), 236, 236),
        ("AVVkFrame.sem", offset_of!(VkFrame60, sem), 272, 272),
        (
            "AVVkFrame.sem_value",
            offset_of!(VkFrame60, sem_value),
            336,
            336,
        ),
        ("AVVkFrame.offset", offset_of!(VkFrame60, offset), 408, 408),
        (
            "AVVkFrame.queue_family",
            offset_of!(VkFrame60, queue_family),
            472,
            472,
        ),
        ("sizeof AVVkFrame", size_of::<VkFrame60>(), 504, 504),
    ];
    for (name, ours, v59, v60) in fields {
        assert_eq!(ours, v59, "{name} in libavutil 59.39");
        assert_eq!(ours, v60, "{name} in libavutil 60.26");
    }
    assert_eq!(size_of::<DeviceContext60>(), 1400);
    assert_eq!(size_of::<FramesContext>(), 144);
    let fields61 = [
        ("AVVulkanDeviceContext.get_proc_addr", offset_of!(DeviceContext61, get_proc_addr), 8),
        ("AVVulkanDeviceContext.act_dev", offset_of!(DeviceContext61, act_dev), 32),
        ("AVVulkanDeviceContext.device_features", offset_of!(DeviceContext61, device_features), 40),
        ("AVVulkanDeviceContext.enabled_dev_extensions", offset_of!(DeviceContext61, enabled_dev_extensions), 296),
        ("AVVulkanDeviceContext.nb_enabled_dev_extensions", offset_of!(DeviceContext61, nb_enabled_dev_extensions), 304),
        ("AVVulkanDeviceContext.lock_queue", offset_of!(DeviceContext61, lock_queue), 312),
        ("AVVulkanDeviceContext.unlock_queue", offset_of!(DeviceContext61, unlock_queue), 320),
        ("AVVulkanDeviceContext.qf", offset_of!(DeviceContext61, qf), 328),
        ("AVVulkanDeviceContext.nb_qf", offset_of!(DeviceContext61, nb_qf), 1352),
        ("AVVulkanDeviceContext.queue_flags", offset_of!(DeviceContext61, queue_flags), 1356),
        ("sizeof AVVulkanDeviceContext", size_of::<DeviceContext61>(), 1360),
        ("AVVkFrame.img", offset_of!(VkFrame61, img), 0),
        ("AVVkFrame.tiling", offset_of!(VkFrame61, tiling), 64),
        ("AVVkFrame.mem", offset_of!(VkFrame61, mem), 72),
        ("AVVkFrame.size", offset_of!(VkFrame61, size), 136),
        ("AVVkFrame.flags", offset_of!(VkFrame61, flags), 200),
        ("AVVkFrame.access", offset_of!(VkFrame61, access), 208),
        ("AVVkFrame.layout", offset_of!(VkFrame61, layout), 272),
        ("AVVkFrame.sem", offset_of!(VkFrame61, sem), 304),
        ("AVVkFrame.sem_value", offset_of!(VkFrame61, sem_value), 368),
        ("AVVkFrame.offset", offset_of!(VkFrame61, offset), 432),
        ("AVVkFrame.queue_family", offset_of!(VkFrame61, queue_family), 496),
        ("AVVkFrame.internal", offset_of!(VkFrame61, internal), 528),
        ("sizeof AVVkFrame", size_of::<VkFrame61>(), 536),
    ];
    for (name, ours, v61) in fields61 {
        assert_eq!(ours, v61, "{name} in libavutil 61.1");
    }
}

#[test]
fn ffmpeg_hears_of_sdls_family_and_the_decode_families_sdl_made_a_queue_in() {
    let graphics =
        vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE | vk::QueueFlags::TRANSFER;
    let decode = vk::QueueFlags::VIDEO_DECODE_KHR | vk::QueueFlags::TRANSFER;
    let h264_hevc = vk::VideoCodecOperationFlagsKHR::DECODE_H264
        | vk::VideoCodecOperationFlagsKHR::DECODE_H265;
    let families = [
        (graphics, vk::VideoCodecOperationFlagsKHR::empty()),
        (
            vk::QueueFlags::COMPUTE,
            vk::VideoCodecOperationFlagsKHR::empty(),
        ),
        (decode, h264_hevc),
        (
            vk::QueueFlags::VIDEO_ENCODE_KHR,
            vk::VideoCodecOperationFlagsKHR::ENCODE_H264,
        ),
    ];
    let listed = listed_families(&families, 0, 1 << 2);
    // The flags compare as raw bits: ash's flag types print only with its debug feature.
    let summary: Vec<_> = listed
        .iter()
        .map(|family| {
            (
                family.idx,
                family.num,
                family.flags.as_raw(),
                family.video_caps.as_raw(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (0, 1, graphics.as_raw(), 0),
            (2, 1, decode.as_raw(), h264_hevc.as_raw())
        ]
    );
    assert_eq!(
        listed_families(&families, 0, 0).len(),
        1,
        "without SDL's decode queue only SDL's family"
    );
}

/// The frames are made in the two-plane format of the stream's depth and chroma; other streams
/// (4:2:2, 12-bit, grey) and a stream unlike the one the device was checked for are refused
/// before FFmpeg makes anything.
#[test]
fn the_frames_are_made_in_the_two_plane_format_of_the_checked_pictures() {
    use crate::media::ffi::*;
    for (ten_bit, chroma444) in [(false, false), (true, false), (false, true), (true, true)] {
        let format = picture_sw_format(ten_bit, chroma444);
        assert_eq!(
            depth_and_chroma(format),
            Some((ten_bit, chroma444)),
            "{format}"
        );
        // SAFETY: FFmpeg's static descriptor table.
        let planes = unsafe {
            let descriptor = &*av_pix_fmt_desc_get(format);
            (0..descriptor.nb_components as usize)
                .map(|c| descriptor.comp[c].plane)
                .max()
        };
        assert_eq!(planes, Some(1), "{format} has two planes");
    }
    let streams = [
        (AV_PIX_FMT_YUV420P, Some((false, false))),
        (AV_PIX_FMT_YUV420P10LE, Some((true, false))),
        (AV_PIX_FMT_YUV444P, Some((false, true))),
        (AV_PIX_FMT_YUV444P10LE, Some((true, true))),
        (AV_PIX_FMT_YUV422P, None),
        (AV_PIX_FMT_YUV420P12LE, None),
        (AV_PIX_FMT_GRAY8, None),
    ];
    for (stream, expected) in streams {
        assert_eq!(depth_and_chroma(stream), expected, "{stream}");
    }
    // SAFETY: A context allocated and freed here, with no device: refused before FFmpeg uses it.
    unsafe {
        let mut context = avcodec_alloc_context3(ptr::null());
        assert!(!context.is_null());
        let eight = Pictures::default();
        (*context).sw_pix_fmt = AV_PIX_FMT_YUV420P10LE;
        let error = frames_context(context, eight).unwrap_err();
        assert!(
            error.contains("10-bit 4:2:0, not the 8-bit 4:2:0"),
            "{error}"
        );
        (*context).sw_pix_fmt = AV_PIX_FMT_YUV422P;
        let error = frames_context(context, eight).unwrap_err();
        assert!(error.contains("not 8- or 10-bit 4:2:0 or 4:4:4"), "{error}");
        assert!((*context).hw_frames_ctx.is_null());
        avcodec_free_context(&mut context);
    }
}

/// SDL waits for its whole device under its queue lock, so FFmpeg's decode queues take that lock
/// too, not one of their own (`stream::vulkan_tests_unix` runs it against a device wait).
#[test]
fn every_queue_ffmpeg_submits_to_takes_sdls_queue_lock() {
    let mut fake = 0u8;
    let opaque = Opaque {
        chain: Chain::new(),
        extensions: Vec::new(),
        sdl_lock: (&raw mut fake).cast(),
    };
    for family in [0, 1, 3, 31] {
        assert_eq!(lock_of(&opaque, family), opaque.sdl_lock, "family {family}");
    }
}

/// FFmpeg takes the shared renderer's device for every codec its video extensions decode. Needs
/// a Vulkan device with video decoding (ANV only with `ANV_VIDEO_DECODE=1`) and SDL's offscreen
/// driver: `ANV_VIDEO_DECODE=1 cargo test -p nightjar-desktop -- --ignored --test-threads=1 vulkan_video`.
#[test]
#[ignore = "needs a Vulkan device with video decoding and SDL's offscreen driver"]
fn ffmpeg_takes_the_shared_renderers_device() {
    // SAFETY: Plain SDL calls on objects made here; each device context is freed before the renderer.
    unsafe {
        SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"offscreen".as_ptr());
        assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
        let window = SDL_CreateWindow(c"Vulkan video".as_ptr(), 640, 360, SDL_WINDOW_HIDDEN);
        let renderer = crate::sdl::create_renderer_for(window, true, true, true);
        assert!(!renderer.is_null(), "{}", crate::sdl::error());
        let gpu: *mut SDL_GPUDevice =
            crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER).cast();
        let props = SDL_GetGPUDeviceProperties(gpu);
        assert_ne!(
            SDL_GetNumberProperty(props, DECODE_FAMILIES.as_ptr(), 0),
            0,
            "SDL made no decode queue"
        );
        let extensions =
            Extensions(SDL_GetNumberProperty(props, EXTENSIONS_PROPERTY.as_ptr(), 0) as u32);
        let codecs: Vec<_> = [Codec::H264, Codec::Hevc, Codec::Av1]
            .into_iter()
            .filter(|&codec| extensions.decodes(codec))
            .collect();
        assert!(
            !codecs.is_empty(),
            "the renderer has no codec's video extensions"
        );
        for codec in codecs {
            let mut context = device_context(gpu, codec, Pictures::default())
                .unwrap_or_else(|e| panic!("{codec:?}: {e}"));
            av_buffer_unref(&mut context);
        }
        SDL_DestroyRenderer(renderer);
        SDL_DestroyWindow(window);
        SDL_Quit();
    }
}
