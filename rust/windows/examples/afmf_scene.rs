//! A full-display D3D11 test app at a capped frame rate, for checking whether
//! AMD Fluid Motion Frames (frame generation) reaches a capture (B7). Not packaged.
//!
//! The scene scrolls a busy pattern, so the driver has motion to extrapolate, and
//! carries a marker band: a bar that jumps `STEP` pixels per rendered frame and a
//! 16-bit frame number. A captured picture whose bar is not at a multiple of `STEP`
//! (or whose number bits are blended) was not rendered by this app.
//!
//! usage: afmf_scene DISPLAY SECONDS FPS [REPORT.json] [--exclusive]
//!   The scene is a borderless top-most popup over the display, so Windows can flip it
//!   directly to the screen. Changes no display mode, HDR or layout setting. (Exclusive
//!   fullscreen was tried on the 780M panel and DXGI refused it with 0x887A0001; observed, output not saved.)
//!   `--exclusive` asks DXGI for exclusive fullscreen on the display's output at its current
//!   mode, as a fullscreen-exclusive game would; AFMF's driver path treats such a game as its
//!   main case. The report says whether DXGI granted it and whether it still held at the end.
#[cfg(not(windows))]
fn main() {
    eprintln!("This probe requires Windows.");
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    scene::run()
}

#[cfg(windows)]
mod scene {
    use anyhow::{Context, Result, bail};
    use butterpollo_windows::capture::{ComGuard, displays, enable_dpi_awareness};
    use std::time::{Duration, Instant};
    use windows::{
        Win32::{
            Foundation::*,
            Graphics::{
                Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, Fxc::*},
                Direct3D11::*,
                Dxgi::{Common::*, *},
            },
            System::{LibraryLoader::GetModuleHandleW, Performance::*, Threading::*},
            UI::WindowsAndMessaging::*,
        },
        core::{BOOL, Interface, PCSTR, w},
    };

    /// Pixels the marker bar moves per rendered frame.
    const STEP: u32 = 48;
    const SHADER: &str = r#"
cbuffer Frame : register(b0) { uint frame; uint width; uint step; uint pad; };
float4 vertex(uint id : SV_VertexID) : SV_Position {
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * float2(2, -2) + float2(-1, 1), 0, 1);
}
float4 picture(float4 p : SV_Position) : SV_Target {
    if (p.y < 24) {
        float bar = float((frame * step) % (width - 64));
        return (p.x >= bar && p.x < bar + 64) ? float4(1, 1, 1, 1) : float4(0, 0, 0.3, 1);
    }
    if (p.y < 48) {
        if (p.x >= 16 && p.x < 16 + 16 * 24) {
            float v = ((frame >> (uint(p.x - 16) / 24)) & 1);
            return float4(v, v, v, 1);
        }
        return float4(0, 0, 0, 1);
    }
    float2 at = p.xy + float2(frame * 7, frame * 3);
    float grid = fmod(floor(at.x / 40) + floor(at.y / 40), 2);
    float3 color = lerp(float3(0.05, 0.1, 0.3), float3(0.8, 0.45, 0.1), grid);
    float ring = sin(length(at - float2(900, 700)) * 0.05) * 0.5 + 0.5;
    color = lerp(color, float3(ring, 1 - ring, 0.5), 0.35);
    color += sin(at.x * 0.013) * sin(at.y * 0.017) * 0.15;
    return float4(color, 1);
}
"#;

    /// Puts `window` in front: an unassigned key tap gives this process the last input, which
    /// the foreground lock asks for (`lossless::to_front_quietly`); failing that, the input of
    /// the window in front is attached for the call. True when it is in front.
    fn bring_to_front(window: HWND) -> bool {
        if butterpollo_windows::lossless::to_front_quietly(window) {
            return true;
        }
        // SAFETY: these calls take handles and thread ids by value; a stale one just fails,
        // and the attachment is undone before returning.
        unsafe {
            let front = GetForegroundWindow();
            let theirs = GetWindowThreadProcessId(front, None);
            let ours = GetCurrentThreadId();
            let attached =
                theirs != 0 && theirs != ours && AttachThreadInput(ours, theirs, true).as_bool();
            let _ = BringWindowToTop(window);
            let _ = SetForegroundWindow(window);
            if attached {
                let _ = AttachThreadInput(ours, theirs, false);
            }
            GetForegroundWindow() == window
        }
    }

    /// Why DXGI refused exclusive fullscreen, in words for the run's report.
    fn fullscreen_refusal(code: windows::core::HRESULT, front: bool) -> &'static str {
        match (code, front) {
            (_, false) => {
                "the window is not in front (the foreground lock, or another window took it)"
            }
            (DXGI_ERROR_NOT_CURRENTLY_AVAILABLE, true) => {
                "the output is not available now (another exclusive app, a locked or non-interactive desktop)"
            }
            (DXGI_ERROR_INVALID_CALL, true) => {
                "DXGI_ERROR_INVALID_CALL in front: the swap chain or window does not allow it"
            }
            _ => "unexpected",
        }
    }

    struct Window(HWND);
    impl Drop for Window {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }
    /// Leaves exclusive fullscreen before the swap chain is released, as DXGI requires.
    struct Windowed(bool, IDXGISwapChain3);
    impl Drop for Windowed {
        fn drop(&mut self) {
            if self.0 {
                unsafe {
                    let _ = self.1.SetFullscreenState(false, None::<&IDXGIOutput>);
                }
            }
        }
    }
    struct Wait(HANDLE);
    impl Drop for Wait {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    unsafe extern "system" fn window(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    unsafe fn compile(entry: &'static [u8], target: &'static [u8]) -> Result<Vec<u8>> {
        unsafe {
            let mut code = None;
            let mut errors = None;
            let result = D3DCompile(
                SHADER.as_ptr().cast(),
                SHADER.len(),
                PCSTR::null(),
                None,
                None,
                PCSTR(entry.as_ptr()),
                PCSTR(target.as_ptr()),
                D3DCOMPILE_OPTIMIZATION_LEVEL3,
                0,
                &mut code,
                Some(&mut errors),
            );
            if let Err(error) = result {
                let details = errors
                    .map(|blob| {
                        String::from_utf8_lossy(std::slice::from_raw_parts(
                            blob.GetBufferPointer().cast(),
                            blob.GetBufferSize(),
                        ))
                        .into_owned()
                    })
                    .unwrap_or_default();
                bail!("scene shader: {error}: {details}");
            }
            let code = code.context("shader compiler returned no bytecode")?;
            Ok(
                std::slice::from_raw_parts(code.GetBufferPointer().cast(), code.GetBufferSize())
                    .to_vec(),
            )
        }
    }

    pub fn run() -> Result<()> {
        let usage = "usage: afmf_scene DISPLAY SECONDS FPS [REPORT.json] [--exclusive]";
        let all: Vec<String> = std::env::args().skip(1).collect();
        let exclusive = all.iter().any(|arg| arg == "--exclusive");
        let mut args = all.into_iter().filter(|arg| arg != "--exclusive");
        let display_name = args.next().context(usage)?;
        let seconds: u64 = args.next().context(usage)?.parse()?;
        let fps: u32 = args.next().context(usage)?.parse()?;
        let report = args.next();
        if !(1..=3600).contains(&seconds) || !(1..=1000).contains(&fps) || args.next().is_some() {
            bail!("seconds must be 1..3600 and fps 1..1000");
        }
        enable_dpi_awareness();
        let _com = ComGuard::new()?;
        let display = displays()?
            .into_iter()
            .find(|d| d.display_name.eq_ignore_ascii_case(&display_name))
            .context("explicitly selected display is not active")?;
        if display.width < 640 || display.height < 128 {
            bail!("the scene needs a display of at least 640 by 128");
        }
        let source_mode = butterpollo_windows::display::mode(&display.display_name)?;
        let timer = butterpollo_windows::timing::Timer::new()?;
        let interval = Duration::from_secs_f64(1.0 / f64::from(fps));
        unsafe {
            let module = GetModuleHandleW(None)?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(window),
                hInstance: HINSTANCE(module.0),
                lpszClassName: w!("ButterpolloAfmfScene"),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                bail!("cannot register test window");
            }
            let window = Window(CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("ButterpolloAfmfScene"),
                w!("Rubylight AFMF scene"),
                WS_POPUP | WS_VISIBLE,
                display.x,
                display.y,
                display.width as i32,
                display.height as i32,
                None,
                None,
                Some(HINSTANCE(module.0)),
                None,
            )?);
            SetWindowPos(
                window.0,
                Some(HWND_TOPMOST),
                display.x,
                display.y,
                display.width as i32,
                display.height as i32,
                SWP_SHOWWINDOW,
            )?;
            // Exclusive fullscreen needs the window in front, and Windows' foreground lock keeps
            // a process launched in the background (a scheduled task, a remote shell) from
            // taking it with SetForegroundWindow alone.
            let front = bring_to_front(window.0);
            println!("AFMF_SCENE stage=foreground front={}", u8::from(front));
            let factory: IDXGIFactory2 = CreateDXGIFactory1()?;
            let adapter = factory.EnumAdapters1(display.adapter_index)?;
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device = device.context("no D3D11 device")?;
            let context = context.context("no D3D11 context")?;
            let swap = factory.CreateSwapChainForHwnd(
                &device,
                window.0,
                &DXGI_SWAP_CHAIN_DESC1 {
                    Width: display.width,
                    Height: display.height,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                    BufferCount: 3,
                    SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                    Scaling: DXGI_SCALING_STRETCH,
                    AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                    Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
                    ..Default::default()
                },
                None,
                None,
            )?;
            let swap: IDXGISwapChain3 = swap.cast()?;
            let output = adapter.EnumOutputs(display.output_index)?;
            // Exclusive fullscreen on this display's output, at its current mode. A refusal is
            // reported and the run goes on borderless, so the report says which case it measured.
            let granted = if exclusive {
                match swap.SetFullscreenState(true, &output) {
                    Ok(()) => {
                        swap.ResizeBuffers(
                            0,
                            0,
                            0,
                            DXGI_FORMAT_UNKNOWN,
                            DXGI_SWAP_CHAIN_FLAG(
                                DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0,
                            ),
                        )?;
                        println!("AFMF_SCENE stage=exclusive granted=1");
                        Some(true)
                    }
                    Err(error) => {
                        println!(
                            "AFMF_SCENE stage=exclusive granted=0 error={:#010x} front={} why=\"{}\"",
                            error.code().0,
                            u8::from(GetForegroundWindow() == window.0),
                            fullscreen_refusal(error.code(), GetForegroundWindow() == window.0)
                        );
                        Some(false)
                    }
                }
            } else {
                None
            };
            let _windowed = Windowed(granted == Some(true), swap.clone());
            swap.SetMaximumFrameLatency(1)?;
            let wait = Wait(swap.GetFrameLatencyWaitableObject());
            if wait.0.is_invalid() {
                bail!("swap chain has no presentation event");
            }
            let texture: ID3D11Texture2D = swap.GetBuffer(0)?;
            let mut target = None;
            device.CreateRenderTargetView(&texture, None, Some(&mut target))?;
            let mut vertex = None;
            let mut picture = None;
            device.CreateVertexShader(
                &compile(b"vertex\0", b"vs_5_0\0")?,
                None,
                Some(&mut vertex),
            )?;
            device.CreatePixelShader(
                &compile(b"picture\0", b"ps_5_0\0")?,
                None,
                Some(&mut picture),
            )?;
            let mut constants = None;
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: 16,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut constants),
            )?;
            let constants = constants.unwrap();
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(vertex.as_ref(), None);
            context.PSSetShader(picture.as_ref(), None);
            context.PSSetConstantBuffers(0, Some(&[Some(constants.clone())]));
            context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                Width: display.width as f32,
                Height: display.height as f32,
                MinDepth: 0.,
                MaxDepth: 1.,
                ..Default::default()
            }]));
            // 0 is sRGB (SDR); 12 is HDR10 (PQ), the mode in which a panel dims SDR content.
            let color_space = output.cast::<IDXGIOutput6>()?.GetDesc1()?.ColorSpace.0;
            let mut frequency = 0;
            QueryPerformanceFrequency(&mut frequency)?;
            println!(
                "AFMF_SCENE stage=ready display={} {}x{} refresh={} color_space={color_space} fps={fps}",
                display.display_name, display.width, display.height, source_mode.dmDisplayFrequency
            );
            let start = Instant::now();
            let mut next_frame = start;
            let mut frames = Vec::new();
            while start.elapsed() < Duration::from_secs(seconds) {
                if WaitForSingleObject(wait.0, 1000) != WAIT_OBJECT_0 {
                    bail!("presentation event timed out");
                }
                timer.until(next_frame);
                // A stall costs one frame, not a burst of catch-up pictures.
                next_frame = (next_frame + interval).max(Instant::now());
                let mut message = MSG::default();
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    if message.message == WM_QUIT {
                        bail!("test window closed");
                    }
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                let mut ticks = 0;
                QueryPerformanceCounter(&mut ticks)?;
                let frame = frames.len() as u32 + 1;
                let values = [frame, display.width, STEP, 0];
                context.UpdateSubresource(&constants, 0, None, values.as_ptr().cast(), 0, 0);
                context.OMSetRenderTargets(Some(&[target.clone()]), None);
                context.Draw(3, 0);
                swap.Present(1, DXGI_PRESENT(0)).ok()?;
                frames.push(serde_json::json!({"frame":frame,"qpc":ticks}));
            }
            let elapsed = start.elapsed().as_secs_f64();
            let mut held = BOOL(0);
            let held = granted == Some(true)
                && swap.GetFullscreenState(Some(&mut held), None).is_ok()
                && held.as_bool();
            let data = serde_json::json!({
                "scope":"render times of a full-display test scene; says nothing about what a capture or the screen showed",
                "display":display,"fps_cap":fps,"step":STEP,
                "display_refresh_hz":source_mode.dmDisplayFrequency,"output_color_space":color_space,"seconds":elapsed,
                "exclusive_requested":exclusive,"exclusive_granted":granted,"exclusive_held_at_end":held,
                "qpc_frequency":frequency,"presented_frames":frames.len(),"frames":frames
            });
            if let Some(path) = report {
                std::fs::write(path, serde_json::to_vec_pretty(&data)?)?;
            }
            println!(
                "AFMF_SCENE frames={} seconds={elapsed:.3} rendered_fps={:.2}",
                frames.len(),
                frames.len() as f64 / elapsed
            );
        }
        Ok(())
    }
}
