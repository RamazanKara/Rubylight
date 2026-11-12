//! Frame Warp's passes: their sizes and constants, and on a GPU the
//! thumbnail against the same samples taken on the CPU and the HUD mask of
//! frames whose left part stays while the rest changes.
use super::*;
use crate::capture::{ComGuard, Device, Image};

#[test]
fn the_grid_keeps_the_picture_shape_and_no_more_cells_than_pixels() {
    assert_eq!(hud_grid(0, (1920, 1080)), None);
    assert_eq!(hud_grid(64, (0, 1080)), None);
    assert_eq!(hud_grid(480, (1920, 1080)), Some((480, 270)));
    assert_eq!(hud_grid(960, (1920, 1080)), Some((960, 540)));
    // A 21:9 picture rounds to the nearest row.
    assert_eq!(hud_grid(512, (3440, 1440)), Some((512, 214)));
    // A capture smaller than the mask: a cell per pixel at most.
    assert_eq!(hud_grid(1024, (640, 360)), Some((640, 360)));
    assert_eq!(hud_grid(4096, (7680, 4320)), Some((2048, 1152)));
    // A very wide picture keeps at least one row.
    assert_eq!(hud_grid(8, (4000, 10)), Some((8, 1)));
}

#[test]
fn the_hud_words_follow_one_another() {
    let layout = layout((100, 50), (40, 20)).unwrap();
    assert_eq!(
        layout,
        Layout {
            codes: 0,
            counts: 5000,
            scores: 5800,
            bits: 6600,
            stride: 2,
            words: 6640,
        }
    );
    assert!(super::layout((65536, 65536), (2048, 2048)).is_err());
}

#[test]
fn constants_match_the_shader_cbuffer() {
    let pass = Pass {
        thumbnail: (256, 144),
        hud: Some(Hud {
            grid: (40, 20),
            read: true,
        }),
    };
    let layout = layout((100, 50), (40, 20)).unwrap();
    assert_eq!(
        constants((100, 50), Pixel::Rgba10Pq, &pass, SCORE, Some(&layout)),
        [
            100, 50, 256, 144, 40, 20, 2, 1, 0, 0, 5000, 5800, 6600, 2, HUD_SCORE, 0
        ]
    );
    let thumbnail_only = Pass {
        thumbnail: (256, 144),
        hud: None,
    };
    assert_eq!(
        constants((1920, 1080), Pixel::RgbaF16, &thumbnail_only, CLEAR, None),
        [
            1920, 1080, 256, 144, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, HUD_SCORE, 0
        ]
    );
}

#[test]
fn passes_outside_the_buffers_are_refused() {
    let pass = |thumbnail, hud| Pass { thumbnail, hud };
    assert!(pass((256, 144), None).check().is_ok());
    assert!(pass((256, 256), None).check().is_ok());
    assert!(pass((258, 144), None).check().is_err());
    assert!(pass((254, 144), None).check().is_err());
    assert!(pass((256, 257), None).check().is_err());
    assert!(pass((256, 0), None).check().is_err());
    let hud = |grid| Some(Hud { grid, read: false });
    assert!(pass((256, 144), hud((2048, 1152))).check().is_ok());
    assert!(pass((256, 144), hud((2049, 1152))).check().is_err());
    assert!(pass((256, 144), hud((64, 0))).check().is_err());
}

/// A grey BGRA picture, `value(x, y)` in every colour channel.
fn grey(width: u32, height: u32, value: impl Fn(u32, u32) -> u8) -> Image {
    let mut bytes = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let v = value(x, y);
            bytes.extend_from_slice(&[v, v, v, 255]);
        }
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

/// The thumbnail `frame_warp_thumb_cs` makes of a grey picture, on the CPU.
fn thumbnail(image: &Image, (width, height): (u32, u32)) -> Vec<u8> {
    let (sw, sh) = (image.width, image.height);
    let mut luma = Vec::with_capacity((width * height) as usize);
    for ty in 0..height {
        let y0 = ty * sh / height;
        let y1 = ((ty + 1) * sh / height).max(y0 + 1);
        for tx in 0..width {
            let x0 = tx * sw / width;
            let x1 = ((tx + 1) * sw / width).max(x0 + 1);
            let span = (x1 - x0, y1 - y0);
            let n = (span.0.min(8), span.1.min(8));
            let mut sum = 0.0f32;
            for sy in 0..n.1 {
                for sx in 0..n.0 {
                    let x = x0 + (2 * sx + 1) * span.0 / (2 * n.0);
                    let y = y0 + (2 * sy + 1) * span.1 / (2 * n.1);
                    sum +=
                        f32::from(image.bytes[y as usize * image.stride + x as usize * 4]) / 255.0;
                }
            }
            luma.push(((sum / (n.0 * n.1) as f32).clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
        }
    }
    luma
}

/// `image` on the GPU once D3D11 has finished uploading it: the upload has no
/// fence for the compute queue to wait on, and a large one is still copying
/// when the passes would read it.
fn uploaded(gpu: &Device, image: &Image) -> Result<GpuImage> {
    use windows::Win32::Graphics::Direct3D11::{D3D11_QUERY_DESC, D3D11_QUERY_EVENT};
    use windows::core::BOOL;
    let source = GpuImage::upload(gpu, image)?;
    let desc = D3D11_QUERY_DESC {
        Query: D3D11_QUERY_EVENT,
        MiscFlags: 0,
    };
    let mut query = None;
    // SAFETY: `desc` and `query` are live locals; the event query writes one
    // BOOL into the live local `done`, and only once it has completed.
    unsafe {
        gpu.device.CreateQuery(&desc, Some(&mut query))?;
        let query = query.context("an event query")?;
        gpu.context.End(&query);
        gpu.context.Flush();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let mut done = BOOL(0);
            let read = gpu.context.GetData(
                &query,
                Some((&raw mut done).cast()),
                std::mem::size_of::<BOOL>() as u32,
                0,
            );
            if read.is_ok() && done.as_bool() {
                break;
            }
            anyhow::ensure!(
                std::time::Instant::now() < deadline,
                "the upload did not finish"
            );
            std::thread::yield_now();
        }
    }
    Ok(source)
}

struct Rig {
    _com: ComGuard,
    gpu: Device,
    passes: FrameWarpGpu,
}
impl Rig {
    fn new() -> Result<Self> {
        let _com = ComGuard::new()?;
        let gpu = Device::new("")?;
        let passes = FrameWarpGpu::new(Compute::for_device(&gpu.device)?)?;
        Ok(Self { _com, gpu, passes })
    }
    /// Run `pass` over `image`, wait for it and poll.
    fn run(
        &mut self,
        image: &Image,
        pass: &Pass,
        at: u64,
    ) -> Result<(Vec<ThumbnailReadout>, Option<MaskReadout>)> {
        let source = uploaded(&self.gpu, image)?;
        assert!(self.passes.can_read(&source));
        self.passes
            .submit(&source, pass, at)?
            .context("a free slot")?;
        self.passes.compute.wait(self.passes.last)?;
        self.passes.poll()
    }
}

#[test]
#[ignore = "requires D3D12 compute"]
fn the_thumbnail_is_the_mean_of_its_samples() -> Result<()> {
    let mut rig = Rig::new()?;
    for (width, height, size) in [
        (512, 288, (256, 144)),
        (3840, 2160, (256, 144)),
        (200, 100, (256, 128)),
    ] {
        let image = grey(width, height, |x, y| ((x * 7 + y * 13) % 251) as u8);
        let pass = Pass {
            thumbnail: size,
            hud: None,
        };
        let (thumbnails, mask) = rig.run(&image, &pass, u64::from(width))?;
        assert!(mask.is_none());
        let [readout] = thumbnails.as_slice() else {
            panic!("one thumbnail, not {}", thumbnails.len());
        };
        assert_eq!(
            (readout.at, readout.width, readout.height),
            (u64::from(width), size.0 as usize, size.1 as usize)
        );
        let expected = thumbnail(&image, size);
        for (i, (got, want)) in readout.luma.iter().zip(&expected).enumerate() {
            assert!(
                got.abs_diff(*want) <= 1,
                "{width}x{height} pixel {i}: {got} vs {want}"
            );
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires D3D12 compute"]
fn cells_that_stay_while_the_rest_changes_become_hud() -> Result<()> {
    let mut rig = Rig::new()?;
    let (width, height) = (256, 128);
    let grid = hud_grid(64, (width, height)).unwrap();
    assert_eq!(grid, (64, 32));
    // The left quarter stays; the rest changes on every frame.
    let frame = |n: u32| {
        grey(width, height, move |x, y| {
            if x < 64 {
                (x + y) as u8
            } else {
                (x * 3 + y + n * 17) as u8
            }
        })
    };
    let mut mask = None;
    for n in 0..=HUD_SCORE + 2 {
        let last = n == HUD_SCORE + 2;
        let pass = Pass {
            thumbnail: (256, 128),
            hud: Some(Hud { grid, read: last }),
        };
        let (_, read) = rig.run(&frame(n), &pass, u64::from(n))?;
        if last {
            mask = read;
        } else {
            assert!(read.is_none());
        }
    }
    let mask = mask.context("the mask")?;
    assert_eq!((mask.width, mask.height), grid);
    assert_eq!(mask.words.len(), 2 * 32);
    for row in mask.words.chunks(2) {
        // Columns 0-15 (pixels 0-63) are HUD, the rest is not.
        assert_eq!(row, [0x0000_ffff, 0], "{:08x?}", mask.words);
    }
    // A pass while the camera stood still starts the scoring over from the
    // next capture's codes, and scores already earned stay.
    let still = Pass {
        thumbnail: (256, 128),
        hud: None,
    };
    rig.run(&frame(100), &still, 100)?;
    let pass = Pass {
        thumbnail: (256, 128),
        hud: Some(Hud { grid, read: true }),
    };
    let (_, mask) = rig.run(&frame(101), &pass, 101)?;
    let mask = mask.context("the mask after a pause")?;
    assert!(mask.words.chunks(2).all(|row| row == [0x0000_ffff, 0]));
    Ok(())
}

/// Wall time from submitting a capture's passes (thumbnail and HUD scoring,
/// a mask read every 60th) to their completion: an upper bound on their GPU
/// cost, printed for rust/PERFORMANCE.md.
#[test]
#[ignore = "requires D3D12 compute; prints timings"]
fn the_passes_finish_within_a_frame() -> Result<()> {
    let mut rig = Rig::new()?;
    for (width, height) in [(1920, 1080), (2560, 1440), (3840, 2160)] {
        let image = grey(width, height, |x, y| ((x * 7 + y * 13) % 251) as u8);
        let source = uploaded(&rig.gpu, &image)?;
        let grid = hud_grid(512, (width, height)).context("a grid")?;
        let mut times = Vec::new();
        for n in 0..300u64 {
            let pass = Pass {
                thumbnail: (256, 144),
                hud: Some(Hud {
                    grid,
                    read: n % 60 == 59,
                }),
            };
            let start = std::time::Instant::now();
            rig.passes
                .submit(&source, &pass, n)?
                .context("a free slot")?;
            rig.passes.compute.wait(rig.passes.last)?;
            let elapsed = start.elapsed();
            rig.passes.poll()?;
            // The first passes include pipeline and buffer setup.
            if n >= 20 {
                times.push(elapsed.as_secs_f64() * 1000.0);
            }
        }
        times.sort_by(f64::total_cmp);
        let at = |q: f64| times[((times.len() - 1) as f64 * q).round() as usize];
        println!(
            "frame warp passes {width}x{height}: median {:.3} ms, p95 {:.3} ms, max {:.3} ms",
            at(0.5),
            at(0.95),
            at(1.0)
        );
        assert!(at(0.5) < 4.0, "{width}x{height}: median {:.3} ms", at(0.5));
    }
    Ok(())
}
