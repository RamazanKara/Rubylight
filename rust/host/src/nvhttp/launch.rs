//! `/launch` and `/resume`, and the launch of a WebRTC session: the order of a launch, the same for both
//! transports (`launch_back_half`). Its steps are in `launch_steps`.

use super::*;

// Launching prepares displays and audio and runs prep commands, for seconds
// to minutes: on the blocking pool, not on the workers that serve every
// other request.
pub(super) async fn launch(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
    Query(args): Query<Args>,
) -> Response {
    blocking(move || start(h, connection, args, false)).await
}
pub(super) async fn resume(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
    Query(args): Query<Args>,
) -> Response {
    blocking(move || start(h, connection, args, true)).await
}
pub(super) async fn blocking(work: impl FnOnce() -> Response + Send + 'static) -> Response {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
pub(crate) fn check_streaming_session(h: &Shared, phase: &'static str) -> Result<()> {
    #[cfg(test)]
    let result = match &h.reconnect_fixture {
        Some(fixture) => fixture.check_streaming_session(),
        None => ensure_windows_streaming_session(),
    };
    #[cfg(not(test))]
    let result = {
        let _ = h;
        ensure_windows_streaming_session()
    };
    // Success is useful evidence too: launch preparation can take seconds or
    // minutes, so the final accepted-launch timestamp is not the first check.
    tracing::info!(
        phase,
        ready = result.is_ok(),
        "Windows streaming session gate sampled"
    );
    result
}
pub(super) fn start(h: Shared, connection: Connection, args: Args, resume: bool) -> Response {
    let requested = args
        .get("appid")
        .and_then(|id| id.parse::<u32>().ok())
        .unwrap_or(0);
    let active = remote_game(&h);
    let control = remote::identify(requested, args.get("appuuid").map_or("", String::as_str))
        .or_else(|| {
            active
                .as_ref()
                .filter(|game| requested == remote::running_game_id(game.app.id))
                .map(|_| Control::RunningGame)
        });
    // A device streams its second screen under one pairing: the launch carries `rlSecondScreen=1`
    // and the client is the device's second screen (`as_requested`), which every session rule
    // treats as a device of its own, so it neither replaces the first screen's stream nor shares
    // its display. Only a launch of a paired, authorized device gets there.
    let authorized = validate_launch_client(&h, &connection, control, resume);
    let client = match authorized {
        Ok(c) => c,
        Err(error) => {
            tracing::warn!(%error, app_id = requested, resume, "Moonlight launch authorization failed");
            return xml(401, &[], Some(error.to_string()));
        }
    };
    // A device joining another device's game becomes its guest when `guest_join` lets it
    // (`multiclient`), and may wait for the owner's answer: before the launch lock, so other
    // launches, the owner's among them, go on meanwhile. Otherwise it streams as it always did.
    let joins_game = resume
        || matches!(control, Some(Control::Resume | Control::RunningGame))
        || (control.is_none()
            && requested != 0
            && active.as_ref().is_some_and(|game| game.app.id == requested));
    let plain = matches!(control, None | Some(Control::Resume | Control::RunningGame))
        && !args.get("input_only").is_some_and(|v| v == "1")
        && !args.get("remote_monitor").is_some_and(|v| v == "1");
    let guest = match h.device_guests.gate(
        &h,
        &crate::multiclient::Join {
            client: &client,
            joins_game,
            plain,
        },
    ) {
        Ok(crate::multiclient::Gate::Guest { id, .. }) => Some(id),
        Ok(crate::multiclient::Gate::NotGuest) => None,
        Err(refusal) => {
            return launch_response(
                Err(LaunchFailure(refusal.status(), refusal.message()).into()),
                requested,
                if resume { "resume" } else { "gamesession" },
                resume,
                false,
            );
        }
    };
    let _transition = h.launch_transition.lock().unwrap();
    // Whatever refuses this launch below, a press of Continue here it was for ends with it.
    let attempt = crate::continue_here::Attempt::new(
        &h,
        &client.uuid,
        matches!(control, None | Some(Control::Resume | Control::RunningGame))
            && !args.get("input_only").is_some_and(|v| v == "1")
            && !args.get("remote_monitor").is_some_and(|v| v == "1"),
    );
    // A new stream: the last one's failure no longer describes it.
    h.clear_failure(&client.uuid);
    // The reply names the endpoint Moonlight called. Android and iOS read
    // <gamesession> from /launch and fail without it, also when the launch
    // joins the running game.
    let reply = if resume { "resume" } else { "gamesession" };
    if control != Some(Control::Terminate)
        && let Err(error) = check_streaming_session(
            &h,
            if resume {
                "resume_request"
            } else {
                "launch_request"
            },
        )
    {
        return launch_response(Err(error), requested, reply, resume, false);
    }
    h.request_codec_probe();
    let owner = match validate_launch_request(&h, &client, control) {
        ControlFlow::Continue(owner) => owner,
        ControlFlow::Break(response) => return response,
    };
    // A guest streams the running game, also when it named the game in a `/launch`.
    let resume = resume
        || matches!(control, Some(Control::Resume | Control::RunningGame))
        || (control == Some(Control::Monitor) && owner == remote::Owner::Monitor)
        || guest.is_some();
    let result = (|| -> Result<(String, String)> {
        let key: [u8; 16] = hex::decode(args.get("rikey").context("missing stream key")?)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid stream key length"))?;
        let key_id = stream_key_id(args.get("rikeyid").context("missing stream key ID")?)?;
        let (role, app, app_id, coop) =
            resolve_launch_app(&h, &args, &client, requested, control, owner, resume)?;
        if role != Role::InputOnly {
            h.wait_for_startup_codecs_blocking(STARTUP_CODEC_WAIT);
        }
        if role != Role::InputOnly
            && h.codecs.load(std::sync::atomic::Ordering::Acquire) & 0x0783_0301 == 0
        {
            return Err(no_encoder(&h, &client.uuid));
        }
        let mut launch =
            build_launch_session(&connection, &args, client, app_id, role, key, key_id);
        if let Some(id) = guest {
            launch.options.insert(
                butterpollo_core::multiclient::LAUNCH_OPTION.into(),
                id.to_string(),
            );
        }
        // A refused `rlFecPercentage` fails the launch rather than streaming
        // with protection the client did not ask for.
        butterpollo_core::extensions::fec_choice(&launch.options)?;
        // So does a refused `rlRateChange`, rather than a stream without 0x553C.
        butterpollo_core::rate_change::launch_choice(&launch.options)?;
        // And a display scale Windows does not offer (`rlDisplayScale`).
        butterpollo_core::display_policy::client_display_scale(&launch.options)?;
        // A co-op launch is no background stream; a background stream runs
        // in a Nucleus Co-op session of its own ([`launch_back_half`]).
        launch.background = Some(coop.is_none() && validate_background_launch(&h, &launch)?);
        // Beside another device's game a Moonlight stream never ends what that device streams,
        // whether it joins by `/resume` or by a `/launch` of the Resume or Running Game tile.
        if resume
            && launch.plain_stream()
            && !plays_game(&h, &launch.client.uuid)
            && h.sessions
                .lock()
                .unwrap()
                .plain_webrtc_held_by_other(&launch.client.uuid)
        {
            return Err(LaunchFailure(409, WEBRTC_HELD_BY_OTHER).into());
        }
        let rtsp_port = h.config.read().unwrap().ports()?.rtsp;
        if !launch.rtsp_encrypted
            && crate::network::encryption_mode(&h.config.read().unwrap(), connection.peer) == 2
        {
            let (network, setting) =
                if crate::network::peer_reach(connection.peer) <= crate::network::Reach::Lan {
                    ("the local network", "Local network encryption")
                } else {
                    ("the internet", "Internet encryption")
                };
            bail!(
                "this PC requires encrypted streams on {network} and this device cannot encrypt: update its Moonlight app, or set {setting} to When the device supports it"
            );
        }
        let scheme = if launch.rtsp_encrypted {
            "rtspenc"
        } else {
            "rtsp"
        };
        let launch = launch_back_half(&h, &args, launch, app, coop, resume, owner)?;
        if let Some(id) = guest {
            h.device_guests.joined(id, &launch);
        }
        let host = match connection.local.ip() {
            std::net::IpAddr::V4(ip) => ip.to_string(),
            std::net::IpAddr::V6(ip) => format!("[{ip}]"),
        };
        Ok((reply.into(), format!("{scheme}://{host}:{rtsp_port}")))
    })();
    if result.is_ok() {
        attempt.succeeded();
    }
    let driver_ready = result.is_ok() && butterpollo_windows::display::virtual_display_available();
    launch_response(result, requested, reply, resume, driver_ready)
}
/// A background stream leaves the PC to whoever uses it, so its game is heard
/// only on the stream, whatever the device's "play audio on the PC" says: that
/// setting stays for its normal streams (#56). A co-op player's choice stands.
pub(super) fn keep_background_sound_off_the_pc(launch: &mut Launch) {
    if launch.background == Some(true) && launch.host_audio {
        tracing::info!(client = %launch.client.name, "background stream: the game's sound stays off the PC, whatever the device asks");
        launch.host_audio = false;
    }
}

/// What a launch does once its client is known and the request is parsed, the same for a
/// Moonlight and a WebRTC launch: end the sessions it replaces, wait for their resources, begin
/// the session state, queue the launch, prepare the display, controllers and app, and keep the
/// app. The caller holds `launch_transition`. The launch stays queued until its transport starts it.
/// A co-op launch (`coop`) takes a slot of Nucleus Co-op's session instead, and a background
/// launch (`Launch::background`) a private session of its app: the game it streams is
/// Nucleus's, so the host's own app is neither replaced, started nor kept.
#[allow(
    clippy::too_many_arguments,
    reason = "the launch's parts, as both transports parse them"
)]
fn launch_back_half(
    h: &Shared,
    args: &Args,
    mut launch: Launch,
    app: Option<App>,
    coop: Option<crate::nucleus::CoopApp>,
    resume: bool,
    owner: remote::Owner,
) -> Result<Launch> {
    // The app as this device streams it: the device's block in its `device-overrides` on top,
    // for its Radeon profile and its managed game files.
    let app = app.map(|app| {
        butterpollo_core::device_overrides::for_device(
            &app,
            butterpollo_core::state::device_uuid(&launch.client.uuid),
        )
    });
    let role = launch.role;
    let app_id = launch.app_id;
    let id = launch.id.clone();
    launch.options.remove(crate::display_session::RESUMES_GAME);
    if resume {
        launch
            .options
            .insert(crate::display_session::RESUMES_GAME.into(), "1".into());
    }
    // A stream of Nucleus Co-op's (a co-op or a background launch) streams a
    // game window of its own: it neither ends the other transport's streams
    // nor takes the host's session from whoever owns it.
    let in_nucleus = coop.is_some() || launch.background == Some(true);
    keep_background_sound_off_the_pc(&mut launch);
    // Stopping streams can still own displays shared with this launch.
    // Read both maps under one lock so a worker moving into teardown
    // cannot disappear from the wait before releasing its resources.
    let (canceled, stopping) = {
        let mut sessions = h.sessions.lock().unwrap();
        let mut canceled = sessions.supersede(&launch.client.uuid, role);
        // A Moonlight stream and a WebRTC stream never run together: the newer one ends the older.
        if !in_nucleus {
            canceled.extend(sessions.supersede_other_transport(role, launch.transport));
        }
        let stopping: Vec<_> = sessions
            .active
            .values()
            .filter(|s| s.stopping())
            .map(|s| s.launch.id.clone())
            .chain(sessions.teardown.keys().cloned())
            .collect();
        (canceled, stopping)
    };
    drop(canceled);
    if !stopping.is_empty() {
        tracing::info!(client = %launch.client.name, "waiting for previous streams to release their resources");
        if !wait_for_teardown(&h.sessions, &stopping, Duration::from_secs(5)) {
            return Err(LaunchFailure(503, "Another stream operation is still running").into());
        }
    }
    // The state machine follows stream launches; a failure before the
    // reply returns it to what it was.
    let mut session = (role == Role::Stream && !in_nucleus).then(|| {
        h.moves.forget(&launch.client.uuid);
        h.session_begin(&launch.client.uuid, app.as_ref())
    });
    let game = match (&coop, launch.background == Some(true)) {
        (Some(coop), _) => Some(crate::nucleus::Game::Coop(coop)),
        // A background stream runs only in Nucleus Co-op: an app it can't
        // run is refused, never streamed from the PC's displays.
        (None, true) => Some(crate::nucleus::Game::Background(
            app.as_ref().context(APPLICATION_NOT_FOUND)?,
        )),
        (None, false) => None,
    };
    // The game a Nucleus stream's notice on the PC names (`stream_notice`).
    let announced = match &game {
        Some(crate::nucleus::Game::Coop(coop)) => Some(coop.app.name.clone()),
        Some(crate::nucleus::Game::Background(app)) => Some(app.name.clone()),
        None => None,
    };
    match game {
        Some(game) => {
            let config = crate::stream::effective_config(h, &launch)?;
            let on_pc = crate::stream::plays_on_pc(&config, launch.host_audio);
            let instances = crate::stream::instance_routes(h, &config, on_pc);
            let seat = h.nucleus.admit(crate::nucleus::Admission {
                launch: &id,
                client: &launch.client,
                game,
                size: launch_size(args, &config)?,
                config: &config,
                warnings: launch.warnings.clone(),
                release_parked: &|| release_parked_pads(h),
                instances,
            })?;
            tracing::info!(client = %launch.client.name, session = seat.session, slot = seat.slot, "Nucleus Co-op seat taken");
            // A slot another player held, or whose instance started before
            // its player was known, follows this player's choice.
            if let Some(message) = h.nucleus.plays_on_pc(seat, on_pc) {
                launch.warnings.set("audio_game_pc", message);
            }
            launch.nucleus = Some(seat);
            // Every copy holds the slot: a failure here, the launch's expiry
            // or replacement, or the end of its stream gives it back.
            launch.held = Some(std::sync::Arc::new(crate::nucleus::Held::new(h, seat, &id)));
        }
        None => replace_launch_app(h, &launch, resume, owner)?,
    }
    // Preparing can outlast the pending launch's 30 s (prep commands may
    // run for minutes); it expires only from the reply on, on every path.
    struct Preparing(std::sync::Arc<std::sync::atomic::AtomicBool>);
    impl Drop for Preparing {
        fn drop(&mut self) {
            self.0.store(false, std::sync::atomic::Ordering::Release);
        }
    }
    launch
        .preparing
        .store(true, std::sync::atomic::Ordering::Release);
    let preparing = Preparing(launch.preparing.clone());
    let canceled = h.sessions.lock().unwrap().queue(launch.clone())?;
    drop(canceled);
    if role != Role::InputOnly {
        #[cfg(test)]
        if let Some(fixture) = &h.reconnect_fixture {
            fixture.prepare(&launch);
        }
        if launch.preparation.lock().unwrap().is_none() {
            let prepared = prepare_launch_display(h, args, &launch);
            match prepared {
                Ok(prepared) => *launch.preparation.lock().unwrap() = Some(Box::new(prepared)),
                Err(error) => {
                    let canceled = h.sessions.lock().unwrap().pending.remove(&id);
                    drop(canceled);
                    return Err(error);
                }
            }
        }
    }
    // A co-op player's controller drives its slot's pad, never one of its own; a view-only
    // guest has none on the PC.
    if let Some(value) = args.get("rlControllers")
        && launch.nucleus.is_none()
        && h.device_guests.gamepads_allowed(&launch)
        && let Err(error) = crate::controller_sessions::prepare(h, &launch, value)
    {
        let canceled = h.sessions.lock().unwrap().pending.remove(&id);
        drop(canceled);
        return Err(error);
    }
    if let Some(session) = &session {
        session.app_starting();
    }
    let prepared_app = if launch.nucleus.is_some() {
        // Nucleus Co-op started the game when it admitted the launch.
        Ok(None)
    } else {
        prepare_launch_app(h, args, app.as_ref(), &launch, resume)
    };
    let prepared_app = match prepared_app {
        Ok(prepared_app) => prepared_app,
        Err(error) => {
            // Any failure here, including a config or app lookup error, must
            // release the pending slot now: the client got an error and will
            // never connect, so waiting for expiry would count it for 30 s.
            let canceled = h.sessions.lock().unwrap().pending.remove(&id);
            drop(canceled);
            return Err(error);
        }
    };
    if let Err(error) = check_streaming_session(
        h,
        if resume {
            "resume_prepared"
        } else {
            "launch_prepared"
        },
    ) {
        let canceled = h.sessions.lock().unwrap().pending.remove(&id);
        // A newly started app still belongs to this attempt. Its RAII
        // teardown stops it and undoes prep commands, outside host locks.
        // An existing/resumed app has never been moved out of current_app.
        drop(prepared_app);
        drop(canceled);
        return Err(error);
    }
    // The client has 30 s to connect from now on.
    match h.sessions.lock().unwrap().pending.get_mut(&id) {
        Some(pending) => pending.created = Instant::now(),
        None => bail!("the launch was replaced by a newer one from this device"),
    }
    if launch.nucleus.is_none() {
        retain_launch_app(h, &launch, prepared_app);
    }
    drop(preparing);
    let mode = args.get("mode").map_or("", String::as_str);
    let hdr = args.get("hdrMode").is_some_and(|v| v == "1");
    if launch.transport == Transport::WebRtc {
        tracing::info!(client = %launch.client.name, app_id, role = ?role, mode, hdr, resume, "WebRTC session launched");
    } else {
        tracing::info!(client = %launch.client.name, app_id, role = ?role, mode, hdr, resume, "Moonlight session launched");
    }
    // Everything the client asked for, without the stream's AES key.
    let parameters = args
        .iter()
        .filter(|(name, _)| !matches!(name.as_str(), "rikey" | "rikeyid"))
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    tracing::info!(client = %launch.client.name, resume, ?parameters, "Moonlight launch parameters");
    if let Some(session) = &mut session {
        session.succeed();
    }
    if role == Role::Stream {
        // Off the launch path: the profile's settings are written on a thread of their own.
        crate::radeon_profile::launched(
            h,
            &launch.client.uuid,
            app.as_ref(),
            args.get("mode")
                .and_then(|mode| crate::radeon_profile::stream_fps(mode)),
        );
    }
    if let Some(app) = &announced {
        crate::stream_notice::started(
            &launch.client.uuid,
            &launch.client.name,
            app,
            launch.background == Some(true),
            launch.nucleus,
        );
    }
    Ok(launch)
}
/// The width and height a launch asks for (`mode`, else `fallback_mode`),
/// even like the stream's.
fn launch_size(args: &Args, config: &Config) -> Result<(u32, u32)> {
    let mode = args
        .get("mode")
        .map(String::as_str)
        .unwrap_or(config.get("fallback_mode", "1920x1080x60"));
    let mut dimensions = mode.split('x').map(str::parse::<u32>);
    match (dimensions.next(), dimensions.next()) {
        (Some(Ok(width)), Some(Ok(height))) => Ok((width & !1, height & !1)),
        _ => bail!("launch mode must be WIDTHxHEIGHTxFPS"),
    }
}
/// What a WebRTC start-session asks the launch for. The settings are the validated ones.
pub(crate) struct WebRtcLaunch {
    pub client: Client,
    pub peer: std::net::IpAddr,
    /// The browser runs on this PC ([`crate::network::browser_on_this_pc`]).
    pub this_pc: bool,
    pub app_id: u32,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub hdr: bool,
}
/// Why a WebRTC launch did not happen.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WebRtcLaunchError {
    InvalidApplication,
    /// Another app, or another device's WebRTC stream, holds the host.
    SessionActive(String),
    /// A stream operation of another session has not finished.
    HostBusy,
    Failed(String),
}
/// The launch of a WebRTC session: the same preparation as a Moonlight launch, for the device
/// that signalled start-session. The returned launch is queued; `Sessions::start` takes it over
/// once the peer connection is up. Waits for displays and apps, so never on the async runtime.
pub(crate) fn launch_webrtc(
    h: &Shared,
    request: WebRtcLaunch,
) -> std::result::Result<Launch, WebRtcLaunchError> {
    let _transition = h.launch_transition.lock().unwrap();
    let attempt = crate::continue_here::Attempt::new(h, &request.client.uuid, true);
    let outcome = (|| -> Result<Launch> {
        check_streaming_session(h, "launch_request")?;
        h.request_codec_probe();
        if crate::updater::installing(h) {
            return Err(LaunchFailure(
                503,
                "Rubylight is installing an update. Reconnect shortly.",
            )
            .into());
        }
        let args = Args::from([
            ("appid".to_owned(), request.app_id.to_string()),
            (
                "mode".to_owned(),
                format!("{}x{}x{}", request.width, request.height, request.fps),
            ),
            (
                "hdrMode".to_owned(),
                if request.hdr { "1" } else { "0" }.to_owned(),
            ),
        ]);
        let owner = remote_owner(h, &request.client.uuid);
        let (role, app, app_id, coop) = resolve_launch_app(
            h,
            &args,
            &request.client,
            request.app_id,
            None,
            owner,
            false,
        )
        .map_err(launch_app_error)?;
        // A co-op game runs beside the host's app, as on a Moonlight launch.
        if coop.is_none()
            && h.current_app
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|running| running.id != app_id)
        {
            return Err(LaunchFailure(
                409,
                "Another app is running; switch to this one or stop the host session first",
            )
            .into());
        }
        if h.sessions.lock().unwrap().held_by_other(
            Role::Stream,
            Transport::WebRtc,
            &request.client.uuid,
        ) {
            return Err(LaunchFailure(409, "Another device is streaming over WebRTC").into());
        }
        h.wait_for_startup_codecs_blocking(STARTUP_CODEC_WAIT);
        if h.codecs.load(std::sync::atomic::Ordering::Acquire) & 0x0783_0301 == 0 {
            return Err(no_encoder(h, &request.client.uuid));
        }
        // The stream key belongs to GameStream's RTSP; nothing on this launch uses it.
        let mut launch = build_launch(
            request.peer,
            &args,
            request.client,
            app_id,
            role,
            crypto::random::<16>(),
            rand::random(),
        );
        launch.transport = Transport::WebRtc;
        if request.this_pc {
            launch.options.insert(
                crate::display_session::BROWSER_ON_THIS_PC.into(),
                "1".into(),
            );
        }
        launch.background = Some(coop.is_none() && validate_background_launch(h, &launch)?);
        launch_back_half(h, &args, launch, app, coop, false, owner)
    })();
    if outcome.is_ok() {
        attempt.succeeded();
    }
    outcome.map_err(|error| match error.downcast_ref::<LaunchFailure>() {
        Some(LaunchFailure(404, _)) => WebRtcLaunchError::InvalidApplication,
        Some(LaunchFailure(409, message)) => WebRtcLaunchError::SessionActive((*message).into()),
        Some(LaunchFailure(503, message))
            if *message != butterpollo_core::encoder_gpu::NO_ENCODER_LAUNCH =>
        {
            WebRtcLaunchError::HostBusy
        }
        _ => match error.downcast_ref::<crate::nucleus::Refusal>() {
            // Busy or refused by the session; a missing handler or an
            // unavailable Nucleus keeps its own text.
            Some(refusal) if refusal.status == 409 => {
                WebRtcLaunchError::SessionActive(refusal.message.clone())
            }
            _ => WebRtcLaunchError::Failed(format!("{error:#}")),
        },
    })
}
/// The launch refused for want of an encoder, with the encoder check's own cause
/// first (the device's `/serverinfo` keeps it too), then what to do.
fn no_encoder(h: &Shared, uuid: &str) -> anyhow::Error {
    let refused = anyhow::Error::new(LaunchFailure(
        503,
        butterpollo_core::encoder_gpu::NO_ENCODER_LAUNCH,
    ));
    let Some(cause) = h.encoder_probe_error.lock().unwrap().clone() else {
        h.record_launch_failure(uuid, butterpollo_core::encoder_gpu::NO_ENCODER_LAUNCH);
        return refused;
    };
    let cause = format!("The encoder check failed: {cause}");
    h.record_launch_failure(uuid, &cause);
    refused.context(cause)
}
/// A failed app lookup keeps its own status: only an unknown app is a 404.
pub(super) fn launch_app_error(error: anyhow::Error) -> anyhow::Error {
    if error.downcast_ref::<LaunchFailure>().is_none() && error.to_string() == APPLICATION_NOT_FOUND
    {
        LaunchFailure(404, APPLICATION_NOT_FOUND).into()
    } else {
        error
    }
}
