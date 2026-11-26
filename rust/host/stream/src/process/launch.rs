//! Application launch preparation, environment assembly, and deferred sign-in placeholders.

use super::RunningApp;
use super::commands::{app_bool, exit_timeout, expand};
use super::focus::running_settings_unwatched;
use anyhow::{Context, Result};
use butterpollo_session::state::{App, PrepCommand};
use std::collections::{BTreeMap, HashMap};

/// Whether starting the app runs any command, which needs the user's
/// environment and token.
fn runs_commands(h: &crate::state::Shared, app: &App) -> bool {
    !app.cmd.trim().is_empty()
        || !app.prep.is_empty()
        || app
            .extra
            .get("playnite-id")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|id| !id.trim().is_empty())
        || playnite_fullscreen(app)
        || app
            .extra
            .get("detached")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|commands| !commands.is_empty())
        || (!app_bool(app, "exclude-global-prep-cmd", false)
            && h.config.read().unwrap().get("global_prep_cmd", "[]").trim() != "[]")
}
/// The launch argument the host sets for a background stream's app; see
/// [`butterpollo_session::background`].
pub const BACKGROUND_ARG: &str = "rubylightBackgroundStream";
/// A failed preparation command as the device shows it: the app, which command (counted
/// across the global ones and the app's own, in the order they run), what went wrong and
/// where to look.
pub(super) fn prep_failure(app: &str, index: usize, what: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "{app}: preparation command {} {what}. Check the app's and the global preparation commands in the host console.",
        index + 1
    )
}
/// Why waiting for preparation command `index` failed. When the launch was `stopped`, the stop
/// is the reason, also when ending the command failed (an elevated command Windows would not
/// end, or one slow to go): not a timeout.
pub(super) fn wait_failure(app: &str, index: usize, stopped: bool) -> anyhow::Error {
    if stopped {
        anyhow::anyhow!(LAUNCH_STOPPED)
    } else {
        prep_failure(app, index, "did not finish within 120 seconds")
    }
}
/// Why a launch that Quit, End session or the device's next launch stopped failed.
pub const LAUNCH_STOPPED: &str = "The launch was stopped before the game started.";
pub fn launch(
    h: &crate::state::Shared,
    app: &App,
    args: &HashMap<String, String>,
) -> Result<RunningApp> {
    launch_with_environment(
        h,
        app,
        args,
        butterpollo_win_io::process::user_environment(),
    )
}
pub(super) fn launch_with_environment(
    h: &crate::state::Shared,
    app: &App,
    args: &HashMap<String, String>,
    environment: Result<BTreeMap<String, String>>,
) -> Result<RunningApp> {
    let background = args.get(BACKGROUND_ARG).is_some_and(|v| v == "1");
    let terminate_on_pause = app_bool(
        app,
        "terminate-on-pause",
        h.config
            .read()
            .unwrap()
            .boolean("terminate_on_pause", false),
    );
    let exit_timeout = exit_timeout(app, &h.config.read().unwrap());
    let scale_factor = args
        .get("scaleFactor")
        .map(|v| v.parse::<u32>())
        .transpose()?
        .unwrap_or(100);
    let gcmap = args
        .get("gcmap")
        .map(|v| v.parse::<u32>())
        .transpose()?
        .unwrap_or(0);
    // The PlayStation controllers among gcmap's slots (`psmap`, additive; older clients omit it).
    let psmap = args
        .get("psmap")
        .map(|v| v.parse::<u32>())
        .transpose()?
        .unwrap_or(0)
        & gcmap
        & 0xFFFF;
    if let Some(value) = args.get("surroundParams") {
        value.parse::<u64>()?;
    }
    if let Some(value) = args.get("surroundAudioInfo") {
        // Java clients send 7.1.4's mask << 16 | count as a negative int.
        crate::stream::audio_encoder::surround_audio_info(value)
            .with_context(|| format!("surroundAudioInfo {value:?} is not a number"))?;
    }
    let mut environment = match environment {
        Ok(environment) => environment,
        Err(error) => {
            // Before anyone signs in (a service after a reboot) the sign-in
            // screen can still be streamed. As in Vibepollo, the app's
            // commands wait until a user signs in.
            let mut placeholder = app.clone();
            placeholder.cmd.clear();
            placeholder.prep.clear();
            placeholder.extra.remove("detached");
            placeholder.extra.remove("state-cmd");
            placeholder
                .extra
                .insert("exclude-global-prep-cmd".into(), true.into());
            placeholder
                .extra
                .insert("exclude-global-state-cmd".into(), true.into());
            let mut system = BTreeMap::new();
            // The launching client owns the session, as with a started app.
            for (arg, name) in [("clientUuid", "UUID"), ("clientName", "NAME")] {
                if let Some(value) = args.get(arg) {
                    let value = if arg == "clientName" {
                        butterpollo_crypto::pairing::command_device_name(value)
                    } else {
                        value.clone()
                    };
                    system.insert(format!("SUNSHINE_CLIENT_{name}"), value.clone());
                    system.insert(format!("APOLLO_CLIENT_{name}"), value);
                }
            }
            let mut running = RunningApp::with_environment(&placeholder, system)?;
            running.background = background;
            running.terminate_on_pause = terminate_on_pause;
            running.exit_timeout = exit_timeout;
            if runs_commands(h, app) {
                tracing::info!(app = %app.name, %error, "no signed-in user; the application starts after sign-in");
                running.deferred = Some((app.clone(), args.clone()));
            }
            return Ok(running);
        }
    };
    // A broken or outdated mod install fails the launch here, loudly.
    crate::neural::before_launch(h, app)?;
    let mut app = app.clone();
    let document = h.app_document.read().unwrap();
    if let Some(values) = document.get("env").and_then(serde_json::Value::as_object) {
        for (key, value) in values {
            environment.insert(
                key.to_uppercase(),
                expand(
                    value.as_str().ok_or_else(|| {
                        anyhow::anyhow!("application environment must contain strings")
                    })?,
                    &environment,
                )?,
            );
        }
    }
    drop(document);
    if background {
        for (key, value) in butterpollo_session::background::ENVIRONMENT {
            environment.insert(key.into(), value.into());
        }
    }
    if !app_bool(&app, "exclude-global-state-cmd", false) {
        let mut global: Vec<PrepCommand> =
            serde_json::from_str(h.config.read().unwrap().get("global_state_cmd", "[]"))?;
        let mut local: Vec<PrepCommand> = serde_json::from_value(
            app.extra
                .get("state-cmd")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        )?;
        global.append(&mut local);
        app.extra
            .insert("state-cmd".into(), serde_json::to_value(global)?);
    }
    let mode: Vec<_> = args
        .get("mode")
        .map_or("1920x1080x60", String::as_str)
        .split('x')
        .collect();
    let (render_width, render_height) = butterpollo_media_policy::display_policy::render_dimensions(
        mode.first()
            .and_then(|value| value.parse().ok())
            .unwrap_or(1920),
        mode.get(1)
            .and_then(|value| value.parse().ok())
            .unwrap_or(1080),
        i64::from(scale_factor),
        app.extra
            .get("scale-factor")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(100),
    );
    for (name, value) in [
        ("SUNSHINE_APP_ID", app.id().to_string()),
        ("SUNSHINE_APP_NAME", app.name.clone()),
        ("SUNSHINE_CLIENT_WIDTH", render_width.to_string()),
        ("SUNSHINE_CLIENT_HEIGHT", render_height.to_string()),
        (
            "SUNSHINE_CLIENT_FPS",
            mode.get(2).unwrap_or(&"60").to_string(),
        ),
        (
            "SUNSHINE_CLIENT_HDR",
            if args.get("hdrMode").is_some_and(|v| v == "1") {
                "true"
            } else {
                "false"
            }
            .into(),
        ),
        ("SUNSHINE_CLIENT_GCMAP", gcmap.to_string()),
        ("SUNSHINE_CLIENT_PSMAP", psmap.to_string()),
        (
            "SUNSHINE_CLIENT_HOST_AUDIO",
            if args.get("localAudioPlayMode").is_some_and(|v| v == "1") {
                "true"
            } else {
                "false"
            }
            .into(),
        ),
        (
            "SUNSHINE_CLIENT_ENABLE_SOPS",
            if args.get("sops").is_some_and(|v| v == "1") {
                "true"
            } else {
                "false"
            }
            .into(),
        ),
    ] {
        environment.insert(name.into(), value);
    }
    if let Some(rate) = mode.get(2) {
        let rate = if rate.contains('.') {
            butterpollo_media_policy::framegen::Rate::parse(rate)?
        } else {
            butterpollo_media_policy::framegen::Rate::from_client(rate.parse()?)
        };
        environment.insert(
            "SUNSHINE_CLIENT_FPS".into(),
            if h.config
                .read()
                .unwrap()
                .boolean("envvar_compatibility_mode", false)
            {
                rate.rounded().to_string()
            } else {
                rate.to_string()
            },
        );
    }
    for (key, value) in args {
        let name = match key.as_str() {
            "clientName" => Some("NAME"),
            "clientUuid" => Some("UUID"),
            "surroundParams" => Some("AUDIO_SURROUND_PARAMS"),
            "surroundAudioInfo" => Some("AUDIO_CONFIGURATION"),
            _ => None,
        };
        if let Some(name) = name {
            let value = if key == "clientName" {
                butterpollo_crypto::pairing::command_device_name(value)
            } else {
                value.clone()
            };
            environment.insert(format!("SUNSHINE_CLIENT_{name}"), value);
        }
    }
    for (key, value) in environment.clone() {
        if let Some(suffix) = key.strip_prefix("SUNSHINE_") {
            environment.insert(format!("APOLLO_{suffix}"), value);
        }
    }
    // Apollo/Vibepollo additions: the app's UUID, the client's own mode
    // before render scaling, and its scale factor.
    for (name, value) in [
        (
            "APOLLO_APP_UUID",
            app.extra
                .get("uuid")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        ),
        (
            "APOLLO_CLIENT_RENDER_WIDTH",
            mode.first()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(1920)
                .to_string(),
        ),
        (
            "APOLLO_CLIENT_RENDER_HEIGHT",
            mode.get(1)
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(1080)
                .to_string(),
        ),
        ("APOLLO_CLIENT_SCALE_FACTOR", scale_factor.to_string()),
        ("APOLLO_APP_STATUS", "STARTING".to_owned()),
    ] {
        environment.insert(name.into(), value);
    }
    if !app_bool(&app, "exclude-global-prep-cmd", false) {
        let mut global: Vec<PrepCommand> =
            serde_json::from_str(h.config.read().unwrap().get("global_prep_cmd", "[]"))?;
        global.append(&mut app.prep);
        app.prep = global;
    }
    app.cmd = expand(&app.cmd, &environment)?;
    app.working_dir = expand(&app.working_dir, &environment)?;
    for prep in &mut app.prep {
        prep.r#do = expand(&prep.r#do, &environment)?;
        prep.undo = expand(&prep.undo, &environment)?;
    }
    if let Some(commands) = app
        .extra
        .get_mut("detached")
        .and_then(serde_json::Value::as_array_mut)
    {
        for command in commands {
            *command = serde_json::Value::String(expand(
                command
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("detached command must be a string"))?,
                &environment,
            )?);
        }
    }
    // An app linked to a Playnite game has no command: Playnite starts it.
    let playnite_id = app
        .extra
        .get("playnite-id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty() && app.cmd.trim().is_empty())
        .map(str::to_owned);
    let fullscreen = app.cmd.trim().is_empty() && playnite_fullscreen(&app);
    let config = h.config.read().unwrap().clone();
    // Lossless Scaling scales the focused window; it would pull the game in
    // front of the person using the PC.
    let lossless = if fullscreen || background {
        None
    } else {
        let fps = mode
            .get(2)
            .and_then(|rate| rate.parse::<f64>().ok())
            .map_or(60., |rate| if rate >= 1000. { rate / 1000. } else { rate });
        butterpollo_media_policy::lossless::options(&serde_json::to_value(&app)?, &config, fps)
    };
    // Lossless Scaling looks for the game among the processes that start
    // after this.
    let baseline = lossless
        .as_ref()
        .map(|_| butterpollo_win_io::process::processes().unwrap_or_default());
    // The game reads its own settings files as it starts: the device's own go in first (a
    // launch they cannot be swapped for fails), then the stream's keys.
    let settings_swap = crate::game_settings::launching(h, &app, args, &environment)?;
    // Until the game runs, a failed launch puts back only the PC's files: the device's copies
    // stay as they were.
    let cancel = |swap: Option<crate::game_settings::Swap>| {
        if let Some(swap) = swap {
            swap.cancel();
        }
    };
    let swapped = settings_swap
        .as_ref()
        .map_or_else(Vec::new, |swap| swap.files().to_vec());
    if let Err(error) = crate::game_files::launching_beside(h, &app, args, &swapped) {
        cancel(settings_swap);
        return Err(error);
    }
    let cancellation = h.launch_cancellation();
    let mut running = match RunningApp::with_environment_cancellable(
        &app,
        environment,
        cancellation.as_deref(),
    ) {
        Ok(running) => running,
        Err(error) => {
            cancel(settings_swap);
            return Err(error);
        }
    };
    if running_settings_unwatched(&app) && settings_swap.is_some() {
        let warning = format!(
            "{}: game settings files: this app starts its game through detached commands or a launcher, whose game the host may not see, so its settings files go back when the stream ends even if the game still runs. The PC's last three files stay as backups.",
            app.name
        );
        h.warnings
            .set(crate::game_settings::UNWATCHED_WARNING, warning);
    }
    running.settings_swap = settings_swap;
    // Recorded at once: a host crash in the first seconds still waits for the game.
    running.watch_game();
    running.background = background;
    running.terminate_on_pause = terminate_on_pause;
    running.exit_timeout = exit_timeout;
    if fullscreen || (playnite_id.is_some() && config.boolean("playnite_enabled", true)) {
        let launch = crate::playnite::Launch::start(
            h,
            if fullscreen {
                None
            } else {
                playnite_id.as_deref()
            },
            &running.environment,
            background,
            running.settings_swap.is_some(),
        );
        match launch {
            Ok(launch) => running.playnite = Some(launch),
            Err(error) => {
                cancel(running.settings_swap.take());
                return Err(error);
            }
        }
    }
    running.playnite_fullscreen = fullscreen;
    if let (Some(options), Some(baseline)) = (lossless, baseline) {
        let folder: Box<dyn Fn() -> Option<String> + Send> = match &running.playnite {
            Some(launch) => Box::new(launch.folder()),
            None => {
                let steam = app
                    .extra
                    .get("steam-install-dir")
                    .and_then(serde_json::Value::as_str)
                    .filter(|folder| !folder.trim().is_empty())
                    .map(str::to_owned)
                    .or_else(|| (!running.working.is_empty()).then(|| running.working.clone()));
                Box::new(move || steam.clone())
            }
        };
        running.lossless = Some(crate::lossless::Session::start(
            options,
            config.get("lossless_scaling_path", "").to_owned(),
            baseline,
            running.child.as_ref().map_or(0, |child| child.pid),
            folder,
        ));
    }
    Ok(running)
}
fn playnite_fullscreen(app: &App) -> bool {
    match app.extra.get("playnite-fullscreen") {
        Some(serde_json::Value::Number(n)) => n.as_i64() == Some(1),
        _ => app_bool(app, "playnite-fullscreen", false),
    }
}
