use super::*;
use butterpollo_core::{packet::VideoPacketizer, rtsp::Negotiated};
use std::time::Duration;

#[test]
#[ignore = "renders a synthetic 1440p scene and compares eight bounded AMD encoding runs"]
fn packed444_synthetic_performance() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let (width, height) = (2560, 1440);
    let mut image = GpuImage::upload(
        &gpu,
        &make_image(&[[0.; 3]], width as usize, height as usize),
    )?;
    let source = br#"
cbuffer Scene : register(b0) { float width; float height; float time; float hdr; };
float4 vertex(uint id : SV_VertexID) : SV_Position {
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * float2(2, -2) + float2(-1, 1), 0, 1);
}
float4 pixel(float4 position : SV_Position) : SV_Target {
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
"#;
    // SAFETY: The descriptors and shader bytes outlive each D3D11 call. All resources
    // belong to this device and are used only on this test's thread.
    let (target, vertex, pixel, constants) = unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        image.texture.GetDesc(&mut desc);
        desc.BindFlags |= D3D11_BIND_RENDER_TARGET.0 as u32;
        let mut texture = None;
        gpu.device
            .CreateTexture2D(&desc, None, Some(&mut texture))?;
        image.texture = Arc::new(texture.context("synthetic scene texture")?);
        let mut target = None;
        gpu.device
            .CreateRenderTargetView(image.texture.as_ref(), None, Some(&mut target))?;
        let mut bytecode = Vec::new();
        for (entry, profile) in [
            (b"vertex\0".as_slice(), b"vs_5_0\0".as_slice()),
            (b"pixel\0", b"ps_5_0\0"),
        ] {
            let mut code = None;
            D3DCompile(
                source.as_ptr().cast(),
                source.len(),
                PCSTR::null(),
                None,
                None,
                PCSTR(entry.as_ptr()),
                PCSTR(profile.as_ptr()),
                D3DCOMPILE_OPTIMIZATION_LEVEL3,
                0,
                &mut code,
                None,
            )
            .context("compile synthetic scene shader")?;
            let code = code.context("synthetic scene shader")?;
            bytecode.push(
                std::slice::from_raw_parts(
                    code.GetBufferPointer().cast::<u8>(),
                    code.GetBufferSize(),
                )
                .to_vec(),
            );
        }
        let mut vertex = None;
        let mut pixel = None;
        gpu.device
            .CreateVertexShader(&bytecode[0], None, Some(&mut vertex))?;
        gpu.device
            .CreatePixelShader(&bytecode[1], None, Some(&mut pixel))?;
        let mut constants = None;
        gpu.device.CreateBuffer(
            &D3D11_BUFFER_DESC {
                ByteWidth: 16,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                ..Default::default()
            },
            None,
            Some(&mut constants),
        )?;
        (
            target.context("scene target")?,
            vertex.unwrap(),
            pixel.unwrap(),
            constants.unwrap(),
        )
    };
    let timer = crate::timing::Timer::new()?;
    let period = Duration::from_nanos(1_000_000_000 / 120);
    let mut accepted = true;
    for codec in [1, 2] {
        for hdr in [false, true] {
            let mut baseline = 0.;
            for packing in [0, 1] {
                let config = Negotiated {
                    width,
                    height,
                    codec,
                    hdr,
                    chroma_packing: packing,
                    fps: 120,
                    bitrate_kbps: 140_000,
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
                        if observed > 120 {
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
                for frame in 0..1320 {
                    while Instant::now() < due {
                        received += consume(encoder.poll()?)?;
                        timer.until((Instant::now() + Duration::from_micros(250)).min(due));
                    }
                    anyhow::ensure!(
                        Instant::now().saturating_duration_since(due) < Duration::from_secs(1),
                        "synthetic scene stalled; stopping GPU tests"
                    );
                    if frame == 120 {
                        measured = Instant::now();
                    }
                    image.captured = Instant::now();
                    let values = [
                        width as f32,
                        height as f32,
                        frame as f32 / 120.,
                        u8::from(hdr) as f32,
                    ];
                    // SAFETY: This thread submits both the scene and conversion on the same
                    // immediate context, so the next draw cannot overwrite a pending read.
                    unsafe {
                        let c = &gpu.context;
                        c.UpdateSubresource(&constants, 0, None, values.as_ptr().cast(), 0, 0);
                        c.IASetInputLayout(None);
                        c.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                        c.VSSetShader(&vertex, None);
                        c.PSSetShader(&pixel, None);
                        c.PSSetConstantBuffers(0, Some(&[Some(constants.clone())]));
                        c.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
                        c.RSSetViewports(Some(&[D3D11_VIEWPORT {
                            Width: width as f32,
                            Height: height as f32,
                            MinDepth: 0.,
                            MaxDepth: 1.,
                            ..Default::default()
                        }]));
                        c.Draw(3, 0);
                        c.OMSetRenderTargets(None, None);
                    }
                    received += consume(encoder.encode_gpu(
                        &image,
                        frame % 240 == 0,
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
                assert_eq!(received, 1320);
                assert_eq!(latencies.len(), 1200);
                latencies.sort_by(f64::total_cmp);
                let p95 = latencies[(latencies.len() - 1) * 95 / 100];
                if packing == 0 {
                    baseline = p95;
                }
                let mbps = wire.iter().sum::<usize>() as f64 * 8. / seconds / 1_000_000.;
                let peak_mbps = wire
                    .windows(120)
                    .map(|w| w.iter().sum::<usize>())
                    .max()
                    .unwrap() as f64
                    * 8.
                    / 1_000_000.;
                let fps = 1200. / seconds;
                let passed =
                    fps >= 119. && mbps <= 200. && peak_mbps <= 200. && p95 - baseline <= 8.33;
                accepted &= passed;
                eprintln!(
                    "{}",
                    serde_json::json!({"scope":"synthetic GPU scene through completed host encoding; excludes network and client presentation; transport bytes use the actual FEC layout", "adapter":gpu.display.adapter, "codec":codec, "hdr":hdr, "packing":packing, "fps":fps, "frames":1200, "p95_ms":p95, "added_p95_ms":p95-baseline, "maximum_pending":maximum_pending, "wire_mbps":mbps, "peak_120_frame_wire_mbps":peak_mbps, "passed":passed})
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
