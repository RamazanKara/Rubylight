//! Unit tests for [`GpuPool`] shared-signal fallback and capture-surface COM refcount lifetime.

use super::*;
use windows::core::Interface;

#[test]
fn shared_d3d11_signal_recreates_once_and_downgrades_when_unavailable() {
    // Steady-state existing signal succeeds without recreating.
    let mut slot = Some(1u32);
    let mut created = 0u32;
    let ready = resolve_shared_signal(
        &mut slot,
        |id| Ok::<u32, &'static str>(*id * 10),
        || {
            created += 1;
            Some(2)
        },
    );
    assert_eq!(ready, Some(10));
    assert_eq!(created, 0);

    // Retired compute queue: existing signal errors once -> recreated signal succeeds.
    let mut slot = Some(1u32);
    let mut created = 0u32;
    let ready = resolve_shared_signal(
        &mut slot,
        |id| {
            if *id == 1 {
                Err("shared compute queue is retired")
            } else {
                Ok(*id * 10)
            }
        },
        || {
            created += 1;
            Some(2)
        },
    );
    assert_eq!(ready, Some(20));
    assert_eq!(created, 1);
    assert_eq!(slot, Some(2));

    // Recreation unavailable: returns None (caller must downgrade off SHARED_NTHANDLE
    // instead of emitting an unsynchronized shared texture with `ready: None`).
    let mut slot = Some(1u32);
    let ready = resolve_shared_signal(&mut slot, |_| Err::<u32, _>("retired"), || None);
    assert_eq!(ready, None);
    assert_eq!(slot, None);
}

#[test]
fn shared_d3d11_downgrade_sets_capture_compute_warning() {
    use std::sync::{Arc, Mutex};
    #[derive(Default)]
    struct Sink(Mutex<Vec<(String, String)>>);
    impl butterpollo_media_policy::WarningSink for Sink {
        fn set(&self, key: &str, text: &str) {
            self.0
                .lock()
                .unwrap()
                .push((key.to_string(), text.to_string()));
        }
        fn clear(&self, _key: &str) {}
    }
    let sink = Arc::new(Sink::default());
    let mut pool = GpuPool {
        warnings: sink.clone(),
        shared: true,
        ..Default::default()
    };
    pool.downgrade_shared_d3d11();
    assert!(!pool.shared);
    assert!(pool.d3d11_signal.is_none());
    let recorded = sink.0.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].0, "capture_compute");
    assert!(recorded[0].1.contains("Shared D3D11 fence unavailable"));
}

#[test]
fn gpu_pool_copy_does_not_retain_strong_com_ref_to_source_surface() -> Result<()> {
    use windows::Win32::{
        Foundation::HMODULE,
        Graphics::Direct3D::{D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0},
    };
    let mut device = None;
    let mut context = None;
    // SAFETY: WARP is Windows' software D3D11 runtime; output pointers are live locals.
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_WARP,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
    }
    let device = device.context("WARP D3D11 device")?;
    let context = context.context("WARP D3D11 context")?;
    let desc = D3D11_TEXTURE2D_DESC {
        Width: 16,
        Height: 16,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        ..Default::default()
    };
    let mut source = None;
    // SAFETY: `device` is live and `desc` describes a valid 16x16 BGRA8 texture.
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut source))? };
    let source = source.unwrap();
    let refcount = |tex: &ID3D11Texture2D| -> u32 {
        let unknown: windows::core::IUnknown = tex.cast().unwrap();
        // SAFETY: `unknown` is a live COM interface; AddRef followed by Release returns the
        // pre-AddRef reference count without altering ownership.
        unsafe {
            let vtbl = unknown.vtable();
            (vtbl.AddRef)(unknown.as_raw());
            (vtbl.Release)(unknown.as_raw()) - 1
        }
    };
    let baseline = refcount(&source);
    let mut pool = GpuPool::default();
    let copied = pool.copy_d3d11_on(&device, &context, &source, None)?;
    assert!(copied.is_some());
    // Flush any deferred command-buffer references and verify GpuPool holds zero COM refs.
    // SAFETY: `context` is live and Flush takes no borrowed pointers.
    unsafe { context.Flush() };
    assert_eq!(
        refcount(&source),
        baseline,
        "GpuPool must not pin capture source surfaces past copy()"
    );
    Ok(())
}
