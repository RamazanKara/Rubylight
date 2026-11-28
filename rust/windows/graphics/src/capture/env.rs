//! Host-supplied platform environment for capture (`VddBridge` and `UserLauncher`).
use super::*;

#[derive(Clone, Default)]
pub struct CaptureEnv {
    pub vdd: Option<std::sync::Arc<dyn idd_push::VddBridge>>,
    pub user_launcher: Option<std::sync::Arc<dyn crate::user_helper::UserLauncher>>,
}

pub(super) fn try_idd_push(
    name: &str,
    kind: &str,
    hdr: bool,
    config: &butterpollo_config::config::Config,
    warnings: &std::sync::Arc<dyn butterpollo_media_policy::WarningSink>,
    env: &CaptureEnv,
) -> Option<Result<Capture>> {
    if !idd_push::wanted_for(name, kind, config, env.vdd.as_deref()) {
        return None;
    }
    match idd_push::Session::new(name, hdr, config, warnings.clone(), env.vdd.as_deref()) {
        Ok(session) => Some(Ok(Capture::IddPush(Box::new(session)))),
        Err(error) if crate::device_loss::DeviceLost::from_error(&error).is_some() => {
            Some(Err(error))
        }
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "Rubylight display frame push unavailable; capturing with WGC");
            warnings.set("capture_idd_push", &format!("Capturing with WGC: the Rubylight display driver's frame push is unavailable ({error:#})."));
            None
        }
    }
}

pub(super) fn switch_on_device_with_vdd(
    current_cap: &mut Capture,
    name: &str,
    kind: &str,
    hdr: bool,
    config: &butterpollo_config::config::Config,
    warnings: std::sync::Arc<dyn butterpollo_media_policy::WarningSink>,
    vdd: Option<&dyn idd_push::VddBridge>,
) -> Option<Result<Capture>> {
    if current_cap.device_removed().is_some() {
        return None;
    }
    let Capture::Wgc(current) = current_cap else {
        return None;
    };
    let target = WindowTarget::parse(name).ok()?;
    let display = target.as_ref().map_or(name, |t| t.display.as_str());
    let helper = crate::process_info::is_system() || config.boolean("wgc_user_helper", false);
    let push = target.is_none() && idd_push::wanted_for(name, kind, config, vdd);
    if !switches_in_place(
        &current.gpu.display.display_name,
        display,
        target.is_some(),
        kind,
        helper,
        push,
    ) {
        return None;
    }
    let window = target.as_ref().map(WindowTarget::handle);
    let client = target.as_ref().is_some_and(|t| t.client);
    let opened = (|| -> Result<Wgc> {
        if let Some(target) = &target {
            window::require_capture(crate::process_info::windows_build())?;
            window::check(target.handle())?;
        }
        let cursor = if window.is_some() {
            window_cursor(config)
        } else {
            config.boolean("wgc_cursor_capture", true)
        };
        let mut capture = Wgc::new_device(
            current.gpu.clone(),
            hdr,
            config.boolean("wgc_high_rate_capture", false),
            window,
            cursor,
            client,
        )?;
        capture.drain_to_newest = config.boolean("wgc_drain_to_newest", false);
        Ok(capture)
    })();
    Some(opened.map(|mut capture| {
        capture.owned = std::mem::take(&mut current.owned);
        capture.owned.warnings = warnings;
        Capture::Wgc(Box::new(capture))
    }))
}
