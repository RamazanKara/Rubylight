//! The steps of a launch: checking the device and its request, finding the app, building the session,
//! replacing a running game, and preparing the display and the app.

use super::*;

pub(super) fn validate_launch_client(
    h: &Shared,
    connection: &Connection,
    control: Option<Control>,
    resume: bool,
) -> Result<Client> {
    let permission = match control {
        Some(Control::Terminate) => 1 << 26,
        Some(Control::Resume | Control::RunningGame) => 1 << 25,
        _ if resume => 1 << 25,
        _ => 1 << 26,
    };
    if permission == 1 << 25 {
        authenticated_viewer(h, connection)
    } else {
        authenticated(h, connection, permission)
    }
}
pub(super) fn validate_launch_request(
    h: &Shared,
    client: &Client,
    control: Option<Control>,
) -> ControlFlow<Response, remote::Owner> {
    if crate::updater::installing(h) {
        return ControlFlow::Break(xml(
            503,
            &[],
            Some("Rubylight is installing an update. Reconnect shortly.".into()),
        ));
    }
    match control {
        Some(Control::Terminate) => {
            let Some(game) = remote_game(h) else {
                return ControlFlow::Break(xml(
                    409,
                    &[("gamesession", "0".into())],
                    Some("No application is running".into()),
                ));
            };
            let guard = !h
                .config
                .read()
                .unwrap()
                .boolean("remote_monitor_terminate_on_first_request", false)
                && game.owner != client.uuid;
            let mut confirmations = h.confirmations.lock().unwrap();
            if guard
                && !confirmations.confirm(
                    &client.uuid,
                    remote::Confirmation::Terminate,
                    &game.generation,
                    game.app.id,
                    Instant::now(),
                )
            {
                return ControlFlow::Break(xml(410, &[("resume", "0".into()), ("gamesession", "0".into())], Some("This will close the active stream but leave Remote Monitor and Remote Input connected. Launch Terminate again within 60 seconds to confirm this was intentional.".into())));
            }
            confirmations.clear(&client.uuid, remote::Confirmation::Terminate);
            drop(confirmations);
            let canceled = h.sessions.lock().unwrap().stop_role(Role::Stream, None);
            drop(canceled);
            h.stop_app(AppStop::Terminate(&client.uuid));
            return ControlFlow::Break(xml(
                410,
                &[("gamesession", "0".into())],
                Some("Application terminated".into()),
            ));
        }
        Some(Control::DisconnectMonitor) => {
            crate::remote_display::disconnect(h, Some(&client.uuid));
            return ControlFlow::Break(xml(
                410,
                &[("gamesession", "0".into())],
                Some("Remote monitor disconnected".into()),
            ));
        }
        Some(Control::DisconnectInput) => {
            let canceled = h
                .sessions
                .lock()
                .unwrap()
                .stop_role(Role::InputOnly, Some(&client.uuid));
            drop(canceled);
            return ControlFlow::Break(xml(
                410,
                &[("gamesession", "0".into())],
                Some("Remote input disconnected".into()),
            ));
        }
        _ => {}
    }
    let owner = remote_owner(h, &client.uuid);
    if (control == Some(Control::Input) && owner != remote::Owner::None)
        || (control == Some(Control::Monitor) && owner == remote::Owner::Input)
    {
        return ControlFlow::Break(xml(
            409,
            &[("gamesession", "0".into())],
            Some("Remote session action conflicts with this client's current session state".into()),
        ));
    }
    ControlFlow::Continue(owner)
}
/// Why a `/resume` of another device's game found nothing to join. Nightjar and Android read these
/// to say it in the user's language: `nightjar_policy::continue_here::resume_refusal`.
pub(crate) const WEBRTC_HELD_BY_OTHER: &str = "Another device is streaming over WebRTC";
pub(crate) const RESUME_GAME_CHANGED: &str = "The game running on this PC changed";
pub(crate) const NOTHING_TO_RESUME: &str = "no application or remote monitor to resume";
/// Whether `device` started or took over the running game, also a background one.
pub(super) fn plays_game(h: &Shared, device: &str) -> bool {
    h.current_app
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|game| device_uuid(&game.owner) == device_uuid(device))
}
pub(super) fn resolve_launch_app(
    h: &Shared,
    args: &Args,
    client: &Client,
    requested: u32,
    control: Option<Control>,
    owner: remote::Owner,
    resume: bool,
) -> Result<(Role, Option<App>, u32, Option<crate::nucleus::CoopApp>)> {
    let mut role = match control {
        Some(Control::Monitor) => Role::RemoteMonitor,
        Some(Control::Input) => Role::InputOnly,
        _ => Role::Stream,
    };
    if args.get("input_only").is_some_and(|v| v == "1") {
        role = Role::InputOnly;
    }
    if args.get("remote_monitor").is_some_and(|v| v == "1") {
        role = Role::RemoteMonitor;
    }
    // A microphone-only session sends no input, so the administrator's
    // input-only switch does not apply; it needs the launch permission alone.
    let mic_only = mic_only_requested(args);
    if mic_only && role != Role::InputOnly {
        bail!("a microphone-only session must be input-only");
    }
    if role == Role::InputOnly
        && !mic_only
        && !h
            .config
            .read()
            .unwrap()
            .boolean("enable_input_only_mode", false)
    {
        bail!("remote input is disabled by the administrator");
    }
    if mic_only {
        // Whatever it names, it starts no app. It is a launch, never a resume
        // or a tile, so a device that may only view streams cannot start one.
        ensure!(
            !resume && control.is_none(),
            "a microphone-only session is a launch, not a resume or a remote session action"
        );
        ensure!(
            client.allows(butterpollo_core::state::LAUNCH_APPS),
            "a microphone-only session needs the launch permission"
        );
        return Ok((role, None, requested, None));
    }
    // A Remote Monitor adds a display and Remote Input takes the PC's input: each is a session
    // of its own, whichever endpoint or tile carries the request, so a device that may only
    // view streams starts neither. A resume of the monitor a device has is decided below.
    ensure!(
        role == Role::Stream || client.allows(butterpollo_core::state::LAUNCH_APPS),
        "a remote monitor or remote input session needs the launch permission"
    );
    let app_id = if resume {
        let game = h.current_app.lock().unwrap().as_ref().map(|a| a.id);
        if owner == remote::Owner::Monitor {
            role = Role::RemoteMonitor;
            2147483505
        } else if let Some(id) = game {
            id
        } else if h.monitors.lock().unwrap().contains_key(&client.uuid) {
            role = Role::RemoteMonitor;
            2147483505
        } else {
            bail!(NOTHING_TO_RESUME);
        }
    } else {
        requested
    };
    let app = h
        .apps
        .read()
        .unwrap()
        .iter()
        .find(|a| {
            a.id() == app_id
                || a.aliases.contains(&app_id)
                // A resume is of the running game, whatever the client still calls it.
                || (!resume
                    && args.get("appuuid").is_some_and(|id| {
                        a.extra.get("uuid").and_then(serde_json::Value::as_str) == Some(id)
                    }))
        })
        .cloned();
    // A Resume or Running Game tile names only a way to reach the game, and a device's own game
    // is its own whatever it calls it. Anyone else who names another game asked before it changed.
    if resume
        && control.is_none()
        && role == Role::Stream
        && !plays_game(h, &client.uuid)
        && let (Some(running), Some(named)) = (&app, args.get("appuuid"))
        && !named.is_empty()
        && running
            .extra
            .get("uuid")
            .and_then(serde_json::Value::as_str)
            != Some(named.as_str())
    {
        return Err(LaunchFailure(409, RESUME_GAME_CHANGED).into());
    }
    // The co-op entries come after the apps: an app keeps its id.
    let coop = if role == Role::Stream && app.is_none() && !resume {
        let apps = h.apps.read().unwrap().clone();
        let configured = crate::nucleus::configured(&h.config.read().unwrap());
        h.nucleus.resolve(&apps, &configured, app_id)
    } else {
        None
    };
    if let Some(coop) = coop {
        let app_id = coop.id();
        return Ok((role, Some(coop.app.clone()), app_id, Some(coop)));
    }
    if role == Role::Stream && app.is_none() {
        bail!(APPLICATION_NOT_FOUND);
    }
    let app_id = app.as_ref().map_or(app_id, |a| a.id());
    Ok((role, app, app_id, None))
}
pub(super) fn build_launch_session(
    connection: &Connection,
    args: &Args,
    client: Client,
    app_id: u32,
    role: Role,
    key: [u8; 16],
    key_id: u32,
) -> Launch {
    build_launch(
        connection.peer.ip(),
        args,
        client,
        app_id,
        role,
        key,
        key_id,
    )
}
/// `rlMicOnly=1` on /launch ([`butterpollo_core::mic::ONLY_OPTION`]).
fn mic_only_requested(args: &Args) -> bool {
    args.get(butterpollo_core::mic::ONLY_OPTION)
        .is_some_and(|v| v == "1")
}
pub(super) fn build_launch(
    peer: std::net::IpAddr,
    args: &Args,
    mut client: Client,
    app_id: u32,
    role: Role,
    key: [u8; 16],
    key_id: u32,
) -> Launch {
    if role == Role::InputOnly && mic_only_requested(args) {
        // Its input is refused like a device's without the permission.
        client.perm &= butterpollo_core::mic::ONLY_PERMISSION;
    }
    Launch {
        id: uuid::Uuid::new_v4().to_string(),
        client,
        peer,
        app_id,
        background: None,
        key,
        key_id,
        ping: hex::encode(crypto::random::<8>()),
        connect_data: rand::random(),
        role,
        created: Instant::now(),
        rtsp_encrypted: args
            .get("corever")
            .is_some_and(|v| v.parse::<u32>().unwrap_or(0) >= 1),
        rtsp_counter: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(1)),
        rtsp_received: Default::default(),
        preparation: Default::default(),
        vrr_requested: args.get("vrr").is_some_and(|s| s == "1")
            || args.get("client_vrr").is_some_and(|s| s == "1")
            || args.get("clientVrrRequested").is_some_and(|s| s == "1"),
        host_audio: args.get("localAudioPlayMode").is_some_and(|s| s == "1"),
        requested_rate: args
            .get("mode")
            .and_then(|mode| mode.rsplit('x').next())
            .and_then(|rate| {
                if rate.contains('.') {
                    butterpollo_core::framegen::Rate::parse(rate).ok()
                } else {
                    rate.parse()
                        .ok()
                        .map(butterpollo_core::framegen::Rate::from_client)
                }
            })
            .map_or(0, |rate| rate.0),
        audio_preparation: Default::default(),
        // A guest's id is the host's to set (`multiclient`), never the client's.
        options: args
            .iter()
            .filter(|(k, _)| k.as_str() != butterpollo_core::multiclient::LAUNCH_OPTION)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        preparing: Default::default(),
        transport: Default::default(),
        roaming_offered: Default::default(),
        claimed_until: None,
        nucleus: None,
        held: None,
        warnings: Default::default(),
    }
}
pub(super) fn replace_launch_app(
    h: &Shared,
    launch: &Launch,
    resume: bool,
    owner: remote::Owner,
) -> Result<()> {
    let mut current = h.current_app.lock().unwrap();
    if launch.role == Role::Stream
        && !resume
        && current.as_ref().is_some_and(|a| a.id != launch.app_id)
    {
        if owner != remote::Owner::None {
            return Err(LaunchFailure(409, "Remote Input or Remote Monitor is active; launch Terminate before starting a different app").into());
        }
        let generation = &current.as_ref().unwrap().generation;
        if h.config
            .read()
            .unwrap()
            .boolean("remote_monitor_confirm_app_replacement", true)
            && !replacement_confirmed(h, launch, generation)
        {
            return Err(LaunchFailure(410, "An app is already running. Launch this app again within 60 seconds to confirm that you want to close it.").into());
        }
        h.confirmations
            .lock()
            .unwrap()
            .clear(&launch.client.uuid, remote::Confirmation::Replace);
        let (canceled, stopped) = {
            let mut sessions = h.sessions.lock().unwrap();
            let canceled = sessions.stop_role(Role::Stream, None);
            let stopped: Vec<_> = sessions
                .active
                .values()
                .filter(|s| s.launch.role == Role::Stream)
                .map(|s| s.launch.id.clone())
                .chain(sessions.teardown.keys().cloned())
                .collect();
            (canceled, stopped)
        };
        let previous = current.take();
        drop(current);
        drop(canceled);
        // Stopping the app waits for it to exit, never under the lock
        // that every serverinfo and app list request takes.
        h.end_app(previous, AppStop::Replaced(&launch.client.uuid));
        let audio = h.app_audio.lock().unwrap().take();
        let displays = std::mem::take(&mut *h.app_display.lock().unwrap());
        drop((audio, displays));
        if !wait_for_teardown(&h.sessions, &stopped, Duration::from_secs(10)) {
            return Err(LaunchFailure(503, "Another stream operation is still running").into());
        }
        current = h.current_app.lock().unwrap();
    }
    // Preparing displays, audio and the app's own commands can take many
    // seconds; serverinfo and the app list read current_app meanwhile.
    // Launches and stops stay serialized by launch_transition.
    drop(current);
    Ok(())
}
/// Launch parameter of Rubylight clients: `1` when this request is the user's
/// answer to the host's prompt, `0` otherwise.
const CONFIRM_REPLACE: &str = "rlConfirmReplace";
/// Whether `launch` may close the running game. A client that sends
/// [`CONFIRM_REPLACE`] confirms on that request only, so no launch of it
/// redeems a prompt the user cancelled or never saw: a remembered launch, a
/// retry, a frontend starting the game again. Its `1` answers the prompt this
/// host showed it for the game still running; with none pending (another game
/// started meanwhile, or no prompt at all) the host asks again. Any other
/// client keeps Vibepollo's rule: the same launch again within 60 seconds
/// confirms.
fn replacement_confirmed(h: &Shared, launch: &Launch, generation: &str) -> bool {
    let mut book = h.confirmations.lock().unwrap();
    let (client, now) = (&launch.client.uuid, Instant::now());
    match launch.options.get(CONFIRM_REPLACE).map(String::as_str) {
        Some("1")
            if book.answer(
                client,
                remote::Confirmation::Replace,
                generation,
                launch.app_id,
                now,
            ) =>
        {
            true
        }
        Some(_) => {
            book.ask(
                client,
                remote::Confirmation::Replace,
                generation,
                launch.app_id,
                now,
            );
            false
        }
        None => book.confirm(
            client,
            remote::Confirmation::Replace,
            generation,
            launch.app_id,
            now,
        ),
    }
}
/// Refuses what `validate_background_launch` would refuse of `client`'s stream of the running app,
/// before a press of Continue here ends another device's stream for that launch.
pub(crate) fn check_background_mode(h: &Shared, client: &Client, app_id: u32) -> Result<()> {
    let launch = build_launch(
        std::net::Ipv4Addr::LOCALHOST.into(),
        &Args::new(),
        client.clone(),
        app_id,
        Role::Stream,
        [0; 16],
        0,
    );
    validate_background_launch(h, &launch).map(|_| ())
}
pub(super) fn validate_background_launch(h: &Shared, launch: &Launch) -> Result<bool> {
    if launch.role != Role::Stream {
        return Ok(false);
    }
    let background =
        butterpollo_core::background::enabled(&crate::stream::effective_config(h, launch)?);
    let current = h.current_app.lock().unwrap();
    if let Some(app) = current.as_ref().filter(|app| app.id == launch.app_id) {
        if app.background != background {
            return Err(LaunchFailure(
                409,
                "End the current game before changing background streaming mode",
            )
            .into());
        }
        // A device's two screens are one owner.
        if background && device_uuid(&app.owner) != device_uuid(&launch.client.uuid) {
            return Err(LaunchFailure(409, "This background game belongs to another device; end it before starting a background game here").into());
        }
    }
    Ok(background)
}
pub(super) fn prepare_launch_display(
    h: &Shared,
    args: &Args,
    launch: &Launch,
) -> Result<crate::display_session::StreamPreparation> {
    // First: a watched display still reads as live to what follows, and a kept
    // display's resume or a co-op player's window prepares no stream of its own.
    let _wake_watch = crate::display_session::hold_off_wake_watch();
    let config = crate::stream::effective_config(h, launch)?;
    let mode = args
        .get("mode")
        .map(String::as_str)
        .unwrap_or(config.get("fallback_mode", "1920x1080x60"));
    let dimensions: Vec<_> = mode.split('x').collect();
    if dimensions.len() != 3 {
        bail!("launch mode must be WIDTHxHEIGHTxFPS");
    }
    let rate = if dimensions[2].contains('.') {
        butterpollo_core::framegen::Rate::parse(dimensions[2])?
    } else {
        butterpollo_core::framegen::Rate::from_client(dimensions[2].parse()?)
    };
    let mut stream = butterpollo_core::rtsp::Negotiated {
        // Encoders need even sizes. Moonlight rounds only the
        // height, and only later; a 2556x1179 phone was refused.
        width: dimensions[0]
            .parse::<u32>()
            .context("invalid launch width")?
            & !1,
        height: dimensions[1]
            .parse::<u32>()
            .context("invalid launch height")?
            & !1,
        fps: rate.rounded(),
        rate_millihz: rate.0,
        hdr: args.get("hdrMode").is_some_and(|v| v == "1"),
        codec: 1,
        vrr_low_latency: launch.vrr_requested,
        ..Default::default()
    };
    butterpollo_core::stream_policy::apply_color(&mut stream, &config);
    stream.validate()?;
    // A co-op player streams its game window: no display is prepared.
    if launch.nucleus.is_some() {
        return crate::nucleus::prepare(h, launch, &stream, &config, crate::nucleus::WINDOW_WAIT);
    }
    if launch.role == Role::Stream {
        // Reuse only this client's own retained display; another
        // client streaming the same app keeps its display. One set up for
        // another display choice or layout is replaced.
        let client_display = crate::display_session::client_display(launch);
        let app = crate::stream::launch_app(h, launch);
        let layout = crate::display_session::launch_display_layout(
            &config,
            launch,
            app.as_ref(),
            client_display,
        );
        let retained = crate::display_session::take_retained_display(
            h,
            launch,
            &config,
            app.as_ref(),
            |lease| lease.matches(&stream) && lease.serves(client_display, layout),
            |display| butterpollo_windows::display::joinable_mode(display).is_some(),
        );
        match retained {
            Ok(lease) => return lease.resume(&h.directory, &config, launch.warnings.clone()),
            // Dropped here, after the lock: releasing restores Windows.
            Err(released) => drop(released),
        }
    }
    crate::display_session::prepare_stream(h, launch, &stream, &config)
}
pub(super) fn prepare_launch_app(
    h: &Shared,
    args: &Args,
    app: Option<&App>,
    launch: &Launch,
    resume: bool,
) -> Result<Option<crate::process::RunningApp>> {
    // A resume never starts a game: one closed while this launch prepared stays closed.
    if resume && launch.role == Role::Stream && h.current_app.lock().unwrap().is_none() {
        bail!(NOTHING_TO_RESUME);
    }
    #[cfg(test)]
    if let Some(fixture) = &h.reconnect_fixture {
        return fixture.prepare_app(h, app, launch);
    }
    let mut background = false;
    if launch.role != Role::InputOnly {
        let config = crate::stream::effective_config(h, launch)?;
        background = butterpollo_core::background::enabled(&config);
        if config.boolean("stream_audio", true)
            && config.get("audio_capture_mode", "system") == "system"
            && !(launch.role == Role::RemoteMonitor
                && config.boolean("remote_monitor_mute_audio", false))
        {
            // 2, 6, 8 or 12 (7.1.4; ANNOUNCE refuses it later unless it is offered).
            let channels = crate::stream::audio_encoder::surround_channels(
                args.get("surroundAudioInfo").map(String::as_str),
            ) as usize;
            match butterpollo_windows::audio_route::Route::acquire(
                &config,
                &h.directory,
                launch.host_audio,
                channels,
            ) {
                Ok(route) => {
                    *launch.audio_preparation.lock().unwrap() =
                        Some(Box::new(std::sync::Arc::new(route)))
                }
                Err(error) => {
                    tracing::warn!(%error, "audio route will be retried after stream negotiation")
                }
            }
        }
    }
    // Prep commands can run for minutes: the app starts without the lock
    // that every serverinfo, app list and asset request takes. Launches
    // and stops stay serialized by launch_transition, so no other app is
    // installed meanwhile.
    if launch.role == Role::Stream && h.current_app.lock().unwrap().is_none() {
        let mut process_args = args.clone();
        process_args.insert("clientName".into(), launch.client.name.clone());
        process_args.insert("clientUuid".into(), launch.client.uuid.clone());
        // The host decides this, never the client's launch query.
        process_args.remove(crate::process::BACKGROUND_ARG);
        if background {
            process_args.insert(crate::process::BACKGROUND_ARG.into(), "1".into());
        }
        // The frame limiter's rate, which the stream's display set up, for the managed game
        // files' double limiter warning.
        process_args.remove(crate::game_files::FRAME_LIMIT_ARG);
        if let Some(rate) = butterpollo_windows::limiter::active_rate() {
            process_args.insert(
                crate::game_files::FRAME_LIMIT_ARG.into(),
                rate.rounded().to_string(),
            );
        }
        return crate::process::launch(h, app.context("application not found")?, &process_args)
            .map(Some);
    }
    Ok(None)
}
pub(super) fn retain_launch_app(
    h: &Shared,
    launch: &Launch,
    prepared_app: Option<crate::process::RunningApp>,
) {
    if let Some(running) = prepared_app {
        let mut current = h.current_app.lock().unwrap();
        if current.is_none() {
            let started = running.started_event();
            *current = Some(running);
            drop(current);
            h.app_started_webhook(started);
        } else {
            drop(current);
            tracing::warn!("another application started meanwhile; this launch's app is stopped");
            drop(running);
        }
    }
    #[cfg(test)]
    if let Some(fixture) = &h.reconnect_fixture {
        fixture.retain_resources();
        return;
    }
    if launch.role == Role::Stream {
        *h.app_audio.lock().unwrap() = launch
            .audio_preparation
            .lock()
            .unwrap()
            .as_ref()
            .map(|route| route.route());
        if let Some(lease) = launch
            .preparation
            .lock()
            .unwrap()
            .as_ref()
            .map(|prepared| prepared.prepared().display.clone())
        {
            crate::display_session::record_game_display(h, launch, &lease);
            h.app_display
                .lock()
                .unwrap()
                .insert(launch.client.uuid.clone(), (lease, None));
        }
    }
}
pub(super) fn launch_response(
    result: Result<(String, String)>,
    requested: u32,
    reply: &str,
    resume: bool,
    driver_ready: bool,
) -> Response {
    match result {
        Ok((key, url)) => xml(
            200,
            &[
                (key.as_str(), "1".into()),
                ("sessionUrl0", url),
                ("VirtualDisplayDriverReady", driver_ready.to_string()),
            ],
            None,
        ),
        Err(error) => {
            tracing::warn!(
                error = %format!("{error:#}"),
                app_id = requested,
                resume,
                "Moonlight session launch failed"
            );
            let status = match error.downcast_ref::<LaunchFailure>() {
                Some(failure) => failure.0,
                None => error
                    .downcast_ref::<crate::nucleus::Refusal>()
                    .map_or(503, |refusal| refusal.status),
            };
            xml(status, &[(reply, "0".into())], Some(format!("{error:#}")))
        }
    }
}
