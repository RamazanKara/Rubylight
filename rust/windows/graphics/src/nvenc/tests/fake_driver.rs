//! Fake NVENC C ABI function table backing unit tests without NVIDIA hardware.

use super::*;

pub(in crate::nvenc) unsafe extern "C" fn create(
    table: *mut NV_ENCODE_API_FUNCTION_LIST,
) -> NVENCSTATUS {
    // SAFETY: `Session::open_version` passes its live, exclusive function table.
    let table = unsafe { &mut *table };
    assert_eq!(table.version, version(table.version).structure(2, false));
    table.nvEncOpenEncodeSessionEx = Some(open);
    table.nvEncGetEncodeCaps = Some(caps);
    table.nvEncGetEncodeGUIDCount = Some(guid_count);
    table.nvEncGetEncodeGUIDs = Some(guids);
    table.nvEncGetEncodePresetConfigEx = Some(preset_ex);
    table.nvEncGetEncodePresetConfig = Some(preset);
    table.nvEncInitializeEncoder = Some(initialize);
    table.nvEncRegisterResource = Some(register);
    table.nvEncUnregisterResource = Some(unregister);
    table.nvEncMapInputResource = Some(map);
    table.nvEncUnmapInputResource = Some(unmap);
    table.nvEncCreateBitstreamBuffer = Some(output);
    table.nvEncDestroyBitstreamBuffer = Some(destroy_output);
    table.nvEncRegisterAsyncEvent = Some(register_event);
    table.nvEncUnregisterAsyncEvent = Some(unregister_event);
    table.nvEncEncodePicture = Some(encode);
    table.nvEncLockBitstream = Some(lock);
    table.nvEncUnlockBitstream = Some(unlock);
    table.nvEncReconfigureEncoder = Some(reconfigure);
    table.nvEncInvalidateRefFrames = Some(invalidate);
    table.nvEncSetIOCudaStreams = Some(super::opt_tests::set_io_cuda_streams);
    table.nvEncDestroyEncoder = Some(destroy);
    SUCCESS
}
unsafe extern "C" fn open(
    parameters: *mut NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS,
    raw: *mut *mut c_void,
) -> NVENCSTATUS {
    // SAFETY: The session passes a pointer to its live parameter struct for this call.
    let p = unsafe { &*parameters };
    let api = version(p.apiVersion);
    assert_eq!(p.version, api.structure(1, false));
    // SAFETY: `raw` points at the session's `raw` field, writable for this call.
    unsafe {
        *raw = Box::into_raw(Box::new(api)).cast();
    }
    state(|d| {
        d.opened.push(api);
        // SAFETY: `raw` was written just above and still points at the session's field.
        d.active.insert(unsafe { *raw } as usize);
        if let Some(status) = d.open_status {
            status
        } else if d.reject_open == Some(api) {
            _NVENCSTATUS_NV_ENC_ERR_INVALID_VERSION
        } else {
            SUCCESS
        }
    })
}
unsafe extern "C" fn destroy(raw: *mut c_void) -> NVENCSTATUS {
    // SAFETY: `raw` is a live handle from `open`; it is freed only at the end of this call.
    let api = unsafe { handle(raw) };
    let status = state(|d| {
        assert!(d.active.remove(&(raw as usize)));
        if d.destroy_busy {
            assert!(!d.mapped.is_empty());
            assert!(!d.resources.is_empty());
            assert!(!d.outputs.is_empty());
            assert!(!d.events.is_empty());
            assert!(!d.calls.iter().any(|call| matches!(
                *call,
                "unmap input" | "unregister event" | "destroy output" | "unregister input"
            )));
            // Model the driver stopping its worker and releasing native
            // registrations only when the session itself is destroyed.
            d.mapped.clear();
            d.resources.clear();
            d.outputs.clear();
            d.events.clear();
        }
        d.destroyed.push(api);
        d.calls.push("destroy session");
        d.destroy_status.unwrap_or(SUCCESS)
    });
    // SAFETY: `raw` came from Box::into_raw in `open`, and each session is destroyed once.
    drop(unsafe { Box::from_raw(raw.cast::<ApiVersion>()) });
    status
}
unsafe extern "C" fn caps(
    raw: *mut c_void,
    _: GUID,
    parameters: *mut NV_ENC_CAPS_PARAM,
    value: *mut i32,
) -> NVENCSTATUS {
    // SAFETY: The session passes a pointer to its live parameter struct for this call.
    let p = unsafe { &*parameters };
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    assert_eq!(p.version, unsafe { handle(raw) }.structure(1, false));
    // SAFETY: `value` points at the session's live local out-value.
    unsafe {
        *value = state(|d| match p.capsToQuery {
            _NV_ENC_CAPS_NV_ENC_CAPS_WIDTH_MAX | _NV_ENC_CAPS_NV_ENC_CAPS_HEIGHT_MAX => 8192,
            _NV_ENC_CAPS_NV_ENC_CAPS_ASYNC_ENCODE_SUPPORT => i32::from(d.asynchronous),
            _NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_REF_PIC_INVALIDATION => i32::from(!d.no_rfi),
            _NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_MULTIPLE_REF_FRAMES => i32::from(!d.no_multiple_refs),
            _NV_ENC_CAPS_NV_ENC_CAPS_NUM_ENCODER_ENGINES => d.engines.max(1),
            _NV_ENC_CAPS_NV_ENC_CAPS_SUPPORTED_RATECONTROL_MODES => d.rate_modes.unwrap_or(0x3f),
            _NV_ENC_CAPS_NV_ENC_CAPS_MB_PER_SEC_MAX => d.macroblocks,
            _NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_SUBFRAME_READBACK => i32::from(!d.no_subframe),
            _NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_DYN_BITRATE_CHANGE => i32::from(!d.no_dynamic_bitrate),
            _NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_YUV444_ENCODE => i32::from(!d.no_yuv444),
            _NV_ENC_CAPS_NV_ENC_CAPS_SUPPORT_10BIT_ENCODE => i32::from(!d.no_10bit),
            _ => 1,
        });
    }
    SUCCESS
}
fn listed_codecs() -> Vec<GUID> {
    state(|d| d.codecs.clone())
        .unwrap_or(vec![0, 1, 2])
        .into_iter()
        .map(|codec| codec_guid(codec).unwrap())
        .collect()
}
unsafe extern "C" fn guid_count(_: *mut c_void, count: *mut u32) -> NVENCSTATUS {
    // SAFETY: `count` points at the session's live local out-value.
    unsafe { *count = listed_codecs().len() as u32 };
    SUCCESS
}
unsafe extern "C" fn guids(
    _: *mut c_void,
    out: *mut GUID,
    capacity: u32,
    count: *mut u32,
) -> NVENCSTATUS {
    let listed = listed_codecs();
    let written = listed.len().min(capacity as usize);
    // SAFETY: `out` holds `capacity` GUIDs and `count` is a live local out-value.
    unsafe {
        std::ptr::copy_nonoverlapping(listed.as_ptr(), out, written);
        *count = written as u32;
    }
    SUCCESS
}
unsafe extern "C" fn preset(
    raw: *mut c_void,
    codec: GUID,
    _: GUID,
    config: *mut NV_ENC_PRESET_CONFIG,
) -> NVENCSTATUS {
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    let api = unsafe { handle(raw) };
    // SAFETY: The session passes an exclusive pointer to its live preset struct.
    let p = unsafe { &mut *config };
    assert_eq!(p.version, api.preset());
    assert_eq!(p.presetCfg.version, api.config());
    // A quality preset may enable B frames/lookahead. The host must override it.
    p.presetCfg.frameIntervalP = 4;
    p.presetCfg.rcParams.set_enableLookahead(1);
    p.presetCfg.rcParams.lookaheadDepth = 16;
    if api >= ApiVersion(13, 0) && state(|d| d.preset_hdr_metadata) {
        // SAFETY: The requested codec selects the corresponding plain-data union member.
        unsafe {
            if codec.Data1 == NV_ENC_CODEC_HEVC_GUID.Data1 {
                p.presetCfg
                    .encodeCodecConfig
                    .hevcConfig
                    .set_outputMasteringDisplay(1);
                p.presetCfg.encodeCodecConfig.hevcConfig.set_outputMaxCll(1);
            } else if codec.Data1 == NV_ENC_CODEC_AV1_GUID.Data1 {
                p.presetCfg
                    .encodeCodecConfig
                    .av1Config
                    .set_outputMasteringDisplay(1);
                p.presetCfg.encodeCodecConfig.av1Config.set_outputMaxCll(1);
            }
        }
    }
    state(|d| d.calls.push("preset"));
    SUCCESS
}
unsafe extern "C" fn preset_ex(
    raw: *mut c_void,
    codec: GUID,
    guid: GUID,
    tuning: NV_ENC_TUNING_INFO,
    config: *mut NV_ENC_PRESET_CONFIG,
) -> NVENCSTATUS {
    assert_eq!(
        tuning,
        NV_ENC_TUNING_INFO_NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY
    );
    if state(|d| d.preset_fallback) {
        _NVENCSTATUS_NV_ENC_ERR_UNSUPPORTED_PARAM
    } else {
        // SAFETY: The arguments are this call's own, valid as `preset` requires.
        unsafe { preset(raw, codec, guid, config) }
    }
}
unsafe extern "C" fn initialize(
    raw: *mut c_void,
    parameters: *mut NV_ENC_INITIALIZE_PARAMS,
) -> NVENCSTATUS {
    // SAFETY: The session passes a pointer to its live parameter struct for this call.
    let p = unsafe { *parameters };
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    let api = unsafe { handle(raw) };
    assert_eq!(p.version, api.initialize());
    assert!(!p.encodeConfig.is_null());
    // SAFETY: `encodeConfig` is non-null (asserted above) and points at the session's boxed config.
    let config = unsafe { *p.encodeConfig };
    assert_eq!(config.version, api.config());
    state(|d| {
        d.initialized = Some((p, config));
        if d.reject_initialize == Some(api) {
            _NVENCSTATUS_NV_ENC_ERR_INVALID_VERSION
        } else if d.d3d12 && d.reject_d3d12_format == Some(p.bufferFormat) {
            _NVENCSTATUS_NV_ENC_ERR_INVALID_PARAM
        } else {
            SUCCESS
        }
    })
}
unsafe extern "C" fn register(
    raw: *mut c_void,
    parameters: *mut NV_ENC_REGISTER_RESOURCE,
) -> NVENCSTATUS {
    // SAFETY: The session passes an exclusive pointer to its live parameter struct for this call.
    let p = unsafe { &mut *parameters };
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    let api = unsafe { handle(raw) };
    assert_eq!(p.version, api.register());
    let wait = if p.pInputFencePoint.is_null() {
        None
    } else {
        assert_eq!(
            p.bufferUsage, _NV_ENC_BUFFER_USAGE_NV_ENC_INPUT_IMAGE,
            "nvEncodeAPI.h allows pInputFencePoint only on NV_ENC_INPUT_IMAGE"
        );
        // SAFETY: The caller passes a live NV_ENC_FENCE_POINT_D3D12 for this call.
        let point = unsafe { *p.pInputFencePoint };
        assert_eq!(point.version, api.structure(1, false));
        assert_eq!((point.bWait(), point.bSignal()), (1, 0));
        Some((point.pFence as usize, point.waitValue))
    };
    p.registeredResource = p.resourceToRegister;
    state(|d| {
        assert!(
            d.resources
                .insert(p.registeredResource as usize, p.bufferFormat)
                .is_none()
        );
        d.register_fence_points.push(wait);
        d.calls.push("register input");
    });
    SUCCESS
}
unsafe extern "C" fn unregister(_: *mut c_void, resource: NV_ENC_REGISTERED_PTR) -> NVENCSTATUS {
    if state(|d| d.reject_unregister) {
        return _NVENCSTATUS_NV_ENC_ERR_GENERIC;
    }
    state(|d| {
        if let Some(signaled) = &d.eos_signaled {
            assert!(
                signaled.load(std::sync::atomic::Ordering::Acquire),
                "unregister input ran before async EOS completionEvent signaled"
            );
        }
        assert!(!d.mapped.contains(&(resource as usize)));
        assert!(d.resources.remove(&(resource as usize)).is_some());
        d.calls.push("unregister input");
    });
    SUCCESS
}
unsafe extern "C" fn map(
    raw: *mut c_void,
    parameters: *mut NV_ENC_MAP_INPUT_RESOURCE,
) -> NVENCSTATUS {
    // SAFETY: The session passes an exclusive pointer to its live parameter struct for this call.
    let p = unsafe { &mut *parameters };
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    assert_eq!(p.version, unsafe { handle(raw) }.structure(4, false));
    p.mappedResource = p.registeredResource;
    state(|d| {
        p.mappedBufferFmt = d.resources[&(p.registeredResource as usize)];
        assert!(d.mapped.insert(p.mappedResource as usize));
        d.calls.push("map input");
    });
    SUCCESS
}
unsafe extern "C" fn unmap(_: *mut c_void, resource: NV_ENC_INPUT_PTR) -> NVENCSTATUS {
    state(|d| {
        assert!(d.mapped.remove(&(resource as usize)));
        d.calls.push("unmap input");
    });
    SUCCESS
}
unsafe extern "C" fn output(
    raw: *mut c_void,
    parameters: *mut NV_ENC_CREATE_BITSTREAM_BUFFER,
) -> NVENCSTATUS {
    // SAFETY: The session passes an exclusive pointer to its live parameter struct for this call.
    let p = unsafe { &mut *parameters };
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    assert_eq!(p.version, unsafe { handle(raw) }.structure(1, false));
    let mut buffer = Box::<Buffer>::default();
    p.bitstreamBuffer = (&mut *buffer as *mut Buffer).cast();
    state(|d| {
        d.outputs.insert(p.bitstreamBuffer as usize, buffer);
        d.calls.push("create output");
        if d.reject_output {
            _NVENCSTATUS_NV_ENC_ERR_OUT_OF_MEMORY
        } else {
            SUCCESS
        }
    })
}
unsafe extern "C" fn destroy_output(_: *mut c_void, output: NV_ENC_OUTPUT_PTR) -> NVENCSTATUS {
    state(|d| {
        if let Some(signaled) = &d.eos_signaled {
            assert!(
                signaled.load(std::sync::atomic::Ordering::Acquire),
                "destroy output ran before async EOS completionEvent signaled"
            );
        }
        let buffer = d.outputs.remove(&(output as usize)).unwrap();
        assert!(!buffer.locked);
        d.calls.push("destroy output");
    });
    SUCCESS
}
unsafe extern "C" fn register_event(
    raw: *mut c_void,
    parameters: *mut NV_ENC_EVENT_PARAMS,
) -> NVENCSTATUS {
    // SAFETY: The session passes a pointer to its live parameter struct for this call.
    let p = unsafe { &*parameters };
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    assert_eq!(p.version, unsafe { handle(raw) }.event());
    if state(|d| d.reject_register_event) {
        return _NVENCSTATUS_NV_ENC_ERR_GENERIC;
    }
    state(|d| assert!(d.events.insert(p.completionEvent as usize)));
    SUCCESS
}
unsafe extern "C" fn unregister_event(
    raw: *mut c_void,
    parameters: *mut NV_ENC_EVENT_PARAMS,
) -> NVENCSTATUS {
    // SAFETY: The session passes a pointer to its live parameter struct for this call.
    let p = unsafe { &*parameters };
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    assert_eq!(p.version, unsafe { handle(raw) }.event());
    state(|d| {
        if let Some(signaled) = &d.eos_signaled {
            assert!(
                signaled.load(std::sync::atomic::Ordering::Acquire),
                "unregister event ran before async EOS completionEvent signaled"
            );
        }
        assert!(d.events.remove(&(p.completionEvent as usize)));
        d.calls.push("unregister event");
    });
    SUCCESS
}
unsafe extern "C" fn encode(raw: *mut c_void, parameters: *mut NV_ENC_PIC_PARAMS) -> NVENCSTATUS {
    // SAFETY: The session passes a pointer to its live parameter struct for this call.
    let p = unsafe { &*parameters };
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    assert_eq!(p.version, unsafe { handle(raw) }.picture());
    if p.encodePicFlags & _NV_ENC_PIC_FLAGS_NV_ENC_PIC_FLAG_EOS != 0 {
        // SAFETY: The parameters are plain old data; this copy outlives the call.
        let eos = unsafe { std::ptr::read(p) };
        return state(|d| {
            d.eos_pictures.push(eos);
            d.calls.push("eos");
            let status = d.eos_status.unwrap_or(SUCCESS);
            if status == SUCCESS && !p.completionEvent.is_null() {
                assert!(d.events.contains(&(p.completionEvent as usize)));
                if !d.hold_eos_event {
                    let event_raw = p.completionEvent as usize;
                    let delay = d.eos_delay_ms;
                    let signaled = d.eos_signaled.clone();
                    if delay > 0 {
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(delay));
                            if let Some(flag) = signaled {
                                flag.store(true, std::sync::atomic::Ordering::Release);
                            }
                            // SAFETY: Teardown waits for this live event before closing it.
                            unsafe {
                                let _ = SetEvent(HANDLE(event_raw as *mut c_void));
                            }
                        });
                    } else {
                        if let Some(flag) = signaled {
                            flag.store(true, std::sync::atomic::Ordering::Release);
                        }
                        // SAFETY: `completionEvent` is a live registered event (asserted above).
                        unsafe {
                            SetEvent(HANDLE(p.completionEvent)).unwrap();
                        }
                    }
                }
            }
            status
        });
    }
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    let api = unsafe { handle(raw) };
    state(|d| {
        // SAFETY: The parameters are plain old data; this copy outlives the call.
        d.pictures.push(unsafe { std::ptr::read(p) });
        let mut input = p.inputBuffer;
        let mut signal = None;
        if d.d3d12 {
            // SAFETY: A D3D12 session passes its slot's boxed resource structures, live while the
            // frame is pending.
            let (resource, output) = unsafe {
                (
                    &*(p.inputBuffer as *const NV_ENC_INPUT_RESOURCE_D3D12),
                    &*(p.outputBitstream as *const NV_ENC_OUTPUT_RESOURCE_D3D12),
                )
            };
            let version = api.structure(1, false);
            let (wait, done) = (resource.inputFencePoint, output.outputFencePoint);
            assert_eq!(
                [resource.version, output.version, wait.version, done.version],
                [version; 4]
            );
            assert_eq!(
                (wait.bWait(), wait.bSignal(), done.bWait(), done.bSignal()),
                (1, 0, 0, 1)
            );
            assert_eq!(done.pFence as usize, FAKE_FENCE);
            assert!(done.signalValue > d.fence_points.last().map_or(0, |point| point.2));
            assert!(d.mapped.contains(&(output.pOutputBuffer as usize)));
            assert_eq!(
                d.resources[&(output.pOutputBuffer as usize)],
                _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_U8
            );
            d.fence_points
                .push((wait.pFence as usize, wait.waitValue, done.signalValue));
            input = resource.pInputBuffer;
            signal = Some(done.signalValue);
        }
        assert!(d.mapped.contains(&(input as usize)));
        if d.reject_submit {
            return _NVENCSTATUS_NV_ENC_ERR_GENERIC;
        }
        if d.d3d12 {
            d.outputs.entry(p.outputBitstream as usize).or_default();
        }
        if let Some(value) = signal.filter(|_| !d.hold_fence) {
            d.fence = value;
        }
        let codec = state_codec(d);
        let buffer = d.outputs.get_mut(&(p.outputBitstream as usize)).unwrap();
        buffer.bytes = p.inputTimeStamp.to_le_bytes().to_vec();
        buffer.bytes.extend((8..d.picture).map(|i| i as u8));
        buffer.timestamp = p.inputTimeStamp;
        buffer.idr = p.encodePicFlags & _NV_ENC_PIC_FLAGS_NV_ENC_PIC_FLAG_FORCEIDR != 0;
        buffer.deltas = (!p.qpDeltaMap.is_null()).then(|| {
            // SAFETY: A non-null map holds `qpDeltaMapSize` bytes, live while the frame is pending.
            unsafe { std::slice::from_raw_parts(p.qpDeltaMap, p.qpDeltaMapSize as usize) }.to_vec()
        });
        d.maps.push(buffer.deltas.clone());
        let pointer = if codec == 1 {
            // SAFETY: An HEVC session fills this union member; it holds only integers and pointers.
            unsafe { p.codecPicParams.hevcPicParams.pMasteringDisplay }
        } else if codec == 2 {
            // SAFETY: An AV1 session fills this union member; it holds only integers and pointers.
            unsafe { p.codecPicParams.av1PicParams.pMasteringDisplay }
        } else {
            ptr::null_mut()
        };
        if !pointer.is_null() {
            // SAFETY: `pointer` is non-null and points at the slot's boxed metadata, live while the
            // frame is pending.
            buffer.metadata = Some((pointer, unsafe { (*pointer).maxLuma }));
        }
        let light = if codec == 1 {
            // SAFETY: An HEVC session fills this union member.
            unsafe { p.codecPicParams.hevcPicParams.pMaxCll }
        } else if codec == 2 {
            // SAFETY: An AV1 session fills this union member.
            unsafe { p.codecPicParams.av1PicParams.pMaxCll }
        } else {
            ptr::null_mut()
        };
        if !light.is_null() {
            // SAFETY: The pending slot owns this non-null metadata box.
            let values = unsafe { *light };
            buffer.light = Some((
                light,
                values.maxContentLightLevel,
                values.maxPicAverageLightLevel,
            ));
        }
        if !p.completionEvent.is_null() {
            assert!(d.events.contains(&(p.completionEvent as usize)));
            if !d.hold_event {
                // SAFETY: `completionEvent` is the slot's live event, registered with this driver
                // (asserted above).
                unsafe {
                    SetEvent(HANDLE(p.completionEvent)).unwrap();
                }
            }
        }
        if d.need_more_input {
            _NVENCSTATUS_NV_ENC_ERR_NEED_MORE_INPUT
        } else {
            SUCCESS
        }
    })
}
unsafe extern "C" fn lock(raw: *mut c_void, parameters: *mut NV_ENC_LOCK_BITSTREAM) -> NVENCSTATUS {
    // SAFETY: The session passes an exclusive pointer to its live parameter struct for this call.
    let p = unsafe { &mut *parameters };
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    assert_eq!(p.version, unsafe { handle(raw) }.lock());
    state(|d| {
        d.lock_do_not_wait.push(p.doNotWait());
        if d.lock_busy > 0 {
            d.lock_busy -= 1;
            return _NVENCSTATUS_NV_ENC_ERR_LOCK_BUSY;
        }
        if d.keyframe_busy > 0 && d.outputs[&(p.outputBitstream as usize)].idr {
            d.keyframe_busy -= 1;
            return _NVENCSTATUS_NV_ENC_ERR_LOCK_BUSY;
        }
        let slices = configured_slices(d);
        let buffer = d.outputs.get_mut(&(p.outputBitstream as usize)).unwrap();
        if let Some((pointer, expected)) = buffer.metadata {
            assert_eq!(
                // SAFETY: The slot owning `pointer` is still pending, so its metadata box is live.
                unsafe { (*pointer).maxLuma },
                expected,
                "in-flight HDR metadata was overwritten"
            );
        }
        if let Some((pointer, cll, fall)) = buffer.light {
            // SAFETY: The slot owning the metadata is still pending.
            let values = unsafe { *pointer };
            assert_eq!(
                (values.maxContentLightLevel, values.maxPicAverageLightLevel),
                (cll, fall),
                "in-flight content light metadata was overwritten"
            );
        }
        assert!(!buffer.locked);
        buffer.locked = true;
        p.bitstreamBufferPtr = buffer.bytes.as_mut_ptr().cast();
        p.bitstreamSizeInBytes = buffer.bytes.len() as u32;
        if let Some((written, bytes)) = d.partial.pop_front() {
            assert_eq!(p.doNotWait(), 1, "a subframe lock does not wait");
            assert!(!p.sliceOffsets.is_null());
            p.numSlices = written;
            p.bitstreamSizeInBytes = bytes as u32;
            p.hwEncodeStatus = 1;
            for slice in 0..written as usize {
                let offset = if d.bad_offsets {
                    bytes
                } else {
                    slice * bytes / written as usize
                };
                // SAFETY: `sliceOffsets` holds an entry per macroblock, more than any test's
                // slices.
                unsafe { *p.sliceOffsets.add(slice) = offset as u32 };
            }
        } else {
            p.numSlices = slices;
            p.hwEncodeStatus = 2;
        }
        if !p.outputStatsPtr.is_null() {
            // Each block's QP is 30 moved by the picture's delta map.
            let size = std::mem::size_of::<NV_ENC_OUTPUT_STATS_BLOCK>();
            let blocks = p.outputStatsPtrSize as usize / size;
            for index in 0..blocks {
                // SAFETY: The session passes a buffer of `outputStatsPtrSize` bytes.
                let block = unsafe {
                    &mut *p
                        .outputStatsPtr
                        .cast::<NV_ENC_OUTPUT_STATS_BLOCK>()
                        .add(index)
                };
                // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
                assert_eq!(block.version, unsafe { handle(raw) }.structure(1, false));
                let delta = buffer.deltas.as_ref().map_or(0, |deltas| deltas[index]);
                block.QP = (30 + i32::from(delta)) as u8;
            }
        }
        p.outputTimeStamp = buffer.timestamp;
        p.pictureType = if buffer.idr {
            _NV_ENC_PIC_TYPE_NV_ENC_PIC_TYPE_IDR
        } else {
            _NV_ENC_PIC_TYPE_NV_ENC_PIC_TYPE_P
        };
        d.calls.push("lock output");
        SUCCESS
    })
}
unsafe extern "C" fn unlock(_: *mut c_void, output: NV_ENC_OUTPUT_PTR) -> NVENCSTATUS {
    state(|d| {
        let buffer = d.outputs.get_mut(&(output as usize)).unwrap();
        assert!(buffer.locked);
        buffer.locked = false;
        if d.d3d12 {
            // The READBACK buffer holds one picture at a time.
            d.outputs.remove(&(output as usize));
        }
        d.calls.push("unlock output");
    });
    SUCCESS
}
unsafe extern "C" fn reconfigure(
    raw: *mut c_void,
    parameters: *mut NV_ENC_RECONFIGURE_PARAMS,
) -> NVENCSTATUS {
    // SAFETY: The session passes a pointer to its live parameter struct for this call.
    let p = unsafe { *parameters };
    // SAFETY: `raw` is a live handle that `open` boxed and `destroy` has not freed.
    assert_eq!(p.version, unsafe { handle(raw) }.reconfigure());
    // SAFETY: `encodeConfig` is the boxed config `Session::bitrate` keeps alive for this call.
    let config = unsafe { *p.reInitEncodeParams.encodeConfig };
    state(|d| {
        d.reconfigure_count += 1;
        d.reconfigured = Some((p, config));
        if d.reject_reconfigure || (d.reject_in_place && p.resetEncoder() == 0) {
            _NVENCSTATUS_NV_ENC_ERR_INVALID_PARAM
        } else {
            SUCCESS
        }
    })
}
unsafe extern "C" fn invalidate(_: *mut c_void, frame: u64) -> NVENCSTATUS {
    state(|d| {
        if d.reject_invalidate == Some(frame) {
            _NVENCSTATUS_NV_ENC_ERR_INVALID_PARAM
        } else {
            d.invalidated.push(frame);
            SUCCESS
        }
    })
}
