//! Encoder focus on the compute converter: the still-age detector and the ROI
//! map against `butterpollo_core::encoder_focus`, which A2's encoder reads.
use super::*;
use crate::capture::{ComGuard, Device, GpuImage, Image, Pixel};
use butterpollo_core::encoder_focus::{Levels, Mode, age_grid, centre_weight, ctb_age, importance};

struct Rig {
    _com: ComGuard,
    gpu: Device,
    compute: Arc<Compute>,
}
impl Rig {
    fn new() -> Result<Self> {
        let _com = ComGuard::new()?;
        let gpu = Device::new("")?;
        let compute = Compute::for_device(&gpu.device)?;
        Ok(Self { _com, gpu, compute })
    }
    fn converter(&self, compute: &Arc<Compute>, image: &Image) -> Result<Converter> {
        let hdr = image.pixel == Pixel::RgbaF16;
        let mut converter = Converter::new(compute.clone(), image.width, image.height, hdr)?;
        converter.values = crate::gpu_color::constants(
            &butterpollo_core::rtsp::Negotiated {
                width: image.width,
                height: image.height,
                codec: 1,
                hdr,
                ..Default::default()
            },
            (image.width, image.height, image.pixel),
            [100., 1.],
            None,
        );
        Ok(converter)
    }
    /// Convert `image` and wait for it.
    fn convert(&self, converter: &mut Converter, image: &Image) -> Result<ID3D12Resource> {
        let source = GpuImage::upload(&self.gpu, image)?;
        let opened = converter.compute.open(&source.texture)?;
        let converted = converter.convert(&opened, format(image.pixel), None, &source)?;
        converted.wait()?;
        Ok(converted.texture.as_ref().clone())
    }
}
/// A BGRA image whose pixels depend on their position and `seed`.
fn bgra(width: u32, height: u32, seed: u32) -> Image {
    let mut bytes = vec![0; (width * height * 4) as usize];
    for (index, pixel) in bytes.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let v = (index as u32)
            .wrapping_mul(2_654_435_761)
            .wrapping_add(seed)
            >> 8;
        *pixel = [v as u8, (v >> 8) as u8, (v >> 16) as u8, 255];
    }
    Image {
        width,
        height,
        stride: width as usize * 4,
        bytes,
        captured: std::time::Instant::now(),
        pixel: Pixel::Bgra8,
    }
}
/// Repaint 16x16 block `(bx, by)`'s interior. Its first and last columns and
/// rows stay, because chroma also reads the pixel left of each pair.
fn repaint(image: &mut Image, (bx, by): (u32, u32), value: u8) {
    for y in by * 16 + 1..by * 16 + 15 {
        for x in bx * 16 + 1..bx * 16 + 15 {
            let at = y as usize * image.stride + x as usize * 4;
            image.bytes[at..at + 3].copy_from_slice(&[value, value.wrapping_mul(3), !value]);
        }
    }
}
/// `bytes` of a buffer on `compute`'s device, copied out and waited for.
fn read_buffer(compute: &Compute, buffer: &ID3D12Resource, bytes: u64) -> Result<Vec<u32>> {
    let readback = focus_buffer(
        &compute.device,
        D3D12_HEAP_TYPE_READBACK,
        bytes,
        D3D12_RESOURCE_STATE_COPY_DEST,
    )?;
    // SAFETY: Both buffers are live and at least `bytes` long; the copy is waited for before
    // Map, and the slice stays inside the mapped buffer until Unmap.
    unsafe {
        let allocator: ID3D12CommandAllocator = compute
            .device
            .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COMPUTE)?;
        let list: ID3D12GraphicsCommandList = compute.device.CreateCommandList(
            0,
            D3D12_COMMAND_LIST_TYPE_COMPUTE,
            &allocator,
            None,
        )?;
        list.CopyBufferRegion(&readback, 0, buffer, 0, bytes);
        list.Close()?;
        compute.wait(compute.execute(&list)?)?;
        let mut mapped = std::ptr::null_mut();
        readback.Map(0, None, Some(&mut mapped))?;
        let words = std::slice::from_raw_parts(mapped.cast::<u32>(), bytes as usize / 4).to_vec();
        readback.Unmap(0, None);
        Ok(words)
    }
}
/// The converted texture's bytes, both planes, row by row.
fn read_texture(compute: &Compute, texture: &ID3D12Resource) -> Result<Vec<u8>> {
    // SAFETY: `buffer` is sized by GetCopyableFootprints for both planes, the copy is waited for
    // before Map, and each row read stays within its footprint until Unmap below.
    unsafe {
        let desc = texture.GetDesc();
        let pixel_bytes = if desc.Format == DXGI_FORMAT_NV12 {
            1
        } else {
            2
        };
        let mut layouts = [D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); 2];
        let mut total = 0u64;
        compute.device.GetCopyableFootprints(
            &desc,
            0,
            2,
            0,
            Some(layouts.as_mut_ptr()),
            None,
            None,
            Some(&mut total),
        );
        let buffer = focus_buffer(
            &compute.device,
            D3D12_HEAP_TYPE_READBACK,
            total,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;
        let allocator: ID3D12CommandAllocator = compute
            .device
            .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COMPUTE)?;
        let list: ID3D12GraphicsCommandList = compute.device.CreateCommandList(
            0,
            D3D12_COMMAND_LIST_TYPE_COMPUTE,
            &allocator,
            None,
        )?;
        for (plane, layout) in layouts.iter().enumerate() {
            let destination = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::ManuallyDrop::new(Some(buffer.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: *layout,
                },
            };
            let source = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::ManuallyDrop::new(Some(texture.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    SubresourceIndex: plane as u32,
                },
            };
            list.CopyTextureRegion(&destination, 0, 0, 0, &source, None);
            drop(std::mem::ManuallyDrop::into_inner(destination.pResource));
            drop(std::mem::ManuallyDrop::into_inner(source.pResource));
        }
        list.Close()?;
        compute.wait(compute.execute(&list)?)?;
        let mut mapped = std::ptr::null_mut();
        buffer.Map(0, None, Some(&mut mapped))?;
        let mut bytes = Vec::new();
        for (plane, layout) in layouts.iter().enumerate() {
            let rows = desc.Height as usize >> plane;
            for y in 0..rows {
                bytes.extend_from_slice(std::slice::from_raw_parts(
                    mapped
                        .cast::<u8>()
                        .add(layout.Offset as usize + y * layout.Footprint.RowPitch as usize),
                    desc.Width as usize * pixel_bytes,
                ));
            }
        }
        buffer.Unmap(0, None);
        Ok(bytes)
    }
}
fn ages(compute: &Compute, converter: &Converter) -> Result<Vec<u32>> {
    let focus = converter.focus.as_deref().context("focus not enabled")?;
    let (width, height) = focus.age_grid;
    let words = read_buffer(compute, &focus.ages, 8 * u64::from(width * height))?;
    Ok(words.chunks(2).map(|pair| pair[1]).collect())
}
fn settings(mode: Mode, levels: Levels, cursor: Option<(i32, i32)>) -> FocusSettings {
    FocusSettings {
        mode,
        levels,
        cursor,
    }
}

#[test]
#[ignore = "requires D3D12 compute"]
fn focus_leaves_the_converted_picture_unchanged() -> Result<()> {
    let rig = Rig::new()?;
    let sdr = bgra(96, 48, 7);
    let mut hdr_bytes = vec![0; 96 * 48 * 8];
    for (index, pixel) in hdr_bytes.as_chunks_mut::<8>().0.iter_mut().enumerate() {
        for channel in 0..3 {
            let value = ((index * 7 + channel * 13) % 97) as f32 / 8.;
            pixel[channel * 2..channel * 2 + 2]
                .copy_from_slice(&half::f16::from_f32(value).to_bits().to_le_bytes());
        }
        pixel[6..].copy_from_slice(&half::f16::ONE.to_bits().to_le_bytes());
    }
    let hdr = Image {
        width: 96,
        height: 48,
        stride: 96 * 8,
        bytes: hdr_bytes,
        captured: std::time::Instant::now(),
        pixel: Pixel::RgbaF16,
    };
    for image in [&sdr, &hdr] {
        let mut plain = rig.converter(&rig.compute, image)?;
        let expected = read_texture(&rig.compute, &rig.convert(&mut plain, image)?)?;
        let mut focused = rig.converter(&rig.compute, image)?;
        focused.enable_focus(16)?;
        focused.set_focus(Some(settings(
            Mode::Centre,
            Levels::default(),
            Some((5, 5)),
        )))?;
        for _ in 0..2 {
            let actual = read_texture(&rig.compute, &rig.convert(&mut focused, image)?)?;
            assert!(actual == expected, "{:?} picture changed", image.pixel);
        }
        assert!(focused.latest_map(0).is_some());
    }
    Ok(())
}

#[test]
#[ignore = "requires D3D12 compute"]
fn still_ages_count_unchanged_frames_and_restart_on_a_change() -> Result<()> {
    let rig = Rig::new()?;
    let mut image = bgra(64, 32, 3);
    let mut converter = rig.converter(&rig.compute, &image)?;
    converter.enable_focus(16)?;
    converter.set_focus(Some(settings(Mode::Static, Levels::default(), None)))?;
    assert_eq!(age_grid(64, 32), (4, 2));
    for frame in 0..5 {
        rig.convert(&mut converter, &image)?;
        assert_eq!(ages(&rig.compute, &converter)?, vec![frame; 8]);
    }
    repaint(&mut image, (1, 1), 200);
    rig.convert(&mut converter, &image)?;
    assert_eq!(
        ages(&rig.compute, &converter)?,
        vec![5, 5, 5, 5, 5, 0, 5, 5]
    );
    // Off drops the old maps; on again starts over, even for an unchanged picture.
    assert!(converter.latest_map(0).is_some());
    converter.set_focus(None)?;
    assert!(converter.latest_map(0).is_none());
    rig.convert(&mut converter, &image)?;
    assert!(converter.latest_map(0).is_none());
    converter.set_focus(Some(settings(Mode::Static, Levels::default(), None)))?;
    assert!(converter.latest_map(0).is_none());
    rig.convert(&mut converter, &image)?;
    assert!(converter.latest_map(0).is_some());
    assert_eq!(ages(&rig.compute, &converter)?, vec![0; 8]);
    rig.convert(&mut converter, &image)?;
    assert_eq!(ages(&rig.compute, &converter)?, vec![1; 8]);
    Ok(())
}

#[test]
#[ignore = "requires D3D12 compute"]
fn maps_match_the_core_reference() -> Result<()> {
    let rig = Rig::new()?;
    let (width, height) = (256, 128);
    let (age_width, age_height) = age_grid(width, height);
    let tuned = Levels {
        moving: 1,
        still: 9,
        ramp: 3,
        centre: 5,
        ctb_still: 0.5,
        noise_bits: 0,
    };
    let variants = [
        settings(Mode::Static, Levels::default(), None),
        settings(Mode::Centre, Levels::default(), None),
        settings(Mode::Centre, tuned, Some((240, 16))),
        settings(Mode::Static, tuned, None),
        settings(
            Mode::Centre,
            Levels {
                ctb_still: 0.,
                ..tuned
            },
            Some((-5000, i32::MAX)),
        ),
    ];
    for block in [16, 64] {
        let mut image = bgra(width, height, 11);
        let mut converter = rig.converter(&rig.compute, &image)?;
        converter.enable_focus(block)?;
        let mut expected = vec![0u32; (age_width * age_height) as usize];
        let mut newest = 0;
        for frame in 0..12u32 {
            if frame > 0 {
                for (index, age) in expected.iter_mut().enumerate() {
                    let index = index as u32;
                    // Block i changes every (i % 6) frames; 0 never changes.
                    let period = index % 6;
                    if period != 0 && frame % period == 0 {
                        repaint(
                            &mut image,
                            (index % age_width, index / age_width),
                            frame as u8,
                        );
                        *age = 0;
                    } else {
                        *age = (*age + 1).min(255);
                    }
                }
            }
            let chosen = variants[frame as usize % variants.len()];
            converter.set_focus(Some(chosen))?;
            rig.convert(&mut converter, &image)?;
            assert_eq!(ages(&rig.compute, &converter)?, expected, "frame {frame}");
            let map = converter
                .latest_map(newest)
                .context("no map after a conversion")?;
            assert!(map.value > newest);
            newest = map.value;
            let grid = (width.div_ceil(block), height.div_ceil(block));
            assert_eq!((map.width, map.height), grid);
            let mut reference = Vec::new();
            for y in 0..grid.1 {
                for x in 0..grid.0 {
                    let age = if block == 16 {
                        expected[(y * age_width + x) as usize]
                    } else {
                        let mut ages = Vec::new();
                        for ay in y * 4..(y * 4 + 4).min(age_height) {
                            for ax in x * 4..(x * 4 + 4).min(age_width) {
                                ages.push(expected[(ay * age_width + ax) as usize]);
                            }
                        }
                        ctb_age(&ages, chosen.levels.ctb_still_256())
                    };
                    let weight = centre_weight(x, y, block, width, height, chosen.cursor);
                    reference.push(importance(age, weight, chosen.mode, &chosen.levels));
                }
            }
            assert_eq!(
                map.rows,
                &reference[..],
                "block {block} frame {frame} {chosen:?}"
            );
            let uniform = reference.iter().all(|&value| value == reference[0]);
            assert_eq!(map.uniform, uniform, "block {block} frame {frame}");
            if frame == 0 && chosen.mode == Mode::Static {
                assert!(map.uniform, "a first static map has no still blocks");
            }
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires native D3D12; gates only a task-owned compute queue"]
fn latest_map_never_waits_for_an_unfinished_conversion() -> Result<()> {
    struct Gate(ID3D12Fence);
    impl Drop for Gate {
        fn drop(&mut self) {
            // SAFETY: Signalling a live fence from the CPU releases the queue waiting on it.
            let _ = unsafe { self.0.Signal(1) };
        }
    }
    let rig = Rig::new()?;
    // SAFETY: A read-only query on this test's live device.
    let luid = unsafe { rig.compute.device.GetAdapterLuid() };
    let compute = Compute::new(luid)?;
    let image = bgra(64, 32, 5);
    // Move the queue's fence past the fresh target's, so the two timelines differ.
    let mut other = rig.converter(&compute, &image)?;
    for _ in 0..3 {
        rig.convert(&mut other, &image)?;
    }
    let mut converter = rig.converter(&compute, &image)?;
    converter.enable_focus(16)?;
    converter.set_focus(Some(settings(Mode::Static, Levels::default(), None)))?;
    // SAFETY: The gate and queue belong to this test, and Gate signals on drop.
    let gate = Gate(unsafe { compute.device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? });
    // SAFETY: The gate is on the queue's device.
    unsafe { compute.queue.Wait(&gate.0, 1)? };
    let source = GpuImage::upload(&rig.gpu, &image)?;
    let converted = converter.convert(
        &compute.open(&source.texture)?,
        format(image.pixel),
        None,
        &source,
    )?;
    let asked = std::time::Instant::now();
    assert!(converter.latest_map(0).is_none());
    assert!(asked.elapsed() < std::time::Duration::from_millis(50));
    drop(gate);
    converted.wait()?;
    let map = converter
        .latest_map(0)
        .context("finished conversion has no map")?;
    assert_ne!(converted.conversion, converted.value);
    assert_eq!(map.value, converted.conversion);
    assert!(converter.latest_map(converted.conversion).is_none());
    Ok(())
}

#[test]
#[ignore = "requires D3D12 compute"]
fn focus_refuses_what_it_cannot_follow() -> Result<()> {
    let rig = Rig::new()?;
    let image = bgra(64, 32, 1);
    let mut converter = rig.converter(&rig.compute, &image)?;
    assert!(
        converter
            .set_focus(Some(settings(Mode::Static, Levels::default(), None)))
            .is_err()
    );
    assert!(converter.enable_focus(32).is_err());
    converter.enable_focus(16)?;
    assert!(converter.enable_focus(64).is_err());
    assert!(converter.pack_chroma().is_err());
    let mut packed = rig.converter(&rig.compute, &image)?;
    packed.pack_chroma()?;
    assert!(packed.enable_focus(16).is_err());
    Ok(())
}
