//! The couch menu overlay (docs/couch-menu-design.md sections 6.2 and 6.3)
//! on every format, colour mode and path. Closed, every conversion stays bit
//! for bit what it was before the overlay pass existed.
use super::*;
use crate::overlay::{
    Overlay,
    reference::{self, Layout},
};
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
    fn layout(self) -> Layout {
        match self {
            Self::Yuv420 => Layout::Yuv420,
            Self::Pack1 => Layout::Pack(1),
            Self::Pack2 => Layout::Pack(2),
            Self::Pyro420 => Layout::Pyro { yuv444: false },
            Self::Pyro444 => Layout::Pyro { yuv444: true },
        }
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
    fn convert(
        &mut self,
        kind: Kind,
        colour: Colour,
        route: Route,
        overlay: Option<&Overlay>,
    ) -> Result<Vec<Vec<u16>>> {
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
                converter.set_overlay(overlay);
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
            converter.set_overlay(overlay);
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
            converter.set_overlay(overlay);
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
        converter.set_overlay(overlay);
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
        let planes = rig.convert(kind, colour, route, None)?;
        // A second run must agree, or the fixture would only record noise.
        assert_eq!(
            planes,
            rig.convert(kind, colour, route, None)?,
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

/// A 64x32 panel at (32, 16), magnified twice, every 2x2 block its own colour.
fn panel(revision: u64) -> Overlay {
    let (width, height) = (32, 16);
    let mut rgba = vec![255; width * height * 4];
    for (index, pixel) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let v = (index as u32 ^ revision as u32).wrapping_mul(2_246_822_519) >> 7;
        pixel[..3].copy_from_slice(&[v as u8, (v >> 8) as u8, (v >> 16) as u8]);
    }
    Overlay {
        revision,
        origin: [32, 16],
        size: [64, 32],
        scale: 2,
        rgba: rgba.into(),
    }
}
/// A stored sample as the code the reference gives: P010 keeps codes in the
/// upper ten bits, PyroWave's ten-bit planes over their full range.
fn code(sample: u16, kind: Kind, ten_bit: bool) -> u16 {
    match (ten_bit, kind.pyro()) {
        (false, _) => sample,
        (true, false) => sample >> 6,
        (true, true) => (f64::from(sample) * 1023. / 65535.).round() as u16,
    }
}

#[test]
#[ignore = "requires D3D11 video UAVs and D3D12 compute"]
fn overlay_matches_the_reference_inside_and_closed_outside() -> Result<()> {
    let mut rig = Rig::new()?;
    let overlay = panel(1);
    for (kind, colour, route) in cases() {
        let label = format!("{kind:?} {colour:?} {route:?}");
        let config = config(kind, colour);
        let image = if config.hdr { &rig.hdr } else { &rig.sdr };
        let values = constants(
            &config,
            (image.width, image.height, image.pixel),
            [100., 1.],
            None,
        );
        let expected = reference::expected(
            &overlay,
            reference::Colour::from_values(&values),
            kind.layout(),
            [WIDTH, HEIGHT],
        );
        let closed = rig.convert(kind, colour, route, None)?;
        let open = rig.convert(kind, colour, route, Some(&overlay))?;
        assert_eq!(expected.len(), open.len(), "{label}");
        let mut inside = 0;
        for (plane, ((expected, open), closed)) in
            expected.iter().zip(&open).zip(&closed).enumerate()
        {
            assert_eq!(expected.len(), open.len(), "{label} plane {plane}");
            for (index, (expected, (&open, &closed))) in
                expected.iter().zip(open.iter().zip(closed)).enumerate()
            {
                match expected {
                    None => {
                        assert_eq!(open, closed, "{label} plane {plane} sample {index} outside")
                    }
                    Some(expected) => {
                        let actual = code(open, kind, config.ten_bit());
                        assert!(
                            actual.abs_diff(*expected) <= 1,
                            "{label} plane {plane} sample {index}: {actual}, expected {expected}"
                        );
                        inside += 1;
                    }
                }
            }
        }
        assert!(inside > 0, "{label} has no panel samples");
    }
    Ok(())
}

#[test]
#[ignore = "requires D3D11 video UAVs and D3D12 compute"]
fn overlay_uploads_once_per_revision_and_again_for_a_new_converter() -> Result<()> {
    let rig = Rig::new()?;
    let config = config(Kind::Yuv420, Colour::Sdr709);
    let image = &rig.sdr;
    let source = (image.width, image.height, image.pixel);
    let opened = rig.compute.open(&image.texture)?;
    let compute = || -> Result<crate::compute::Converter> {
        let mut converter =
            crate::compute::Converter::new(rig.compute.clone(), WIDTH, HEIGHT, false)?;
        converter.values = constants(&config, source, [100., 1.], None);
        Ok(converter)
    };
    let run = |converter: &mut crate::compute::Converter| -> Result<()> {
        converter
            .convert(&opened, crate::compute::format(image.pixel), None, image)?
            .wait()
    };
    let mut converter = compute()?;
    run(&mut converter)?;
    assert_eq!(
        converter.menu_counts(),
        (0, 0),
        "closed records no menu pass"
    );
    converter.set_overlay(Some(&panel(1)));
    for _ in 0..3 {
        run(&mut converter)?;
    }
    assert_eq!(converter.menu_counts(), (1, 3));
    // More revisions than the ring holds, each read before the next.
    for revision in 2..7 {
        converter.set_overlay(Some(&panel(revision)));
        run(&mut converter)?;
    }
    assert_eq!(converter.menu_counts(), (6, 8));
    converter.set_overlay(None);
    run(&mut converter)?;
    assert_eq!(converter.menu_counts(), (6, 8), "closed again, no pass");
    let mut rebuilt = compute()?;
    rebuilt.set_overlay(Some(&panel(6)));
    run(&mut rebuilt)?;
    assert_eq!(
        rebuilt.menu_counts(),
        (1, 1),
        "a rebuilt converter uploads again"
    );

    let mut graphics = Converter::new(&rig.gpu, &config, source)?;
    graphics.compute = None;
    graphics.convert(image)?;
    assert_eq!(graphics.menu_counts(), (0, 0));
    graphics.set_overlay(Some(&panel(1)));
    graphics.convert(image)?;
    graphics.convert(image)?;
    assert_eq!(graphics.menu_counts(), (1, 2));
    graphics.set_overlay(Some(&panel(2)));
    graphics.convert(image)?;
    assert_eq!(graphics.menu_counts(), (2, 3));

    let pyro = self::config(Kind::Pyro420, Colour::Sdr709);
    let mut planar = PlanarConverter::new(&rig.gpu, &pyro, source)?;
    planar.compute = None;
    planar.convert(image, [100., 1.])?;
    assert_eq!(planar.menu_counts(), (0, 0), "closed draws no menu pass");
    planar.set_overlay(Some(&panel(1)));
    planar.convert(image, [100., 1.])?;
    planar.convert(image, [100., 1.])?;
    assert_eq!(planar.menu_counts(), (1, 2));
    // A panel that does not fit the stream fails loudly.
    graphics.set_overlay(Some(&Overlay {
        origin: [96, 16],
        ..panel(3)
    }));
    assert!(graphics.convert(image).is_err());
    converter.set_overlay(Some(&Overlay {
        origin: [96, 16],
        ..panel(3)
    }));
    assert!(run(&mut converter).is_err());
    Ok(())
}
