//! One answer to which GPU Rubylight's virtual display renders on and which GPU
//! its `idd-push` capture opens: the GPU the Graphics card setting names, else
//! the one Automatic pins ([`pinned_choice`]), under the LUID Windows uses for
//! it now. The display's render adapter (set when the display is created and
//! again after a driver restart) and the capture device both come from here, so
//! capture never opens on one GPU while the display renders on another, and a
//! chosen GPU that is gone fails with the reason instead of switching GPUs.

use super::Gpu;
use super::adapters::{pinned_choice, select_gpu};
use anyhow::{Result, bail};

/// The console warning a capture that cannot use the chosen GPU sets.
pub(crate) const WARNING: &str = "capture_gpu";

/// What the console says when the GPU the Graphics card setting names is not
/// on the PC now.
pub fn missing_gpu_text(adapter_name: &str, pnp_id: &str) -> String {
    let named = if adapter_name.is_empty() {
        pnp_id
    } else {
        adapter_name
    };
    format!(
        "The GPU chosen under Graphics card ({named}) is not available: Windows lists no such GPU now, as when it is unplugged or disabled or its driver is restarting. Rubylight does not switch to another GPU on its own: reconnect it, or choose another GPU or Automatic under Graphics card, then reconnect."
    )
}

/// The GPU a virtual display renders on: the chosen one, else the one
/// Automatic pins on a PC with several GPUs; None leaves it to Windows. A
/// chosen GPU that is missing is an error that says so.
pub fn render_gpu<'a>(
    gpus: &'a [Gpu],
    adapter_name: &str,
    pnp_id: &str,
) -> Result<Option<&'a Gpu>> {
    let (name, pnp) = pinned_choice(gpus, None, adapter_name, pnp_id);
    if name.is_empty() && pnp.is_empty() {
        return Ok(None);
    }
    match select_gpu(gpus, &name, &pnp) {
        Ok(gpu) => Ok(gpu),
        Err(error) => {
            tracing::debug!(error = %format!("{error:#}"), "the GPU for the virtual display");
            bail!("{}", missing_gpu_text(adapter_name, pnp_id))
        }
    }
}

/// The LUID `idd-push` capture opens its device on. `render` is the GPU the
/// driver reports rendering the display on, zero before it rendered.
///
/// On Automatic that is the GPU it renders on, whichever it is, so capture
/// never mismatches it (the GPUs are listed only before the driver reports
/// one). A chosen GPU must be the one it renders on: when the driver did not
/// take it, capture fails with the reason instead of capturing elsewhere.
/// None opens on the display's own adapter, as before the driver reports a
/// GPU on a PC with nothing to pin.
pub(crate) fn capture_luid(
    render: (u32, i32),
    adapter_name: &str,
    pnp_id: &str,
    list: impl FnOnce() -> Result<Vec<Gpu>>,
) -> Result<Option<(u32, i32)>> {
    let chosen = !adapter_name.is_empty() || !pnp_id.is_empty();
    if !chosen && render != (0, 0) {
        return Ok(Some(render));
    }
    let gpus = list()?;
    let wanted = render_gpu(&gpus, adapter_name, pnp_id)?;
    let Some(gpu) = wanted else {
        return Ok(None);
    };
    if render == (0, 0) || render == gpu.luid {
        return Ok(Some(gpu.luid));
    }
    let rendering = gpus.iter().find(|other| other.luid == render).map_or_else(
        || {
            format!(
                "a GPU Windows no longer lists ({:08x}:{:08x})",
                render.1, render.0
            )
        },
        |other| other.name.clone(),
    );
    bail!(
        "The Rubylight display renders on {rendering}, but Graphics card names {}: the display driver did not take the chosen GPU. Rubylight does not capture on another GPU on its own: reconnect to try again, or choose Automatic or {rendering} under Graphics card.",
        gpu.name
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu(name: &str, vendor: u32, memory: u64, luid: u32, pnp: &str) -> Gpu {
        Gpu {
            name: name.into(),
            vendor,
            dedicated_memory: memory,
            device: 0,
            luid: (luid, 0),
            pnp_id: Some(pnp.into()),
            preference: None,
        }
    }
    /// A Ryzen desktop: its iGPU listed before an RX 7900 XT.
    fn desktop() -> Vec<Gpu> {
        vec![
            gpu(
                "AMD Radeon(TM) Graphics",
                0x1002,
                512 << 20,
                1,
                r"PCI\VEN_1002&DEV_164E\1",
            ),
            gpu(
                "AMD Radeon RX 7900 XT",
                0x1002,
                20 << 30,
                2,
                r"PCI\VEN_1002&DEV_744C\2",
            ),
        ]
    }
    fn listed(gpus: Vec<Gpu>) -> impl FnOnce() -> Result<Vec<Gpu>> {
        move || Ok(gpus)
    }
    fn unlisted() -> impl FnOnce() -> Result<Vec<Gpu>> {
        || panic!("the GPUs were listed although the driver named one")
    }

    #[test]
    fn the_display_renders_on_the_gpu_automatic_pins_or_the_chosen_one() {
        let gpus = desktop();
        assert_eq!(render_gpu(&gpus, "", "").unwrap().unwrap().luid, (2, 0));
        let igpu = render_gpu(&gpus, "AMD Radeon(TM) Graphics", "").unwrap();
        assert_eq!(igpu.unwrap().luid, (1, 0));
        let by_identity = render_gpu(&gpus, "", r"pci\ven_1002&dev_164e\1").unwrap();
        assert_eq!(by_identity.unwrap().luid, (1, 0));
        // A lone GPU is left to Windows.
        assert!(render_gpu(&gpus[1..], "", "").unwrap().is_none());
        // A chosen GPU that is gone: the reason, never another GPU.
        let text = format!(
            "{:#}",
            render_gpu(&gpus, "", r"PCI\VEN_10DE&DEV_2684\9").unwrap_err()
        );
        assert!(
            text.contains(r"(PCI\VEN_10DE&DEV_2684\9) is not available"),
            "{text}"
        );
        assert!(text.contains("does not switch to another GPU"), "{text}");
        let text = missing_gpu_text("NVIDIA GeForce RTX 4090", r"PCI\X");
        assert!(text.contains("(NVIDIA GeForce RTX 4090)"), "{text}");
    }

    #[test]
    fn automatic_capture_opens_on_the_gpu_the_display_renders_on() {
        // The driver names the GPU: no listing, no mismatch, whichever it is.
        assert_eq!(
            capture_luid((2, 0), "", "", unlisted()).unwrap(),
            Some((2, 0))
        );
        assert_eq!(
            capture_luid((1, 0), "", "", unlisted()).unwrap(),
            Some((1, 0))
        );
        // Before it does: the GPU the display was asked to render on.
        assert_eq!(
            capture_luid((0, 0), "", "", listed(desktop())).unwrap(),
            Some((2, 0))
        );
        // A lone GPU before the driver names one: the display's own adapter.
        let lone = desktop().split_off(1);
        assert_eq!(capture_luid((0, 0), "", "", listed(lone)).unwrap(), None);
    }

    #[test]
    fn a_chosen_gpu_must_be_the_one_the_display_renders_on() {
        let card = "AMD Radeon RX 7900 XT";
        assert_eq!(
            capture_luid((2, 0), card, "", listed(desktop())).unwrap(),
            Some((2, 0))
        );
        assert_eq!(
            capture_luid((0, 0), card, "", listed(desktop())).unwrap(),
            Some((2, 0))
        );
        // The driver rendered on the iGPU instead: the reason, not the iGPU.
        let text = format!(
            "{:#}",
            capture_luid((1, 0), card, "", listed(desktop())).unwrap_err()
        );
        assert!(
            text.starts_with("The Rubylight display renders on AMD Radeon(TM) Graphics, but Graphics card names AMD Radeon RX 7900 XT"),
            "{text}"
        );
        // On a GPU Windows no longer lists, after a driver restart.
        let text = format!(
            "{:#}",
            capture_luid((9, 0), card, "", listed(desktop())).unwrap_err()
        );
        assert!(
            text.contains("no longer lists (00000000:00000009)"),
            "{text}"
        );
        // The chosen GPU is gone: the console's reason.
        let gone = desktop().split_off(1);
        let text = format!(
            "{:#}",
            capture_luid((2, 0), "", r"PCI\VEN_1002&DEV_164E\1", listed(gone)).unwrap_err()
        );
        assert!(text.contains("is not available"), "{text}");
        // A listing that fails is the error it gives.
        let failed = capture_luid((2, 0), card, "", || anyhow::bail!("DXGI is gone"));
        assert_eq!(format!("{:#}", failed.unwrap_err()), "DXGI is gone");
    }
}
