use super::{tests::make_image, *};
use crate::capture::{ComGuard, Device, GpuImage};
use butterpollo_core::{packet::VideoPacketizer, rtsp::Negotiated};
use std::time::{Duration, Instant};
use windows::{
    Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile},
    core::PCSTR,
};

#[test]
#[ignore = "renders a synthetic 1440p scene and compares twelve bounded AMD encoding runs"]
fn packed444_synthetic_performance() -> Result<()> {
    compare(2560, 1440, 120, 140_000)
}

#[test]
#[ignore = "compares packed layouts at 1080p60 through AMD encoding and transport framing"]
fn packed444_1080p60_performance() -> Result<()> {
    compare(1920, 1080, 60, 20_000)
}

fn compare(width: u32, height: u32, fps: usize, bitrate_kbps: u32) -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let mut image = GpuImage::upload(
        &gpu,
        &make_image(&[[0.; 3]], width as usize, height as usize),
    )?;
    let source = br#"
cbuffer Scene : register(b0) { float width; float height; float time; float hdr; };
RWTexture2D<float4> output : register(u0);
float4 shade(float4 position) {
    float2 uv = (2 * position.xy - float2(width, height)) / height;
    float3 origin = float3(sin(time * 0.4), 1.5, time * 1.5);
    float3 ray = normalize(float3(uv.x, -uv.y - 0.25, 1.5));
    float nearest = ray.y < -0.001 ? -origin.y / ray.y : 1000;
    float3 ground = origin + ray * nearest;
    float grid = frac((floor(ground.x * 2) + floor(ground.z * 2)) * 0.5) * 2;
    float3 rgb = nearest < 1000 ? lerp(float3(0.03, 0.12, 0.03), float3(0.4, 0.2, 0.03), grid) :
        float3(0.1, 0.3, 0.7) * (1 + ray.y);
    [unroll] for (int i = 0; i < 12; ++i) {
        float3 center = float3((i % 4 - 1.5) * 1.7 + sin(time + i) * 0.3,
            0.8 + sin(time * 2 + i) * 0.3, floor(time * 1.5 / 12) * 12 + 3 + (i / 4) * 3);
        float3 delta = origin - center;
        float b = dot(delta, ray);
        float d = b * b - dot(delta, delta) + 0.5;
        float hit = -b - sqrt(max(d, 0));
        if (d > 0 && hit > 0 && hit < nearest) {
            nearest = hit;
            float3 normal = normalize(origin + ray * hit - center);
            float diffuse = max(dot(normal, normalize(float3(-1, 2, -1))), 0);
            float specular = pow(max(dot(reflect(ray, normal), normalize(float3(-1, 2, -1))), 0), 48);
            rgb = (0.5 + 0.5 * cos(float3(0, 2, 4) + i)) * (0.08 + diffuse) + specular * (hdr != 0 ? 12.5 : 1);
        }
    }
    if (position.y < 24 && position.x < 512) {
        rgb = (uint(position.x + time * 120) & 1) != 0 ? float3(1, 0, 1) : float3(0, 1, 0);
    }
    return float4(rgb, 1);
}
[numthreads(8, 8, 1)]
void scene(uint3 id : SV_DispatchThreadID) {
    if (id.x < uint(width) && id.y < uint(height)) {
        output[id.xy] = shade(float4(id.x + 0.5, id.y + 0.5, 0, 1));
    }
}
"#;
    let scene = Scene::new(&gpu, &mut image, source)?;
    let timer = crate::timing::Timer::new()?;
    let period = Duration::from_nanos(1_000_000_000 / fps as u64);
    let mut accepted = true;
    for codec in [1, 2] {
        for hdr in [false, true] {
            let mut baseline = 0.;
            for packing in [0, 1, 2] {
                let config = Negotiated {
                    width,
                    height,
                    codec,
                    hdr,
                    chroma_packing: packing,
                    fps: fps as u32,
                    bitrate_kbps,
                    ..Default::default()
                };
                let mut encoder = crate::amf::Encoder::new_device(&config, gpu.clone())?;
                let mut packetizer = VideoPacketizer {
                    sequence: 0,
                    iv_counter: 0,
                    frame: 1,
                    packet_size: 1392 - 32,
                    fec_percent: 20,
                    min_fec: 2,
                    key: Some([0; 16]),
                };
                let mut latencies = Vec::new();
                let mut wire = Vec::new();
                let mut first_packet = None;
                let mut observed = 0;
                let mut consume = |output: Vec<crate::encoder::Encoded>| -> Result<usize> {
                    let count = output.len();
                    for frame in output {
                        observed += 1;
                        let shards: usize = packetizer
                            .block_layout(frame.bytes.len())
                            .iter()
                            .map(|(data, parity)| data + parity)
                            .sum();
                        // Fixed-size encrypted RTP/FEC datagrams, including IPv6/UDP and
                        // Ethernet framing, preamble and interpacket gap (the host's budget).
                        let bytes = shards * (packetizer.packet_size + 16 + 32 + 40 + 8 + 38);
                        if observed == 1 {
                            first_packet = Some((frame.bytes, frame.idr, bytes));
                        }
                        if observed > fps {
                            latencies.push(
                                frame
                                    .presentation
                                    .context("missing scene timestamp")?
                                    .elapsed()
                                    .as_secs_f64()
                                    * 1000.,
                            );
                            wire.push(bytes);
                        }
                    }
                    Ok(count)
                };
                let mut received = 0;
                let mut maximum_pending = 0;
                let mut measured = Instant::now();
                let mut due = Instant::now();
                eprintln!("Synthetic scene: codec={codec}, hdr={hdr}, packing={packing}");
                for frame in 0..fps * 11 {
                    while Instant::now() < due {
                        received += consume(encoder.poll()?)?;
                        timer.until((Instant::now() + Duration::from_micros(250)).min(due));
                    }
                    anyhow::ensure!(
                        Instant::now().saturating_duration_since(due) < Duration::from_secs(1),
                        "synthetic scene stalled; stopping GPU tests"
                    );
                    if frame == fps {
                        measured = Instant::now();
                    }
                    image.captured = Instant::now();
                    let values = [
                        width as f32,
                        height as f32,
                        frame as f32 / fps as f32,
                        u8::from(hdr) as f32,
                    ];
                    // The scene and the conversion share the compute queue, so the next draw
                    // cannot overwrite a pending read, and the conversion waits for the scene.
                    scene.render(&gpu.compute, &mut image, values)?;
                    received += consume(encoder.encode_gpu(
                        &image,
                        frame % (fps * 2) == 0,
                        config.bitrate_kbps,
                    )?)?;
                    maximum_pending = maximum_pending.max(frame + 1 - received);
                    anyhow::ensure!(
                        frame + 1 - received <= 8,
                        "encoder queue exceeded eight frames; stopping GPU tests"
                    );
                    due += period;
                }
                let deadline = Instant::now() + Duration::from_secs(3);
                while encoder.pending() {
                    received += consume(encoder.poll()?)?;
                    anyhow::ensure!(
                        Instant::now() < deadline,
                        "encoder did not drain; stopping GPU tests"
                    );
                    timer.until(Instant::now() + Duration::from_micros(250));
                }
                let seconds = measured.elapsed().as_secs_f64();
                let (payload, idr, expected_wire_bytes) = first_packet.unwrap();
                let packets = packetizer.encode(&payload, idr, 0, 0)?;
                assert_eq!(
                    expected_wire_bytes,
                    packets.iter().map(|p| p.len() + 40 + 8 + 38).sum::<usize>()
                );
                assert_eq!(received, fps * 11);
                assert_eq!(latencies.len(), fps * 10);
                latencies.sort_by(f64::total_cmp);
                let p95 = latencies[(latencies.len() - 1) * 95 / 100];
                if packing == 0 {
                    baseline = p95;
                }
                let mbps = wire.iter().sum::<usize>() as f64 * 8. / seconds / 1_000_000.;
                let peak_mbps = wire
                    .windows(fps)
                    .map(|w| w.iter().sum::<usize>())
                    .max()
                    .unwrap() as f64
                    * 8.
                    / 1_000_000.;
                let actual_fps = (fps * 10) as f64 / seconds;
                let wire_budget = f64::from(bitrate_kbps) / 700.;
                let passed = actual_fps >= fps as f64 - 1.
                    && mbps <= wire_budget
                    && peak_mbps <= wire_budget
                    && p95 - baseline <= 1000. / fps as f64;
                accepted &= passed;
                eprintln!(
                    "{}",
                    serde_json::json!({"scope":"synthetic GPU scene through completed host encoding; excludes network and client presentation; transport bytes use the actual FEC layout", "adapter":gpu.display.adapter, "codec":codec, "hdr":hdr, "packing":packing, "fps":actual_fps, "frames":fps * 10, "p95_ms":p95, "added_p95_ms":p95-baseline, "maximum_pending":maximum_pending, "wire_mbps":mbps, "peak_one_second_wire_mbps":peak_mbps, "passed":passed})
                );
            }
        }
    }
    anyhow::ensure!(
        accepted,
        "synthetic host performance gate failed; keep packed mode experimental and disabled by default"
    );
    Ok(())
}

/// The synthetic scene as a D3D12 compute kernel that writes one FP16 texture in place.
struct Scene {
    root: ID3D12RootSignature,
    pipeline: ID3D12PipelineState,
    heap: ID3D12DescriptorHeap,
    frames: std::cell::RefCell<Vec<(ID3D12CommandAllocator, ID3D12GraphicsCommandList, u64)>>,
    next: std::cell::Cell<usize>,
    size: (u32, u32),
}
impl Scene {
    /// Replace `image`'s texture by one the kernel can write.
    fn new(gpu: &Device, image: &mut GpuImage, source: &[u8]) -> Result<Self> {
        let compute = &gpu.compute;
        let (width, height) = (image.width, image.height);
        // SAFETY: The descriptions and shader bytes outlive each D3D12 call, and every object
        // belongs to the live compute device used only on this test's thread.
        unsafe {
            let texture = compute.texture(
                width,
                height,
                DXGI_FORMAT_R16G16B16A16_FLOAT,
                D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
                D3D12_HEAP_FLAG_NONE,
            )?;
            let mut code = None;
            D3DCompile(
                source.as_ptr().cast(),
                source.len(),
                PCSTR::null(),
                None,
                None,
                PCSTR(c"scene".as_ptr().cast()),
                PCSTR(c"cs_5_0".as_ptr().cast()),
                D3DCOMPILE_OPTIMIZATION_LEVEL3,
                0,
                &mut code,
                None,
            )
            .context("compile synthetic scene shader")?;
            let code = code.context("synthetic scene shader")?;
            let bytecode = std::slice::from_raw_parts(
                code.GetBufferPointer().cast::<u8>(),
                code.GetBufferSize(),
            );
            let range = D3D12_DESCRIPTOR_RANGE {
                RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_UAV,
                NumDescriptors: 1,
                BaseShaderRegister: 0,
                RegisterSpace: 0,
                OffsetInDescriptorsFromTableStart: 0,
            };
            let parameters = [
                D3D12_ROOT_PARAMETER {
                    ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                    Anonymous: D3D12_ROOT_PARAMETER_0 {
                        Constants: D3D12_ROOT_CONSTANTS {
                            ShaderRegister: 0,
                            RegisterSpace: 0,
                            Num32BitValues: 4,
                        },
                    },
                    ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
                },
                D3D12_ROOT_PARAMETER {
                    ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                    Anonymous: D3D12_ROOT_PARAMETER_0 {
                        DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                            NumDescriptorRanges: 1,
                            pDescriptorRanges: &range,
                        },
                    },
                    ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
                },
            ];
            let mut blob = None;
            D3D12SerializeRootSignature(
                &D3D12_ROOT_SIGNATURE_DESC {
                    NumParameters: parameters.len() as u32,
                    pParameters: parameters.as_ptr(),
                    ..Default::default()
                },
                D3D_ROOT_SIGNATURE_VERSION_1,
                &mut blob,
                None,
            )?;
            let blob = blob.context("no scene root signature")?;
            let root: ID3D12RootSignature = compute.device.CreateRootSignature(
                0,
                std::slice::from_raw_parts(
                    blob.GetBufferPointer().cast::<u8>(),
                    blob.GetBufferSize(),
                ),
            )?;
            let pipeline: ID3D12PipelineState =
                compute
                    .device
                    .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                        pRootSignature: std::mem::transmute_copy(&root),
                        CS: D3D12_SHADER_BYTECODE {
                            pShaderBytecode: bytecode.as_ptr().cast(),
                            BytecodeLength: bytecode.len(),
                        },
                        ..Default::default()
                    })?;
            let heap: ID3D12DescriptorHeap =
                compute
                    .device
                    .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                        Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                        NumDescriptors: 1,
                        Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                        NodeMask: 0,
                    })?;
            compute.device.CreateUnorderedAccessView(
                &texture,
                None,
                Some(&D3D12_UNORDERED_ACCESS_VIEW_DESC {
                    Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
                    ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                        Texture2D: D3D12_TEX2D_UAV {
                            MipSlice: 0,
                            PlaneSlice: 0,
                        },
                    },
                }),
                heap.GetCPUDescriptorHandleForHeapStart(),
            );
            let mut frames = Vec::new();
            for _ in 0..4 {
                let allocator: ID3D12CommandAllocator = compute
                    .device
                    .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COMPUTE)?;
                let list: ID3D12GraphicsCommandList = compute.device.CreateCommandList(
                    0,
                    D3D12_COMMAND_LIST_TYPE_COMPUTE,
                    &allocator,
                    None,
                )?;
                list.Close()?;
                frames.push((allocator, list, 0));
            }
            image.texture = Arc::new(texture);
            Ok(Self {
                root,
                pipeline,
                heap,
                frames: frames.into(),
                next: 0.into(),
                size: (width, height),
            })
        }
    }

    /// Draw one frame into `image`; later work on the compute queue sees the result.
    fn render(&self, compute: &Arc<Compute>, image: &mut GpuImage, values: [f32; 4]) -> Result<()> {
        let index = self.next.get();
        self.next.set((index + 1) % 4);
        let (allocator, list, value) = self.frames.borrow()[index].clone();
        compute.wait(value)?;
        let mut barriers = [
            transition(
                &image.texture,
                D3D12_RESOURCE_STATE_COMMON,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            ),
            transition(
                &image.texture,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_COMMON,
            ),
        ];
        // SAFETY: The frame's previous submission finished (`wait`), and the list references only
        // objects this scene and `image` keep alive until the fence value returned by `execute`.
        let result = unsafe {
            (|| -> Result<u64> {
                allocator.Reset()?;
                list.Reset(&allocator, &self.pipeline)?;
                list.SetComputeRootSignature(&self.root);
                list.SetDescriptorHeaps(&[Some(self.heap.clone())]);
                list.SetComputeRoot32BitConstants(0, 4, values.as_ptr().cast(), 0);
                list.SetComputeRootDescriptorTable(
                    1,
                    self.heap.GetGPUDescriptorHandleForHeapStart(),
                );
                list.ResourceBarrier(&barriers[..1]);
                list.Dispatch(self.size.0.div_ceil(8), self.size.1.div_ceil(8), 1);
                list.ResourceBarrier(&barriers[1..]);
                list.Close()?;
                compute.execute(&list)
            })()
        };
        drop_barriers(&mut barriers);
        let value = result?;
        self.frames.borrow_mut()[index].2 = value;
        image.ready = Some(Ready::on(compute, compute.fence.clone(), value));
        Ok(())
    }
}
