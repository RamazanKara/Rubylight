//! Display scale for a stream: the virtual display's, a kept or remembered scale, and the
//! physical display's calibration.
use super::*;

/// The scale a stream's display gets: the configured virtual display scale for a
/// virtual display or a remote monitor's retained one, and 0 (leave it alone) for a
/// physical display, whose scale belongs to the user.
pub(super) fn virtual_display_scale(virtual_layout: bool, configured: i64) -> i64 {
    if virtual_layout { configured } else { 0 }
}

/// How a stream's display scale is set up ([`scale_setup`]).
#[derive(Debug, PartialEq)]
pub(super) struct ScaleSetup {
    /// The scale for [`set_up_scale`]; 0 leaves it to a kept scale or to Windows.
    pub(super) scale: i64,
    /// The scale is the device's own launch request (`rlDisplayScale`): Windows refusing it
    /// is a warning, as for a kept scale, while a host or device setting stays fatal.
    pub(super) requested: bool,
    /// The device's scale was lowered to the largest the display's resolution allows.
    pub(super) lowered: Option<String>,
    /// Read the scale Windows shows, to keep one picked in Windows during the stream.
    pub(super) learns: bool,
}

impl ScaleSetup {
    /// The device's warning for this stream once the scale in force is known: Windows refused
    /// the scale it asked for (none is in force), else it was lowered.
    pub(super) fn warning(&self, in_force: i64) -> Option<String> {
        if self.requested && in_force == 0 {
            return Some(format!(
                "Windows did not take the {}% display scaling the device asked for, so the virtual display keeps the scale Windows shows. Choose another PC display scaling on the device.",
                self.scale
            ));
        }
        self.lowered.clone()
    }
}

/// The scale a stream's display gets. On a virtual display (or a remote monitor's retained
/// one) the client's `rlDisplayScale` comes first, held to the `largest` scale Windows offers
/// at the display's resolution, then the merged `dd_virtual_display_scale` (device, app, host).
/// A physical display gets none here, and the client's scale is ignored for it. A scale is
/// learned from Windows only when the device `keeps` one and none is configured or requested.
pub(super) fn scale_setup(
    config: &Config,
    options: &std::collections::BTreeMap<String, String>,
    virtual_layout: bool,
    keeps: bool,
    largest: impl FnOnce() -> Result<u32>,
) -> Result<ScaleSetup> {
    // Checked at /launch too, so a bad value never gets this far.
    let requested = butterpollo_core::display_policy::client_display_scale(options)?;
    if let Some(chosen) = requested.filter(|_| virtual_layout) {
        let (scale, lowered) = fit_client_scale(chosen, largest());
        return Ok(ScaleSetup {
            scale,
            requested: true,
            lowered,
            learns: false,
        });
    }
    let configured = config.integer("dd_virtual_display_scale", 0);
    Ok(ScaleSetup {
        scale: virtual_display_scale(virtual_layout, configured),
        requested: false,
        lowered: None,
        // A configured scale is the user's setting, so nothing is learned then.
        learns: keeps && configured == 0,
    })
}

/// A client's display scale, held to the largest Windows offers the display at
/// its resolution: a scale too large for it is lowered, with a warning, instead
/// of failing every stream. A range Windows does not report keeps the choice,
/// and a refusal is then a warning ([`ScaleSetup::warning`]).
pub(super) fn fit_client_scale(chosen: i64, largest: Result<u32>) -> (i64, Option<String>) {
    match largest {
        Ok(largest) if chosen > i64::from(largest) => (
            i64::from(largest),
            Some(format!(
                "The device asked for {chosen}% display scaling, but Windows offers at most {largest}% at this resolution, so the virtual display uses {largest}%. Choose a smaller PC display scaling on the device, or a higher resolution."
            )),
        ),
        Ok(_) => (chosen, None),
        Err(error) => {
            tracing::warn!(scale = chosen, error = %format!("{error:#}"), "the display's scale range could not be read; asking Windows for the device's scale as chosen");
            (chosen, None)
        }
    }
}

/// Whether a paused stream keeps its layout's physical monitors off. Only an
/// exclusive layout switches them off, and only on request: by default a paused
/// stream leaves local display changes alone.
pub(super) fn holds_layout_while_paused(config: &Config, exclusive: bool) -> bool {
    exclusive && config.boolean("dd_hold_layout_while_paused", false)
}

/// Puts the configured virtual display scale on `output`, whichever display it is: a stream's
/// own, or a remote monitor's retained one, which nothing else scales.
pub(super) fn scale_display(
    output: &str,
    configured: i64,
    (width, height): (u32, u32),
    set: impl FnOnce(&str, i64, u32, u32) -> Result<()>,
) -> Result<()> {
    set(output, configured, width, height)
}

/// The ICC profile an HDR stream's display gets: the device's own choice,
/// else, when the host carries it over to a new virtual display, the
/// physical display's Windows HDR Calibration.
pub(super) fn stream_profile(
    hdr: bool,
    chosen: Option<&str>,
    carry_calibration: bool,
    calibration: impl FnOnce() -> Option<String>,
) -> Option<String> {
    if !hdr {
        return None;
    }
    match chosen.filter(|name| !name.is_empty()) {
        Some(name) => Some(name.to_owned()),
        None if carry_calibration => calibration(),
        None => None,
    }
}

/// The Windows HDR Calibration of a physical HDR display, for the virtual
/// display that stands in for it. A stream without one says so.
pub(super) fn physical_calibration() -> Option<String> {
    match hdr_profile::calibration() {
        Ok(Some(calibration)) => {
            tracing::info!(
                display = %calibration.display,
                profile = %calibration.profile,
                peak_nits = calibration.peak,
                "the virtual display gets the physical display's Windows HDR Calibration"
            );
            Some(calibration.profile)
        }
        Ok(None) => {
            tracing::warn!(
                "virtual_display_hdr_calibration is on, but no physical HDR display has a Windows HDR Calibration profile, so the virtual display streams uncalibrated. Run the Windows HDR Calibration app on the HDR display, or choose a profile for the device"
            );
            None
        }
        Err(error) => {
            tracing::warn!(
                error = %format!("{error:#}"),
                "the physical display's Windows HDR Calibration could not be read, so the virtual display streams uncalibrated"
            );
            None
        }
    }
}

/// The scale a device kept from Windows, in percent, when it keeps one (`remember`).
pub(super) fn kept_scale(
    remember: bool,
    extra: &std::collections::BTreeMap<String, serde_json::Value>,
) -> Option<u32> {
    if !remember {
        return None;
    }
    extra
        .get(REMEMBERED_SCALE)
        .and_then(serde_json::Value::as_u64)
        .and_then(|percent| u32::try_from(percent).ok())
        .filter(|percent| *percent > 0)
}

/// Puts the scale on a stream's virtual display: the configured one when there is one, else
/// the one the device kept from Windows (`remember_display_mode`). A configured scale always
/// wins. The kept one is applied after the display's mode is set, so it sits on the mode the
/// device keeps, and Windows refusing it is a warning: the stream goes on at the scale Windows
/// shows. So is refusing a scale the device `requested` at launch; refusing a configured one
/// fails the stream. Returns the scale the display is in force at, for recovery to put back
/// (0 after a refusal: recovery does not retry it).
pub(super) fn apply_virtual_scale(
    output: &str,
    configured: i64,
    requested: bool,
    remembered: Option<u32>,
    (width, height): (u32, u32),
    set: impl FnOnce(&str, i64, u32, u32) -> Result<()>,
) -> Result<i64> {
    match (configured, remembered) {
        (percent, _) if requested && percent != 0 => {
            match scale_display(output, percent, (width, height), set) {
                Ok(()) => Ok(percent),
                Err(error) => {
                    tracing::warn!(
                        scale = percent,
                        error = %format!("{error:#}"),
                        "Windows did not take the display scale the device asked for; the display keeps the scale Windows shows"
                    );
                    Ok(0)
                }
            }
        }
        (0, Some(percent)) => match set(output, i64::from(percent), width, height) {
            Ok(()) => Ok(i64::from(percent)),
            Err(error) => {
                tracing::warn!(
                    scale = percent,
                    error = %format!("{error:#}"),
                    "Windows did not take the display scale this device kept; the display keeps the scale Windows shows"
                );
                Ok(0)
            }
        },
        _ => scale_display(output, configured, (width, height), set).map(|()| configured),
    }
}

/// Sets the stream's display scale, then reads the one Windows shows as the baseline a scale
/// picked during the stream is compared with. It is read after the kept scale is applied, so a
/// scale Windows refused is never mistaken for one the user picked. `learns` is false when a
/// scale is configured or the device does not keep one: nothing is read then.
#[allow(clippy::too_many_arguments)]
pub(super) fn set_up_scale(
    output: &str,
    configured: i64,
    requested: bool,
    remembered: Option<u32>,
    dimensions: (u32, u32),
    learns: bool,
    set: impl FnOnce(&str, i64, u32, u32) -> Result<()>,
    read: impl FnOnce(&str) -> Result<u32>,
) -> Result<(i64, Option<u32>)> {
    let in_force = apply_virtual_scale(output, configured, requested, remembered, dimensions, set)?;
    let baseline = if learns {
        read(output)
            .inspect_err(|error| {
                tracing::warn!(error = %format!("{error:#}"), "the display scale could not be read; a scale picked in Windows is not kept")
            })
            .ok()
    } else {
        None
    };
    Ok((in_force, baseline))
}

/// The Windows scale a physical display takes for a stream: the device's own
/// `dd_virtual_display_scale` override only. The host's setting and an app's
/// are for virtual displays; a physical display's scale is the user's unless
/// the device asks for one. 0 leaves it.
pub(super) fn physical_scale(client: &butterpollo_core::state::Client) -> i64 {
    const KEY: &str = "dd_virtual_display_scale";
    let Some(value) = client
        .extra
        .get("config_overrides")
        .and_then(serde_json::Value::as_object)
        .and_then(|overrides| overrides.get(KEY))
    else {
        return 0;
    };
    let mut own = Config::default();
    // A value the host cannot use was reported when the device's settings
    // were applied to the stream; it leaves the scale as it is.
    if own
        .update(&serde_json::Map::from_iter([(
            KEY.to_owned(),
            value.clone(),
        )]))
        .is_err()
    {
        return 0;
    }
    own.integer(KEY, 0)
}

/// The size a stream's own virtual display is made at: `size` in the shape
/// `dd_rotation_option` gives it. It is made in that shape rather than turned.
pub(super) fn virtual_display_size(
    config: &Config,
    size: (u32, u32),
    stream: (u32, u32),
) -> (u32, u32) {
    butterpollo_core::display_policy::virtual_shape(
        config.dd_rotation_option(),
        config.dd_manual_rotation(),
        size,
        stream,
    )
}
