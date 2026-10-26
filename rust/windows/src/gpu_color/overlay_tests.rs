//! The couch menu overlay (docs/couch-menu-design.md sections 6.2 and 6.3)
//! on every format, colour mode and path. Closed, every conversion stays bit
//! for bit what it was before the overlay pass existed.
use super::*;
use butterpollo_core::rtsp::Negotiated;
use windows::Win32::Graphics::Direct3D12::{D3D12_FENCE_FLAG_NONE, ID3D12Fence};

const WIDTH: u32 = 128;
const HEIGHT: u32 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Yuv420,
    Pack1,
    Pack2,
    Pyro420,
    Pyro444,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Colour {
    Sdr709,
    Sdr709Ten,
    Bt2020,
    Pq,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    D3d12,
    D3d11Compute,
    D3d11Graphics,
    /// PyroWave 4:4:4 with one draw per plane instead of the MRT draw.
    D3d11Separate,
}
impl Kind {
    fn pyro(self) -> bool {
        matches!(self, Self::Pyro420 | Self::Pyro444)
    }
}
/// Every format, colour mode and path the converters have.
fn cases() -> Vec<(Kind, Colour, Route)> {
    let mut cases = Vec::new();
    for kind in [
        Kind::Yuv420,
        Kind::Pack1,
        Kind::Pack2,
        Kind::Pyro420,
        Kind::Pyro444,
    ] {
        for colour in [
            Colour::Sdr709,
            Colour::Sdr709Ten,
            Colour::Bt2020,
            Colour::Pq,
        ] {
            let routes: &[Route] = match kind {
                Kind::Pyro444 => &[Route::D3d12, Route::D3d11Graphics, Route::D3d11Separate],
                _ => &[Route::D3d12, Route::D3d11Compute, Route::D3d11Graphics],
            };
            for &route in routes {
                cases.push((kind, colour, route));
            }
        }
    }
    cases
}
fn config(kind: Kind, colour: Colour) -> Negotiated {
    Negotiated {
        width: WIDTH,
        height: HEIGHT,
        codec: if kind.pyro() { 3 } else { 1 },
        hdr: colour == Colour::Pq,
        sdr_10bit: matches!(colour, Colour::Sdr709Ten | Colour::Bt2020),
        csc_mode: if colour == Colour::Bt2020 { 4 } else { 2 },
        chroma_packing: match kind {
            Kind::Pack1 => 1,
            Kind::Pack2 => 2,
            _ => 0,
        },
        yuv444: kind == Kind::Pyro444,
        ..Default::default()
    }
}
struct Rig {
    _com: ComGuard,
    gpu: Device,
    compute: Arc<crate::compute::Compute>,
    /// The PyroWave planes' fence and its last value.
    fence: ID3D12Fence,
    value: u64,
    sdr: GpuImage,
    hdr: GpuImage,
}
impl Rig {
    fn new() -> Result<Self> {
        let _com = ComGuard::new()?;
        let gpu = Device::new("")?;
        let compute = crate::compute::Compute::for_device(&gpu.device)?;
        // SAFETY: `compute.device` is a live D3D12 device.
        let fence = unsafe { compute.device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? };
        let mut sdr = vec![0; (WIDTH * HEIGHT * 4) as usize];
        for (index, pixel) in sdr.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let v = (index as u32).wrapping_mul(2_654_435_761) >> 8;
            *pixel = [v as u8, (v >> 8) as u8, (v >> 16) as u8, 255];
        }
        let mut hdr = vec![0; (WIDTH * HEIGHT * 8) as usize];
        for (index, pixel) in hdr.as_chunks_mut::<8>().0.iter_mut().enumerate() {
            for channel in 0..3 {
                let value = ((index * 7 + channel * 13) % 97) as f32 / 8.;
                pixel[channel * 2..channel * 2 + 2]
                    .copy_from_slice(&half::f16::from_f32(value).to_bits().to_le_bytes());
            }
            pixel[6..].copy_from_slice(&half::f16::ONE.to_bits().to_le_bytes());
        }
        let image = |bytes, pixel, size| Image {
            width: WIDTH,
            height: HEIGHT,
            stride: (WIDTH * size) as usize,
            bytes,
            pixel,
            captured: Instant::now(),
        };
        Ok(Self {
            sdr: GpuImage::upload(&gpu, &image(sdr, Pixel::Bgra8, 4))?,
            hdr: GpuImage::upload(&gpu, &image(hdr, Pixel::RgbaF16, 8))?,
            _com,
            gpu,
            compute,
            fence,
            value: 0,
        })
    }
    /// The converted planes' samples as stored: NV12 and P010 as one plane
    /// (luma rows, then interleaved chroma rows), PyroWave as three.
    fn convert(&mut self, kind: Kind, colour: Colour, route: Route) -> Result<Vec<Vec<u16>>> {
        let config = config(kind, colour);
        let image = if config.hdr { &self.hdr } else { &self.sdr };
        let source = (image.width, image.height, image.pixel);
        let ten_bit = config.ten_bit();
        let planes = |textures: &[Arc<ID3D11Texture2D>]| -> Result<Vec<Vec<u16>>> {
            textures
                .iter()
                .map(|texture| Ok(samples(&plane_bytes(&self.gpu, texture)?, ten_bit)))
                .collect()
        };
        if kind.pyro() {
            if route == Route::D3d12 {
                let outputs = PlanarConverter::new(&self.gpu, &config, source)?;
                let mut converter =
                    crate::compute::Converter::new(self.compute.clone(), WIDTH, HEIGHT, ten_bit)?;
                converter.values = constants(&config, source, [100., 1.], None);
                converter.values[11] = u32::from(config.yuv444);
                let target = crate::compute::Planes {
                    planes: [
                        self.compute.open(&outputs.textures[0])?,
                        self.compute.open(&outputs.textures[1])?,
                        self.compute.open(&outputs.textures[2])?,
                    ],
                    format: if ten_bit {
                        DXGI_FORMAT_R16_UNORM
                    } else {
                        DXGI_FORMAT_R8_UNORM
                    },
                    fence: self.fence.clone(),
                    after: self.value,
                    done: self.value + 1,
                };
                self.value += 1;
                converter.convert_planes(
                    &self.compute.open(&image.texture)?,
                    crate::compute::format(image.pixel),
                    None,
                    None,
                    &target,
                    Some(image),
                )?;
                let deadline = Instant::now() + std::time::Duration::from_secs(2);
                // SAFETY: `fence` is a live D3D12 fence.
                while unsafe { self.fence.GetCompletedValue() } < self.value {
                    anyhow::ensure!(
                        Instant::now() < deadline,
                        "the plane conversion did not finish"
                    );
                    std::thread::yield_now();
                }
                return planes(&outputs.textures);
            }
            let mut converter = PlanarConverter::new(&self.gpu, &config, source)?;
            match route {
                Route::D3d11Compute => {
                    anyhow::ensure!(converter.compute.is_some(), "no D3D11 plane shader")
                }
                Route::D3d11Graphics => converter.compute = None,
                _ => {
                    converter.compute = None;
                    converter.combined = None;
                }
            }
            converter.convert(image, [100., 1.])?;
            return planes(&converter.textures);
        }
        if route == Route::D3d12 {
            let (width, height) = config.coded_size();
            let mut converter =
                crate::compute::Converter::new(self.compute.clone(), width, height, ten_bit)?;
            if config.chroma_packing != 0 {
                converter.pack_chroma()?;
            }
            converter.values = constants(&config, source, [100., 1.], None);
            let converted = converter.convert(
                &self.compute.open(&image.texture)?,
                crate::compute::format(image.pixel),
                None,
                image,
            )?;
            converted.wait()?;
            return Ok(vec![compute_samples(&self.compute, &converted.texture)?.0]);
        }
        let mut converter = Converter::new(&self.gpu, &config, source)?;
        if route != Route::D3d11Compute {
            converter.compute = None;
        }
        converter.set_luminance([100., 1.]);
        let texture = converter.convert(image)?;
        anyhow::ensure!(
            (route == Route::D3d11Compute) == converter.compute.is_some(),
            "{route:?} did not run"
        );
        planes(std::slice::from_ref(&texture))
    }
}
/// Bytes as stored samples: one byte each for eight-bit planes, else two.
fn samples(bytes: &[u8], ten_bit: bool) -> Vec<u16> {
    if ten_bit {
        bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes(*b))
            .collect()
    } else {
        bytes.iter().map(|&b| u16::from(b)).collect()
    }
}
/// FNV-1a over every sample of every plane.
fn hash(planes: &[Vec<u16>]) -> u64 {
    planes
        .iter()
        .flatten()
        .flat_map(|sample| sample.to_le_bytes())
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

/// The closed outputs on main before the overlay pass, on the RX 7900 XT.
const CLOSED: &[(&str, u64)] = &[
    ("Yuv420 Sdr709 D3d12", 0x613ef7fb3a26716a),
    ("Yuv420 Sdr709 D3d11Compute", 0x613ef7fb3a26716a),
    ("Yuv420 Sdr709 D3d11Graphics", 0x613ef7fb3a26716a),
    ("Yuv420 Sdr709Ten D3d12", 0x490385298497f9dc),
    ("Yuv420 Sdr709Ten D3d11Compute", 0x490385298497f9dc),
    ("Yuv420 Sdr709Ten D3d11Graphics", 0x490385298497f9dc),
    ("Yuv420 Bt2020 D3d12", 0x54770c899f250fad),
    ("Yuv420 Bt2020 D3d11Compute", 0x54770c899f250fad),
    ("Yuv420 Bt2020 D3d11Graphics", 0x54770c899f250fad),
    ("Yuv420 Pq D3d12", 0xaa87977314655b3c),
    ("Yuv420 Pq D3d11Compute", 0xaa87977314655b3c),
    ("Yuv420 Pq D3d11Graphics", 0xaa87977314655b3c),
    ("Pack1 Sdr709 D3d12", 0x4cd65e90bab5e11b),
    ("Pack1 Sdr709 D3d11Compute", 0x4cd65e90bab5e11b),
    ("Pack1 Sdr709 D3d11Graphics", 0x4cd65e90bab5e11b),
    ("Pack1 Sdr709Ten D3d12", 0x9700c79ca5ef4bcd),
    ("Pack1 Sdr709Ten D3d11Compute", 0x9700c79ca5ef4bcd),
    ("Pack1 Sdr709Ten D3d11Graphics", 0x9700c79ca5ef4bcd),
    ("Pack1 Bt2020 D3d12", 0xf01c608656ddb354),
    ("Pack1 Bt2020 D3d11Compute", 0xf01c608656ddb354),
    ("Pack1 Bt2020 D3d11Graphics", 0xf01c608656ddb354),
    ("Pack1 Pq D3d12", 0xd1ec14fe80c5ad2c),
    ("Pack1 Pq D3d11Compute", 0xd1ec14fe80c5ad2c),
    ("Pack1 Pq D3d11Graphics", 0xd1ec14fe80c5ad2c),
    ("Pack2 Sdr709 D3d12", 0x9988320cacd15aca),
    ("Pack2 Sdr709 D3d11Compute", 0x9988320cacd15aca),
    ("Pack2 Sdr709 D3d11Graphics", 0x9988320cacd15aca),
    ("Pack2 Sdr709Ten D3d12", 0x6b6eef343002d03a),
    ("Pack2 Sdr709Ten D3d11Compute", 0x6b6eef343002d03a),
    ("Pack2 Sdr709Ten D3d11Graphics", 0x6b6eef343002d03a),
    ("Pack2 Bt2020 D3d12", 0xe3daee80a22d2125),
    ("Pack2 Bt2020 D3d11Compute", 0xe3daee80a22d2125),
    ("Pack2 Bt2020 D3d11Graphics", 0xe3daee80a22d2125),
    ("Pack2 Pq D3d12", 0x94fe1d10bfa0617a),
    ("Pack2 Pq D3d11Compute", 0x94fe1d10bfa0617a),
    ("Pack2 Pq D3d11Graphics", 0x94fe1d10bfa0617a),
    ("Pyro420 Sdr709 D3d12", 0x979514d0449e42b5),
    ("Pyro420 Sdr709 D3d11Compute", 0x979514d0449e42b5),
    ("Pyro420 Sdr709 D3d11Graphics", 0x979514d0449e42b5),
    ("Pyro420 Sdr709Ten D3d12", 0x9788587fff49e472),
    ("Pyro420 Sdr709Ten D3d11Compute", 0x9788587fff49e472),
    ("Pyro420 Sdr709Ten D3d11Graphics", 0x9788587fff49e472),
    ("Pyro420 Bt2020 D3d12", 0x55df61fe93f43e0e),
    ("Pyro420 Bt2020 D3d11Compute", 0x55df61fe93f43e0e),
    ("Pyro420 Bt2020 D3d11Graphics", 0x55df61fe93f43e0e),
    ("Pyro420 Pq D3d12", 0xf606b1839b22fcb3),
    ("Pyro420 Pq D3d11Compute", 0xf606b1839b22fcb3),
    ("Pyro420 Pq D3d11Graphics", 0xf606b1839b22fcb3),
    ("Pyro444 Sdr709 D3d12", 0xae4993199093dfe3),
    ("Pyro444 Sdr709 D3d11Graphics", 0xae4993199093dfe3),
    ("Pyro444 Sdr709 D3d11Separate", 0xae4993199093dfe3),
    ("Pyro444 Sdr709Ten D3d12", 0xa975671f67ef7caf),
    ("Pyro444 Sdr709Ten D3d11Graphics", 0xa975671f67ef7caf),
    ("Pyro444 Sdr709Ten D3d11Separate", 0xa975671f67ef7caf),
    ("Pyro444 Bt2020 D3d12", 0x29861df1388b0284),
    ("Pyro444 Bt2020 D3d11Graphics", 0x29861df1388b0284),
    ("Pyro444 Bt2020 D3d11Separate", 0x29861df1388b0284),
    ("Pyro444 Pq D3d12", 0x94d47ec81312617b),
    ("Pyro444 Pq D3d11Graphics", 0x94d47ec81312617b),
    ("Pyro444 Pq D3d11Separate", 0x94d47ec81312617b),
];

#[test]
#[ignore = "requires D3D11 video UAVs and D3D12 compute"]
fn overlay_closed_outputs_match_their_fixture_hashes() -> Result<()> {
    let mut rig = Rig::new()?;
    let mut actual = Vec::new();
    for (kind, colour, route) in cases() {
        let planes = rig.convert(kind, colour, route)?;
        // A second run must agree, or the fixture would only record noise.
        assert_eq!(
            planes,
            rig.convert(kind, colour, route)?,
            "{kind:?} {colour:?} {route:?}"
        );
        actual.push((format!("{kind:?} {colour:?} {route:?}"), hash(&planes)));
    }
    let table: String = actual
        .iter()
        .map(|(label, hash)| format!("    (\"{label}\", {hash:#018x}),\n"))
        .collect();
    assert!(
        actual
            .iter()
            .map(|(label, hash)| (label.as_str(), *hash))
            .eq(CLOSED.iter().copied()),
        "closed outputs differ from the fixture; they hash to:\n{table}"
    );
    Ok(())
}
