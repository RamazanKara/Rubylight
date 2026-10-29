//! Test-owned window that switches between a small window and borderless
//! fullscreen, as a game does when it starts. Not packaged.
//!
//! A flip-model swap chain draws a moving picture at the display's refresh.
//! Every cycle the window stays small for WINDOWED_MS, then covers the whole
//! display for FULL_MS (style, position and swap-chain buffers change, the
//! way games switch), then goes back. The report lists each present and each
//! switch with its QPC and UTC time, so a host's capture gaps can be lined up
//! with them. Fullscreen, the window is topmost, over test fixtures such as
//! the motion strip, as a game in front covers the desktop. Never changes
//! the display's mode, refresh rate or HDR.
//!
//!   fullscreen_probe DISPLAY SECONDS REPORT.json [WINDOWED_MS:FULL_MS] [activate]
//!
//! `activate` brings the window to the foreground when it goes fullscreen,
//! as a launched game is; by default it never takes focus.

#[cfg(not(windows))]
fn main() {
    eprintln!("This probe requires Windows.");
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    probe::run()
}

#[cfg(windows)]
mod probe {
    use anyhow::{Context, Result, bail};
    use butterpollo_windows::capture::{ComGuard, displays, enable_dpi_awareness};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    use windows::{
        Win32::{
            Foundation::*,
            Graphics::{
                Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, Fxc::*},
                Direct3D11::*,
                Dxgi::{Common::*, *},
            },
            System::{LibraryLoader::GetModuleHandleW, Performance::*},
            UI::WindowsAndMessaging::*,
        },
        core::{PCSTR, w},
    };

    const SHADER: &str = r#"
cbuffer Frame : register(b0) { uint frame; uint pad0; uint pad1; uint pad2; };
float4 vertex(uint id : SV_VertexID) : SV_Position {
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * float2(2, -2) + float2(-1, 1), 0, 1);
}
float4 picture(float4 p : SV_Position) : SV_Target {
    float2 at = p.xy + float2(frame * 6, frame * 3);
    float grid = fmod(floor(at.x / 48) + floor(at.y / 48), 2);
    float3 color = lerp(float3(0.05, 0.08, 0.15), float3(0.45, 0.25, 0.1), grid);
    if (fmod(at.x, 400) < 10) color = float3(1, 0.9, 0.6);
    return float4(color, 1);
}
"#;
    const SMALL: (i32, i32) = (640, 360);

    struct Window(HWND);
    impl Drop for Window {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.0);
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
            D3DCompile(
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
                None,
            )?;
            let code = code.context("shader compiler returned no bytecode")?;
            Ok(
                std::slice::from_raw_parts(code.GetBufferPointer().cast(), code.GetBufferSize())
                    .to_vec(),
            )
        }
    }

    fn utc_ms() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
    }

    fn qpc() -> i64 {
        let mut ticks = 0;
        unsafe {
            let _ = QueryPerformanceCounter(&mut ticks);
        }
        ticks
    }

    pub fn run() -> Result<()> {
        let usage =
            "usage: fullscreen_probe DISPLAY SECONDS REPORT.json [WINDOWED_MS:FULL_MS] [activate]";
        let mut args = std::env::args().skip(1);
        let display_name = args.next().context(usage)?;
        let seconds: u64 = args.next().context(usage)?.parse()?;
        let report = args.next().context(usage)?;
        let cycle = args.next().unwrap_or_else(|| "4000:1500".into());
        let (windowed, full) = cycle
            .split_once(':')
            .and_then(|(a, b)| Some((a.parse::<u64>().ok()?, b.parse::<u64>().ok()?)))
            .context("the cycle is WINDOWED_MS:FULL_MS")?;
        let activate = match args.next().as_deref() {
            None => false,
            Some("activate") => true,
            Some(other) => bail!("unknown option {other}; {usage}"),
        };
        if !(1..=600).contains(&seconds)
            || !(100..=60_000).contains(&windowed)
            || !(100..=10_000).contains(&full)
        {
            bail!("SECONDS must be 1..600, WINDOWED_MS 100..60000 and FULL_MS 100..10000");
        }
        enable_dpi_awareness();
        let _com = ComGuard::new()?;
        let display = displays()?
            .into_iter()
            .find(|d| d.display_name.eq_ignore_ascii_case(&display_name))
            .context("explicitly selected display is not active")?;
        let small_at = (display.x + 64, display.y + 64);
        let full_size = (display.width as i32, display.height as i32);
        unsafe {
            let module = GetModuleHandleW(None)?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(window),
                hInstance: HINSTANCE(module.0),
                lpszClassName: w!("RubylightFullscreenProbe"),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                bail!("cannot register test window");
            }
            let windowed_style = WS_OVERLAPPEDWINDOW | WS_VISIBLE;
            let window = Window(CreateWindowExW(
                WS_EX_NOACTIVATE,
                w!("RubylightFullscreenProbe"),
                w!("Rubylight fullscreen probe"),
                windowed_style,
                small_at.0,
                small_at.1,
                SMALL.0,
                SMALL.1,
                None,
                None,
                Some(HINSTANCE(module.0)),
                None,
            )?);
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
            let client = |hwnd: HWND| -> Result<(u32, u32)> {
                let mut rect = RECT::default();
                GetClientRect(hwnd, &mut rect)?;
                Ok((
                    (rect.right - rect.left).max(1) as u32,
                    (rect.bottom - rect.top).max(1) as u32,
                ))
            };
            let (w0, h0) = client(window.0)?;
            let swap = factory.CreateSwapChainForHwnd(
                &device,
                window.0,
                &DXGI_SWAP_CHAIN_DESC1 {
                    Width: w0,
                    Height: h0,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                    BufferCount: 2,
                    SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                    Scaling: DXGI_SCALING_STRETCH,
                    AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                    ..Default::default()
                },
                None,
                None,
            )?;
            factory.MakeWindowAssociation(window.0, DXGI_MWA_NO_ALT_ENTER)?;
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
            let mut target: Option<ID3D11RenderTargetView> = None;
            let mut size = (w0, h0);
            let bind =
                |size: (u32, u32), target: &mut Option<ID3D11RenderTargetView>| -> Result<()> {
                    let texture: ID3D11Texture2D = swap.GetBuffer(0)?;
                    device.CreateRenderTargetView(&texture, None, Some(target))?;
                    context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                        Width: size.0 as f32,
                        Height: size.1 as f32,
                        MinDepth: 0.,
                        MaxDepth: 1.,
                        ..Default::default()
                    }]));
                    Ok(())
                };
            bind(size, &mut target)?;
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(vertex.as_ref(), None);
            context.PSSetShader(picture.as_ref(), None);
            context.PSSetConstantBuffers(0, Some(&[Some(constants.clone())]));
            let mut frequency = 0;
            QueryPerformanceFrequency(&mut frequency)?;
            println!("FULLSCREEN stage=ready display={}", display.display_name);
            let start = Instant::now();
            let mut fullscreen = false;
            let mut switch_at = start + Duration::from_millis(windowed);
            let mut presents = Vec::new();
            let mut switches = Vec::new();
            let mut frame = 0u32;
            while start.elapsed() < Duration::from_secs(seconds) {
                let mut message = MSG::default();
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    if message.message == WM_QUIT {
                        bail!("test window closed");
                    }
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                if Instant::now() >= switch_at {
                    fullscreen = !fullscreen;
                    let begin = (qpc(), utc_ms());
                    // The way games switch: style first, then the window
                    // covers the display, then the buffers follow its size.
                    if fullscreen {
                        SetWindowLongPtrW(window.0, GWL_STYLE, (WS_POPUP | WS_VISIBLE).0 as isize);
                        SetWindowPos(
                            window.0,
                            Some(HWND_TOPMOST),
                            display.x,
                            display.y,
                            full_size.0,
                            full_size.1,
                            SWP_FRAMECHANGED
                                | if activate {
                                    SET_WINDOW_POS_FLAGS(0)
                                } else {
                                    SWP_NOACTIVATE
                                },
                        )?;
                        if activate {
                            let _ = SetForegroundWindow(window.0);
                        }
                    } else {
                        SetWindowLongPtrW(window.0, GWL_STYLE, windowed_style.0 as isize);
                        SetWindowPos(
                            window.0,
                            Some(HWND_NOTOPMOST),
                            small_at.0,
                            small_at.1,
                            SMALL.0,
                            SMALL.1,
                            SWP_FRAMECHANGED | SWP_NOACTIVATE,
                        )?;
                    }
                    target = None;
                    context.OMSetRenderTargets(None, None);
                    size = client(window.0)?;
                    swap.ResizeBuffers(
                        0,
                        size.0,
                        size.1,
                        DXGI_FORMAT_UNKNOWN,
                        DXGI_SWAP_CHAIN_FLAG(0),
                    )?;
                    bind(size, &mut target)?;
                    let end = (qpc(), utc_ms());
                    println!(
                        "FULLSCREEN switch fullscreen={fullscreen} size={}x{} took_ms={:.1}",
                        size.0,
                        size.1,
                        (end.0 - begin.0) as f64 * 1000.0 / frequency as f64
                    );
                    switches.push(serde_json::json!({
                        "fullscreen": fullscreen, "width": size.0, "height": size.1,
                        "qpc": begin.0, "utc_ms": begin.1, "done_qpc": end.0, "done_utc_ms": end.1,
                    }));
                    switch_at = Instant::now()
                        + Duration::from_millis(if fullscreen { full } else { windowed });
                }
                frame += 1;
                let values = [frame, 0, 0, 0];
                context.UpdateSubresource(&constants, 0, None, values.as_ptr().cast(), 0, 0);
                context.OMSetRenderTargets(Some(&[target.clone()]), None);
                context.Draw(3, 0);
                swap.Present(1, DXGI_PRESENT(0)).ok()?;
                presents.push(qpc());
            }
            // Always end small, so a stopped probe never leaves the display covered.
            if fullscreen {
                SetWindowLongPtrW(window.0, GWL_STYLE, windowed_style.0 as isize);
                let _ = SetWindowPos(
                    window.0,
                    Some(HWND_NOTOPMOST),
                    small_at.0,
                    small_at.1,
                    SMALL.0,
                    SMALL.1,
                    SWP_FRAMECHANGED | SWP_NOACTIVATE,
                );
            }
            let gaps: Vec<f64> = presents
                .windows(2)
                .map(|w| (w[1] - w[0]) as f64 * 1000.0 / frequency as f64)
                .collect();
            let max_gap = gaps.iter().copied().fold(0.0, f64::max);
            let data = serde_json::json!({
                "display": display, "seconds": start.elapsed().as_secs_f64(), "qpc_frequency": frequency,
                "windowed_ms": windowed, "full_ms": full, "activate": activate,
                "presented_frames": presents.len(), "max_present_gap_ms": max_gap,
                "switches": switches, "presents_qpc": presents,
            });
            std::fs::write(&report, serde_json::to_vec_pretty(&data)?)?;
            println!(
                "FULLSCREEN frames={} switches={} max_present_gap_ms={max_gap:.1}",
                presents.len(),
                switches.len()
            );
        }
        Ok(())
    }
}
