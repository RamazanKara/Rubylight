use super::*;

fn caps(name: &str, kind: vk::PhysicalDeviceType, supported: Features) -> DeviceCaps {
    DeviceCaps { name: name.into(), kind, api: vk::API_VERSION_1_3, subgroup_ops: SUBGROUP_OPS, supported, video: Default::default() }
}

fn with(names: &[&str]) -> Features {
    Features(FEATURES.iter().enumerate().filter(|(_, feature)| names.contains(&feature.name)).fold(0, |bits, (bit, _)| bits | 1 << bit))
}

#[test]
fn only_four_features_are_required() {
    assert_eq!(Features::required().names(), ["subgroupSizeControl", "computeFullSubgroups", "synchronization2", "timelineSemaphore"]);
    assert_eq!(Features::all().names().len(), FEATURES.len());
}

/// The presenter samples the planes in place unless NIGHTJAR_PYROWAVE_SAMPLE_PLANES is 0, the
/// way back to the copy; sampling holds the shown set, so the ring gets one more.
#[test]
fn planes_are_sampled_in_place_unless_the_environment_says_0() {
    assert_eq!(PlaneHandoff::of(None), PlaneHandoff::Sample);
    assert_eq!(PlaneHandoff::of(Some("1".as_ref())), PlaneHandoff::Sample);
    assert_eq!(PlaneHandoff::of(Some("".as_ref())), PlaneHandoff::Sample);
    assert_eq!(PlaneHandoff::of(Some("0".as_ref())), PlaneHandoff::Copy);
    // queue_limit + 2 pictures in flight (the decoder's, the queue's, one spare), plus the shown one.
    assert_eq!(PlaneHandoff::Sample.sets(5), 6);
    assert_eq!(PlaneHandoff::Copy.sets(5), 5);
    assert!(PlaneHandoff::Copy.line().contains("NIGHTJAR_PYROWAVE_SAMPLE_PLANES=0"));
}

#[test]
fn a_device_with_everything_gets_everything_asked_for() {
    assert_eq!(plan(&[caps("RX 7900 XT", vk::PhysicalDeviceType::DISCRETE_GPU, Features::all())]), Ok(Features::all()));
}

#[test]
fn the_optional_features_asked_for_are_the_ones_every_candidate_has() {
    let a = Features::required().0 | with(&["shaderFloat16", "shaderInt16"]).0;
    let b = Features::required().0 | with(&["shaderInt16", "storageBuffer8BitAccess"]).0;
    let planned = plan(&[caps("a", vk::PhysicalDeviceType::DISCRETE_GPU, Features(a)), caps("b", vk::PhysicalDeviceType::DISCRETE_GPU, Features(b))]).unwrap();
    assert_eq!(planned, Features(Features::required().0 | with(&["shaderInt16"]).0));
}

#[test]
fn the_video_extensions_asked_for_are_the_ones_every_candidate_has_and_only_ones_that_decode() {
    use crate::vulkan_video::Extensions;
    let discrete = vk::PhysicalDeviceType::DISCRETE_GPU;
    let with_video = |name, kind, video| DeviceCaps { video: Extensions(video), ..caps(name, kind, Features::all()) };
    let rtx = with_video("RTX 4070", discrete, 0b11111);
    assert_eq!(plan_video(&[rtx.clone(), with_video("GTX 1660", discrete, 0b01111)]), Ok(Extensions(0b01111)), "no AV1 for either");
    // SDL picks the discrete GPU, so the integrated one's lack of video does not matter.
    assert_eq!(plan_video(&[rtx.clone(), with_video("iGPU", vk::PhysicalDeviceType::INTEGRATED_GPU, 0)]), Ok(Extensions(0b11111)));
    // Neither the video nor the import extensions: nothing to ask for (the renderer is made
    // without them, for PyroWave alone or as before Vulkan video).
    let none = Err("the Vulkan devices SDL picks from lack the video decoding extensions, the DMA-BUF import ones and the CUDA interop ones".to_string());
    assert_eq!(plan_video(&[with_video("queues only", discrete, 0b00011)]), none, "queues without a codec");
    assert_eq!(plan_video(&[with_video("a", discrete, 0b11111), with_video("b", discrete, 0b11100)]), none, "b has no queues");
    assert_eq!(plan_video(&[]), Err("Vulkan lists no devices".into()));
    // The DMA-BUF import extensions are asked for when every candidate has them, apart from
    // video decoding (nightjar#80: no Vulkan AV1 decoding, VAAPI pictures mapped instead).
    let import = Extensions(0b111 << 5);
    assert_eq!(plan_video(&[with_video("RADV", discrete, import.0)]), Ok(import));
    assert_eq!(plan_video(&[with_video("RADV", discrete, 0b1111_1111)]), Ok(Extensions(0b1111_1111)));
    assert_eq!(plan_video(&[with_video("a", discrete, 0b1111_1111), with_video("b", discrete, 0b0111_1111)]), Ok(Extensions(0b1_1111)), "b cannot import");
    assert_eq!(plan_video_for(&[with_video("RADV", discrete, 0b1111_1111)], Extensions::wanted(false, true)), Ok(import), "NIGHTJAR_VULKAN_VIDEO=0");
    // The CUDA interop extensions (external memory and semaphore FDs) are asked for when every
    // candidate has both, whatever the switches say, with or without the video or import ones.
    let interop = Extensions(1 << 5 | 1 << 8);
    assert_eq!(plan_video(&[with_video("NVIDIA", discrete, 0b1_1111_1111)]), Ok(Extensions(0b1_1111_1111)));
    assert_eq!(plan_video_for(&[with_video("NVIDIA", discrete, 0b1_0011_1111)], Extensions::wanted(true, false)), Ok(Extensions(0b1_0011_1111)));
    // Interop alone shares the device for NVDEC's copies (a driver without Vulkan video or
    // DMA-BUF import, or both switched off), and the plan says neither Vulkan video nor the
    // VAAPI mapping may use it.
    let alone = plan_video_for(&[with_video("NVIDIA", discrete, 0b1_1111_1111)], Extensions::wanted(false, false));
    assert_eq!(alone, Ok(interop), "interop alone is reason enough to share the device");
    let alone = alone.unwrap();
    assert!(alone.usable().is_empty() && alone.mapping().is_empty() && !alone.decodes(crate::media::Codec::Hevc));
    assert!(crate::vulkan_video::map_plan(true, Ok(alone), Ok(())).is_err(), "no VAAPI mapping without the import extensions");
    assert_eq!(crate::vulkan_video::cuda_plan(Ok(alone)), Ok(()));
    assert_eq!(plan_video(&[with_video("no video", discrete, interop.0)]), Ok(interop), "a driver with only the interop ones");
    // Vulkan video itself still needs the video ones: interop alone is not reason to pick the
    // GPU renderer for Vulkan video decoding.
    assert_eq!(vulkan_video_of(Ok(interop)), Err("the Vulkan devices SDL picks from lack the video decoding extensions".into()));
    assert_eq!(vulkan_video_of(Ok(Extensions(0b1_1111_1111))), Ok(Extensions(0b1_1111_1111)));
    assert_eq!(vulkan_video_of(Err("none".into())), Err("none".into()));
    assert_eq!(plan_video(&[with_video("a", discrete, 0b1_1111_1111), with_video("b", discrete, 0b0_1111_1111)]), Ok(Extensions(0b1111_1111)), "b cannot export semaphores");
    assert_eq!(plan_video(&[with_video("a", discrete, 0b1_0001_1111)]), Ok(Extensions(0b1_1111)), "a semaphore export without the memory one is no interop");
    assert_eq!(plan_video(&[with_video("NVIDIA", discrete, 0b1_1111_1111)]).map(|planned| planned.interop()), Ok(interop));
    assert_eq!(
        plan_video_for(&[with_video("RADV", discrete, import.0)], Extensions::wanted(true, false)),
        Err("the Vulkan devices SDL picks from lack the video decoding extensions and the CUDA interop ones".into()),
        "Vulkan video alone needs the video ones"
    );
    // Apart from PyroWave: a device without its subgroup operations and features decodes video
    // with synchronization2 and timeline semaphores alone, and not without them.
    let narrow = DeviceCaps { subgroup_ops: vk::SubgroupFeatureFlags::BASIC, supported: Features::video(), ..rtx.clone() };
    assert!(plan(std::slice::from_ref(&narrow)).is_err());
    assert_eq!(plan_video(&[narrow]), Ok(Extensions(0b11111)));
    let error = plan_video(&[DeviceCaps { supported: with(&["synchronization2"]), ..rtx.clone() }]).unwrap_err();
    assert!(error.contains("RTX 4070 lacks timelineSemaphore"), "{error}");
    let error = plan_video(&[DeviceCaps { api: vk::make_api_version(0, 1, 2, 0), ..rtx }]).unwrap_err();
    assert!(error.contains("supports Vulkan 1.2, FFmpeg's Vulkan video needs 1.3"), "{error}");
}

#[test]
fn each_half_of_the_request_is_asked_for_without_the_other() {
    use crate::vulkan_video::Extensions;
    assert_eq!(Features::video().names(), ["synchronization2", "timelineSemaphore", "samplerYcbcrConversion"]);
    let both = RendererRequest::halves(Some(Ok(Features::all())), Some(Ok(Extensions(0b111))));
    assert_eq!(both, Ok((Features::all(), Extensions(0b111))));
    // A device whose PyroWave plan has only the required features still gets video's for FFmpeg.
    let both = RendererRequest::halves(Some(Ok(Features::required())), Some(Ok(Extensions(0b111))));
    assert_eq!(both, Ok((Features(Features::required().0 | Features::video().0), Extensions(0b111))));
    assert_eq!(Features::video().without(Features::required()).names(), ["samplerYcbcrConversion"]);
    assert_eq!(RendererRequest::halves(Some(Ok(Features::all())), Some(Err("no video".into()))), Ok((Features::all(), Extensions(0))));
    assert_eq!(RendererRequest::halves(Some(Err("no subgroups".into())), Some(Ok(Extensions(0b111)))), Ok((Features::video(), Extensions(0b111))));
    assert_eq!(RendererRequest::halves(Some(Err("a".into())), Some(Err("b".into()))), Err("a; and for Vulkan video, b".into()));
}

/// A half nobody wants is not asked for: without Vulkan video the request is PyroWave's plan
/// alone, as before Vulkan video, and without PyroWave only video's features.
#[test]
fn a_half_nobody_wants_is_left_out_of_the_request() {
    use crate::vulkan_video::Extensions;
    assert_eq!(RendererRequest::halves(Some(Ok(Features::required())), None), Ok((Features::required(), Extensions(0))));
    assert_eq!(RendererRequest::halves(Some(Err("no subgroups".into())), None), Err("no subgroups".into()));
    assert_eq!(RendererRequest::halves(None, Some(Ok(Extensions(0b111)))), Ok((Features::video(), Extensions(0b111))));
    assert_eq!(RendererRequest::halves(None, Some(Err("b".into()))), Err("for Vulkan video, b".into()));
    assert!(RendererRequest::halves(None, None).is_err());
}

#[test]
fn only_devices_of_the_kind_sdl_picks_count() {
    // llvmpipe beside a GPU: SDL never picks the CPU device, so what it lacks does not matter.
    let cpu = DeviceCaps { api: vk::API_VERSION_1_1, ..caps("llvmpipe", vk::PhysicalDeviceType::CPU, Features::default()) };
    let gpu = caps("Iris Plus 655", vk::PhysicalDeviceType::INTEGRATED_GPU, Features::all());
    assert_eq!(plan(&[cpu.clone(), gpu]), Ok(Features::all()));
    // Alone, the CPU device is the one SDL picks, and it cannot share.
    let error = plan(&[cpu]).unwrap_err();
    assert!(error.contains("llvmpipe") && error.contains("1.1"), "{error}");
    // A discrete GPU beats the integrated one, so its gaps decide.
    let weak = caps("dGPU", vk::PhysicalDeviceType::DISCRETE_GPU, with(&["subgroupSizeControl", "synchronization2"]));
    let error = plan(&[caps("iGPU", vk::PhysicalDeviceType::INTEGRATED_GPU, Features::all()), weak]).unwrap_err();
    assert!(error.contains("dGPU lacks computeFullSubgroups, timelineSemaphore"), "{error}");
}

#[test]
fn a_device_without_the_subgroup_operations_or_vulkan_1_3_is_named_with_what_it_lacks() {
    let old = DeviceCaps { api: vk::make_api_version(0, 1, 2, 0), ..caps("old", vk::PhysicalDeviceType::DISCRETE_GPU, Features::all()) };
    assert!(plan(&[old]).unwrap_err().contains("old supports Vulkan 1.2, sharing it needs 1.3"));
    let narrow = DeviceCaps { subgroup_ops: SUBGROUP_OPS & !vk::SubgroupFeatureFlags::SHUFFLE_RELATIVE, ..caps("narrow", vk::PhysicalDeviceType::DISCRETE_GPU, Features::all()) };
    let error = plan(&[narrow]).unwrap_err();
    assert!(error.contains("narrow lacks subgroup operations") && error.contains("0x20"), "{error}");
    assert_eq!(plan(&[]), Err("Vulkan lists no devices".into()));
}

#[test]
fn features_go_into_the_chain_and_come_back_out() {
    let mut chain = Chain::new();
    assert_eq!(chain.features(), Features::default());
    let features = with(&["timelineSemaphore", "shaderInt16", "storageBuffer16BitAccess", "computeFullSubgroups"]);
    chain.enable(features);
    assert_eq!(chain.features(), features);
    assert_eq!((chain.v12.timeline_semaphore, chain.pdf2.features.shader_int16, chain.v11.storage_buffer16_bit_access, chain.v13.compute_full_subgroups), (vk::TRUE, vk::TRUE, vk::TRUE, vk::TRUE));
    assert_eq!(chain.v12.shader_float16, vk::FALSE);
    // Linked in place, PDF2 first, as Vulkan and Granite read it.
    assert_eq!(chain.pdf2.p_next, (&raw mut chain.v11).cast());
    assert_eq!(chain.v12.p_next, (&raw mut chain.v13).cast());
    assert!(chain.v13.p_next.is_null());
}

#[test]
fn only_one_forces_the_readback() {
    assert!(readback_forced(Some(OsStr::new("1"))));
    for value in [None, Some(""), Some("0"), Some("true"), Some("11")] {
        assert!(!readback_forced(value.map(OsStr::new)), "{value:?}");
    }
}

/// The renderer's device as SDL shares it: every handle, the queue lock and our feature mark,
/// or the reason the stream reads back instead.
#[test]
fn the_shared_device_needs_vulkan_the_patch_and_the_features_mark() {
    // SAFETY: Properties groups made and destroyed here (SDL's properties need no SDL_Init).
    unsafe {
        let props = SDL_CreateProperties();
        assert!(props.0 != 0, "{}", crate::sdl::error());
        let mut fake = [0u8; 5];
        let error = SdlVulkan::from_properties(props, "vulkan").unwrap_err();
        assert!(error.contains("SDL patch is missing"), "{error}");
        for (name, handle) in [INSTANCE, PHYSICAL_DEVICE, DEVICE, QUEUE, QUEUE_LOCK].into_iter().zip(fake.iter_mut()) {
            SDL_SetPointerProperty(props, name.as_ptr(), (handle as *mut u8).cast());
        }
        SDL_SetNumberProperty(props, QUEUE_FAMILY.as_ptr(), 2);
        let error = SdlVulkan::from_properties(props, "vulkan").unwrap_err();
        assert!(error.contains("created without the decoders' features"), "{error}");
        // Each decoder checks the features it needs.
        SDL_SetNumberProperty(props, FEATURES_PROPERTY.as_ptr(), i64::from(with(&["timelineSemaphore"]).0));
        let sdl = SdlVulkan::from_properties(props, "vulkan").unwrap();
        let error = sdl.require(Features::required()).unwrap_err();
        assert!(error.contains("without subgroupSizeControl, computeFullSubgroups, synchronization2"), "{error}");
        assert_eq!(sdl.require(Features::video()), Err("the renderer's Vulkan device was created without synchronization2, samplerYcbcrConversion".into()));
        SDL_SetNumberProperty(props, FEATURES_PROPERTY.as_ptr(), i64::from(Features::video().0));
        let sdl = SdlVulkan::from_properties(props, "vulkan").unwrap();
        assert_eq!(sdl.require(Features::video()), Ok(()));
        assert!(sdl.require(Features::required()).is_err());
        SDL_SetNumberProperty(props, FEATURES_PROPERTY.as_ptr(), i64::from(Features::all().0));
        let error = SdlVulkan::from_properties(props, "direct3d12").unwrap_err();
        assert!(error.contains("not Vulkan"), "{error}");
        let sdl = SdlVulkan::from_properties(props, "vulkan").unwrap();
        assert_eq!((sdl.family, sdl.features, sdl.device.as_raw()), (2, Features::all(), (&raw const fake[2]) as u64));
        assert_eq!(sdl.lock, (&raw mut fake[4]).cast());
        assert_eq!(sdl.compute, super::super::queue::RendererCompute::NotAsked, "a device made with the setting off");
        SDL_DestroyProperties(props);
    }
}

#[test]
fn the_share_info_points_into_itself_and_claims_exactly_the_device_features() {
    let sdl = SdlVulkan {
        instance: vk::Instance::from_raw(1), physical_device: vk::PhysicalDevice::from_raw(2), device: vk::Device::from_raw(3),
        queue: vk::Queue::from_raw(4), family: 5, lock: ptr::null_mut(), features: Features(Features::required().0 | with(&["shaderInt8"]).0),
        compute: super::super::queue::RendererCompute::NotAsked,
    };
    unsafe extern "system" fn loader(_: vk::Instance, _: *const std::ffi::c_char) -> vk::PFN_vkVoidFunction { None }
    let info = ShareInfo::new(&sdl, loader, None);
    // SAFETY: Every pointer is into the box, alive here.
    unsafe {
        let device = &*info.create.device_create_info;
        let queue = &*device.p_queue_create_infos;
        assert_eq!((device.queue_create_info_count, queue.queue_family_index, queue.queue_count, *queue.p_queue_priorities), (1, 5, 1, 1.0));
        let pdf2 = &*device.p_next.cast::<vk::PhysicalDeviceFeatures2>();
        assert_eq!(pdf2.s_type.as_raw(), vk::StructureType::PHYSICAL_DEVICE_FEATURES_2.as_raw());
        let v11 = &*pdf2.p_next.cast::<vk::PhysicalDeviceVulkan11Features>();
        let v12 = &*v11.p_next.cast::<vk::PhysicalDeviceVulkan12Features>();
        assert_eq!((v12.shader_int8, v12.timeline_semaphore, v12.shader_float16), (vk::TRUE, vk::TRUE, vk::FALSE));
        let app = &*(*info.create.instance_create_info).p_application_info;
        assert_eq!((app.api_version, (*info.create.instance_create_info).enabled_extension_count), (vk::API_VERSION_1_3, 0));
        let queues = &*info.create.queue_info;
        assert_eq!((info.create.queue_info_count, queues.queue.as_raw(), queues.family_index, queues.index), (1, 4, 5, 0));
        assert!(info.create.queue_lock_callback.is_some() && info.create.queue_unlock_callback.is_some());
    }
}

/// With the decodes on SDL's compute queue, Granite is told of it beside the graphics queue: in
/// a create info of its own family, or as a second queue of SDL's family.
#[test]
fn the_share_info_lists_the_compute_queue_the_decodes_run_on() {
    use super::super::queue::{QueueSlot, RendererCompute};
    let sdl = SdlVulkan {
        instance: vk::Instance::from_raw(1), physical_device: vk::PhysicalDevice::from_raw(2), device: vk::Device::from_raw(3),
        queue: vk::Queue::from_raw(4), family: 0, lock: ptr::null_mut(), features: Features::required(),
        compute: RendererCompute::NotAsked,
    };
    unsafe extern "system" fn loader(_: vk::Instance, _: *const std::ffi::c_char) -> vk::PFN_vkVoidFunction { None }
    // AMD: the compute family 1.
    let info = ShareInfo::new(&sdl, loader, Some((vk::Queue::from_raw(9), QueueSlot { family: 1, index: 0 })));
    // SAFETY: Every pointer is into the box, alive here.
    unsafe {
        let device = &*info.create.device_create_info;
        let infos = std::slice::from_raw_parts(device.p_queue_create_infos, device.queue_create_info_count as usize);
        let listed: Vec<_> = infos.iter().map(|info| (info.queue_family_index, info.queue_count, *info.p_queue_priorities)).collect();
        assert_eq!(listed, [(0, 1, 1.0), (1, 1, 1.0)]);
        let queues = std::slice::from_raw_parts(info.create.queue_info, info.create.queue_info_count as usize);
        let listed: Vec<_> = queues.iter().map(|queue| (queue.queue.as_raw(), queue.family_index, queue.index)).collect();
        assert_eq!(listed, [(4, 0, 0), (9, 1, 0)]);
    }
    // One family with two queues: the second is the decodes'.
    let info = ShareInfo::new(&sdl, loader, Some((vk::Queue::from_raw(9), QueueSlot { family: 0, index: 1 })));
    // SAFETY: As above.
    unsafe {
        let device = &*info.create.device_create_info;
        let queue_info = &*device.p_queue_create_infos;
        assert_eq!((device.queue_create_info_count, queue_info.queue_family_index, queue_info.queue_count), (1, 0, 2));
        assert_eq!(std::slice::from_raw_parts(queue_info.p_queue_priorities, 2), [1.0, 1.0]);
        let queues = std::slice::from_raw_parts(info.create.queue_info, info.create.queue_info_count as usize);
        let listed: Vec<_> = queues.iter().map(|queue| (queue.queue.as_raw(), queue.family_index, queue.index)).collect();
        assert_eq!(listed, [(4, 0, 0), (9, 0, 1)]);
    }
}

/// The planes are shared by SDL's family and the decodes' only when those differ.
#[test]
fn the_planes_are_concurrent_only_across_two_families() {
    assert!(sharing(0, None).is_empty());
    assert!(sharing(0, Some(0)).is_empty(), "a second queue of SDL's family");
    assert_eq!(sharing(0, Some(1)), [0, 1]);
    assert_eq!(sharing(2, Some(0)), [2, 0]);
}

/// The compute queue as the patched SDL reports it: not asked for, asked for with none, or the queue.
#[test]
fn the_renderers_compute_queue_is_read_from_sdls_properties() {
    use super::super::queue::{QueueSlot, RendererCompute};
    // SAFETY: A properties group made and destroyed here.
    unsafe {
        let props = SDL_CreateProperties();
        assert_eq!(compute_of(props), RendererCompute::NotAsked);
        SDL_SetNumberProperty(props, COMPUTE_FAMILY.as_ptr(), -1);
        SDL_SetPointerProperty(props, COMPUTE_QUEUE.as_ptr(), ptr::null_mut());
        assert_eq!(compute_of(props), RendererCompute::None);
        let mut fake = 0u8;
        SDL_SetNumberProperty(props, COMPUTE_FAMILY.as_ptr(), 1);
        SDL_SetNumberProperty(props, COMPUTE_INDEX.as_ptr(), 0);
        SDL_SetPointerProperty(props, COMPUTE_QUEUE.as_ptr(), (&raw mut fake).cast());
        assert_eq!(compute_of(props), RendererCompute::Queue(vk::Queue::from_raw((&raw mut fake) as u64), QueueSlot { family: 1, index: 0 }));
        SDL_SetNumberProperty(props, COMPUTE_INDEX.as_ptr(), 2);
        assert_eq!(compute_of(props), RendererCompute::None, "no index the patch hands out");
        SDL_DestroyProperties(props);
    }
}

#[test]
fn only_a_request_with_pyrowaves_half_asks_for_a_compute_queue() {
    use crate::vulkan_video::Extensions;
    let request = |pyrowave: Option<Result<Features, String>>, video: Option<Result<Extensions, String>>| {
        let (features, _) = RendererRequest::halves(pyrowave, video).unwrap();
        has_pyrowave(features)
    };
    assert!(request(Some(Ok(Features::required())), None));
    assert!(!request(None, Some(Ok(Extensions(0b11111)))), "Vulkan video's features alone");
    assert!(!request(Some(Err("no subgroups".into())), Some(Ok(Extensions(0b11111)))));
}
