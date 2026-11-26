//! Async Tokio host server lifecycle, logging, panic hook, and graceful shutdown.
use crate::service_tasks;
use anyhow::{Context, Result};
use butterpollo_stream::{
    child_session, dsu, game_files, game_settings, host_warnings, internet_play, invites, library,
    logging, multiclient, network, nvhttp, nvidia_component, radeon_session, remote_display,
    rtsp_server, runtime, session_end, state, steamgriddb, stream, tls,
    webrtc_bridge::WebRtcWebService, win_bridge,
};
use butterpollo_web::{native_api, web};
use butterpollo_webrtc::WebRtcRuntime;
use std::{
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

fn log_encoder_gpu_check() {
    use butterpollo_media_policy::encoder_gpu::{Refusal, Runtimes, check};
    let adapters = butterpollo_win_graphics::capture::gpus()
        .map(|gpus| butterpollo_win_graphics::capture::gpu_adapters(&gpus))
        .unwrap_or_default();
    match check(&adapters, Runtimes::ALL) {
        Ok(()) => {}
        // No driver may only mean the GPU is not up yet, as at boot.
        Err(refusal @ Refusal::NoGpu) => {
            tracing::warn!(%refusal, "no graphics adapter yet; streams wait for an AMD or NVIDIA GPU");
        }
        Err(refusal) => tracing::error!(%refusal, "this PC cannot stream; no AMD or NVIDIA GPU"),
    }
}

#[derive(Clone)]
struct SharedLogFile(std::sync::Arc<std::sync::Mutex<butterpollo_session::logfile::RotatingFile>>);

impl SharedLogFile {
    fn new(file: butterpollo_session::logfile::RotatingFile) -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(file)))
    }
}

impl std::io::Write for SharedLogFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .flush()
    }
}

fn utc_timestamp(now: std::time::SystemTime) -> String {
    let dt = time::OffsetDateTime::from(now);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}Z",
        dt.year(),
        u8::from(dt.month()),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second(),
        dt.microsecond(),
    )
}

fn panic_payload(info: &std::panic::PanicHookInfo<'_>) -> String {
    if let Some(s) = info.payload().downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = info.payload().downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

fn format_panic_line(
    timestamp: &str,
    thread: &str,
    location: Option<&str>,
    message: &str,
) -> String {
    let location = location.unwrap_or("unknown");
    format!(
        "{timestamp} ERROR butterpollo: host panicked thread={thread:?} location={location:?} message={message:?}\n"
    )
}

fn write_panic_line(log: &SharedLogFile, line: &str) -> bool {
    let deadline = Instant::now() + Duration::from_millis(50);
    loop {
        match log.0.try_lock() {
            Ok(mut file) => {
                use std::io::Write;
                return file.write_all(line.as_bytes()).is_ok() && file.flush().is_ok();
            }
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                use std::io::Write;
                let mut file = poisoned.into_inner();
                return file.write_all(line.as_bytes()).is_ok() && file.flush().is_ok();
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

/// Logs panics directly to `butterpollo.log` and flushes before the process aborts
/// (`panic = "abort"` in release) or the crash minidump helper runs.
fn install_panic_hook(log: SharedLogFile) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        thread_local! {
            static IN_HOOK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        }
        if !IN_HOOK.replace(true) {
            let thread = std::thread::current()
                .name()
                .unwrap_or("unnamed")
                .to_owned();
            let location = info
                .location()
                .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()));
            let message = panic_payload(info);
            let timestamp = utc_timestamp(std::time::SystemTime::now());
            let line = format_panic_line(&timestamp, &thread, location.as_deref(), &message);
            if !write_panic_line(&log, &line) {
                tracing::error!(
                    thread = %thread,
                    location = location.as_deref().unwrap_or("unknown"),
                    panic = %message,
                    "host panicked"
                );
            }
            IN_HOOK.set(false);
        }
        previous(info);
    }));
}

enum ExitAction {
    Return,
    Exit(i32),
}

pub(crate) async fn run(args: crate::cli::Args, assets: PathBuf) -> Result<()> {
    let supervised = args.service_stop_source.is_some();
    let stop_signal =
        butterpollo_win_io::process::StopSignal::new(args.service_stop_source.as_deref())?;
    let directory = match &args.config_dir {
        Some(directory) => directory.clone(),
        None => butterpollo_session::paths::portable_profile()?,
    };
    let h = state::Host::load(directory, assets, args.port)?;
    // Crash reporting must not keep the host from starting, e.g. when the
    // reporter process is blocked.
    let crash_reporting = butterpollo_win_io::crash::initialize(&h.directory).err();
    win_bridge::install_hooks();
    let display_recovery = match butterpollo_win_display::display_recovery::initialize_with(
        &h.directory,
        win_bridge::recovery_hooks(),
    ) {
        Ok(outcome) => outcome,
        Err(error) if !error.to_string().contains("another host owns") => {
            tracing::warn!(
                error = format!("{error:#}"),
                "display recovery initialization reported an error; continuing startup"
            );
            Some(error)
        }
        Err(error) => return Err(error),
    };
    let log_path = h
        .config
        .read()
        .unwrap()
        .path("log_path", &h.directory, "logs/butterpollo.log");
    let open = |path: &std::path::Path| -> Result<_> {
        std::fs::create_dir_all(path.parent().context("log directory missing")?)?;
        Ok(butterpollo_session::logfile::RotatingFile::open_default(
            path,
        )?)
    };
    // A log_path that cannot be used (a folder, a missing drive) falls back to
    // the default file instead of keeping the host, and its console, down.
    let default_log = h.directory.join("logs/butterpollo.log");
    let (appender, unusable_log) = match open(&log_path) {
        Ok(appender) => (appender, None),
        Err(error) if log_path != default_log => (open(&default_log)?, Some((log_path, error))),
        Err(error) => return Err(error),
    };
    let log_level = h.config.read().unwrap().log_level();
    let shared_log = SharedLogFile::new(appender);
    let (writer, log_guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .buffered_lines_limit(8192)
        .lossy(true)
        .finish(shared_log.clone());
    use tracing_subscriber::prelude::*;
    // The service host has no console; formatting every event a second time
    // for a stdout nobody reads is wasted work.
    let terminal = std::io::IsTerminal::is_terminal(&std::io::stdout());
    tracing_subscriber::registry()
        .with(logging::layer(log_level))
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false),
        )
        .with(terminal.then(tracing_subscriber::fmt::layer))
        .init();
    install_panic_hook(shared_log);
    let action = match run_logged(
        &args,
        h.clone(),
        supervised,
        stop_signal,
        unusable_log,
        crash_reporting,
        display_recovery,
    )
    .await
    {
        Ok(action) => action,
        Err(error) => {
            tracing::error!(
                reason = h.stop_reason().unwrap_or("startup_or_runtime_error"),
                error = %format!("{error:#}"),
                exit_code = 1u32,
                "host exiting with error"
            );
            drop(log_guard);
            return Err(error);
        }
    };
    drop(log_guard);
    if let ExitAction::Exit(code) = action {
        std::process::exit(code);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_logged(
    args: &crate::cli::Args,
    h: state::Shared,
    supervised: bool,
    stop_signal: butterpollo_win_io::process::StopSignal,
    unusable_log: Option<(PathBuf, anyhow::Error)>,
    crash_reporting: Option<anyhow::Error>,
    display_recovery: Option<anyhow::Error>,
) -> Result<ExitAction> {
    if let Some((path, error)) = unusable_log {
        tracing::warn!(
            path = %path.display(),
            error = format!("{error:#}"),
            "log_path cannot be used; logging to the default file"
        );
    }
    if let Some(error) = crash_reporting {
        tracing::warn!(
            error = format!("{error:#}"),
            "crash reporting unavailable; the host runs without crash reports"
        );
    }
    if let Some(error) = display_recovery {
        tracing::warn!(
            error = format!("{error:#}"),
            "display settings from an interrupted stream were not all restored"
        );
    }
    // Games a host that stopped mid-suspend left frozen run again first.
    crate::suspend_recovery::at_start(&h.directory);
    // Once logging runs: an unreadable file says so.
    butterpollo_win_display::display_arrangement::initialize_remembered(&h.directory);
    if let Err(error) = butterpollo_win_io::audio_app_route::recover(&h.directory) {
        tracing::warn!(
            error = format!("{error:#}"),
            "game audio devices from an interrupted stream were not all restored; retrying"
        );
    }
    if let Some(journal) = h.recover_session() {
        tracing::warn!(
            state = ?journal.state,
            owner = ?journal.owner,
            app = ?journal.app,
            "the previous host stopped during a session; it was cleared"
        );
    }
    if let Err(error) = library::stamp_synced(&h) {
        tracing::error!(error = %format!("{error:#}"), "the synced apps could not be stamped with their sync");
    }
    // NVIDIA's power profile for this executable before any GPU device opens in this process: the
    // driver reads it when the process first loads there, and NVENC at idle clocks encodes slower.
    let nvidia_config = h.config.read().unwrap().clone();
    // The driver panel controls the settings of the GPU the host streams from: Radeon's or
    // NVIDIA's. It is chosen once per host start.
    let panel_driver = tokio::task::spawn_blocking(move || {
        butterpollo_win_graphics::nvidia_profile::apply_configured(&nvidia_config);
        butterpollo_win_graphics::capture::panel_driver(&nvidia_config)
    })
    .await
    .unwrap_or_default();
    // With the NVIDIA component installed, the GPU and driver are asked which image enhancements
    // they offer, after the power profile, off startup's path: serverinfo and the console read
    // the answer. An adapter from an earlier host version is replaced first.
    nvidia_component::start(&h);
    // The first launcher scan for the native library runs on its own thread, so the first request
    // finds it done and startup does not wait.
    library::installed(&h);
    // Radeon settings a crashed run left changed are put back by the Radeon worker thread, which
    // owns every later Radeon write too and queues them behind the recovery. Startup does not
    // wait for it, and with no journal it never touches ADLX. The worker logs failures and retries.
    tracing::info!(
        driver = panel_driver.name(),
        "the driver settings panel controls this GPU maker's driver"
    );
    if let Err(error) = radeon_session::start(&h.radeon, &h.directory, panel_driver) {
        tracing::error!(
            error = %error,
            driver = panel_driver.name(),
            "the driver settings worker could not start; settings from an interrupted session stay in the journal"
        );
    }
    // The restore thread for keys in games' settings files starts, and puts back those a crashed
    // run left changed, as the Windows user once someone is signed in. A stream that writes one
    // of them first puts the PC's own value back itself.
    game_files::start(&h);
    // Game settings files a crashed run left swapped for a device are saved as that device's
    // copies and the PC's own put back, as the Windows user once someone is signed in.
    game_settings::start(&h);
    // Guests from paired devices: their places end with the game or their time.
    multiclient::start(&h);
    // What a Nucleus Co-op session left for the user at the console when an
    // earlier host stopped mid-session is put back before a stream can start
    // a new one.
    service_tasks::restore_nucleus_at_start(
        supervised,
        &h.directory,
        butterpollo_win_io::service::restore_nucleus_layers,
    );
    // The service's keys and backups: SYSTEM and Administrators only, also after a
    // profile was copied in or restored from a backup.
    for folder in ["credentials", butterpollo_session::settings_backup::BACKUPS] {
        let path = h.directory.join(folder);
        if butterpollo_win_io::process::is_system()
            && path.is_dir()
            && let Err(error) = butterpollo_win_io::process::restrict_to_administrators(&path)
        {
            tracing::warn!(%error, folder, "the folder permissions could not be repaired");
        }
    }
    // A SteamGridDB key typed into sunshine.conf, which every local user reads, moves there too.
    steamgriddb::migrate_key(&h);
    let ports = h.config.read().unwrap().ports()?;
    if let Err(error) = butterpollo_win_graphics::vulkan::reconcile(
        h.config.read().unwrap().boolean("vulkan_hdr_layer", true),
    ) {
        tracing::warn!(%error, "Vulkan HDR registration could not be reconciled");
    }
    let bind = network::bind_address(&h.config.read().unwrap(), args.bind)?;
    let discovery = match network::Discovery::start(&h.config.read().unwrap(), bind) {
        Ok(discovery) => discovery,
        Err(error) => {
            tracing::warn!(%error, "Moonlight discovery unavailable");
            None
        }
    };
    // Off unless `enable_dsu_server` is set; a taken port is a console warning.
    let (dsu_port, dsu_lan) = {
        let config = h.config.read().unwrap();
        (config.dsu_server_port(), config.dsu_server_lan())
    };
    dsu::start(dsu_port, dsu_lan, &h.warnings);
    let port_forward = network::port_forward(h.clone(), bind);
    let internet_play = internet_play::start(h.clone(), bind);
    let invite_sweep = invites::start(h.clone());
    let (tray, actions) = if args.no_tray || !h.config.read().unwrap().boolean("system_tray", true)
    {
        (None, None)
    } else {
        let icon = h.assets.parent().unwrap_or(&h.assets).join("apollo.ico");
        let icon = if icon.is_file() {
            icon
        } else {
            h.assets.join("images/apollo.ico")
        };
        match butterpollo_win_io::tray::Tray::new_options(
            icon,
            ports.web,
            h.config
                .read()
                .unwrap()
                .boolean("hide_tray_controls", false),
        ) {
            Ok((tray, events)) => (Some(tray), Some(events)),
            Err(e) => {
                tracing::warn!(error=%e,"tray unavailable");
                (None, None)
            }
        }
    };
    let media = stream::Media::new(h.clone(), bind)?;
    log_encoder_gpu_check();
    h.probe_codecs();
    let webrtc_service: Arc<dyn WebRtcWebService> =
        Arc::new(WebRtcRuntime::install(&h, Some(media.clone())));
    let mut tasks = tokio::task::JoinSet::new();
    for (port, https, web) in [
        (ports.http, false, false),
        (ports.https, true, false),
        (ports.web, true, true),
    ] {
        let router = if web {
            web::router_with(h.clone(), Some(webrtc_service.clone()))
        } else {
            nvhttp::router_with(h.clone(), https, https.then(native_api::router))
        };
        let acceptor = if web {
            Some(tls::console_acceptor(&h.console)?)
        } else if https {
            Some(tls::acceptor(&h.identity, true)?)
        } else {
            None
        };
        let address = (bind, port).into();
        tasks.spawn(tls::serve(address, router, acceptor));
    }
    tasks.spawn(rtsp_server::serve(
        (bind, ports.rtsp).into(),
        h.clone(),
        media.clone(),
    ));
    // The guest page's own port, which serves the play page too: a port that is taken costs the
    // guests and the play page alone, and says so.
    let guest_listener = {
        let address: std::net::SocketAddr = (bind, ports.guest).into();
        let h = h.clone();
        let acceptor = tls::console_acceptor(&h.console)?;
        let router = web::guest_router_with(h.clone(), Some(webrtc_service.clone()));
        tokio::spawn(async move {
            if let Err(error) = tls::serve(address, router, Some(acceptor)).await {
                let error = format!("{error:#}");
                tracing::error!(%error, %address, "the guest page's listener failed");
                h.warnings.set(
                    host_warnings::GUEST_PORT,
                    format!(
                        "Guests can't open their links: TCP port {} could not be opened ({error}).",
                        address.port()
                    ),
                );
            }
        })
    };
    // None when the setting is off or the port is taken; the console says which.
    let webrtc = butterpollo_webrtc::server::start(h.clone(), bind, Some(media.clone()));
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        web_port = ports.web,
        "Butterpollo Rust host started"
    );
    child_session::retire_at_startup(h.directory.clone());
    h.power.log_availability();
    // A block rule, left when Windows' "allow access" prompt was dismissed,
    // overrides the installer's allow rule: say so rather than leave users to
    // open ports by hand. Only a host listening on the network is affected.
    // The console shows it as a host warning; setting it logs it.
    if bind.is_unspecified() {
        let h = h.clone();
        std::thread::spawn(move || {
            let Ok(program) = std::env::current_exe() else {
                return;
            };
            match butterpollo_win_io::firewall::problems(&program) {
                Ok(problems) => match host_warnings::firewall(&problems) {
                    Some(warning) => h.warnings.set(host_warnings::FIREWALL, warning),
                    None => h.warnings.clear(host_warnings::FIREWALL),
                },
                Err(error) => tracing::debug!(%error, "Windows Firewall rules could not be read"),
            }
        });
    }
    // Windows ends the host with the session when it shuts down, before the service hears of
    // it: the session's end runs the shutdown below as the service's stop does. Whether Windows
    // also sends the end-session messages at sign-out to this host, which runs with SYSTEM's
    // token, is unverified; without them a sign-out ends the host as before.
    let session_end = std::sync::Arc::new(session_end::SessionEnd::default());
    let session_end_watch = {
        let session_end = session_end.clone();
        butterpollo_win_io::session_end::SessionEndWatch::start(move |ending| {
            tracing::info!(
                logoff = ending.logoff,
                critical = ending.critical,
                close_app = ending.close_app,
                "Windows is ending the session; the host shuts down and tells its clients"
            );
            if !session_end.ended(session_end::WAIT) {
                tracing::warn!("the clients had not been told when the session's end went on");
            }
        })
    };
    let session_end_watch = session_end_watch
        .inspect_err(|error| {
            tracing::warn!(%error, "the end of the Windows session cannot be heard; clients learn of it by their timeout")
        })
        .ok();
    let mut session_ended = false;
    let stop_source: Option<&'static str>;
    let outcome: Result<()> = tokio::select! {
        signal=tokio::signal::ctrl_c()=>{
            match signal {
                Ok(()) => {
                    tracing::info!("Ctrl+C received; host shutting down");
                    stop_source = Some("ctrl_c");
                    Ok(())
                }
                Err(error) => {
                    tracing::error!(error = %format!("{error:#}"), "Ctrl+C listener failed; host shutting down");
                    stop_source = Some("ctrl_c_error");
                    Err(error.into())
                }
            }
        },
        _=session_end.requested()=>{
            session_ended = true;
            stop_source = Some("windows_session_end");
            Ok(())
        },
        task=tasks.join_next()=>{
            stop_source = Some("listener_failed");
            let error = match task {
                Some(Ok(Err(error))) => error.context("host listener failed"),
                Some(Err(error)) => error.into(),
                _ => anyhow::anyhow!("host listener stopped unexpectedly"),
            };
            tracing::error!(error = %format!("{error:#}"), "host listener stopped; shutting down");
            Err(error)
        },
        _=runtime::maintain(h.clone(), stop_signal, actions, ports.web)=>{
            stop_source = h.stop_reason().or(Some("maintenance_stopped"));
            Ok(())
        }
    };
    // The service ends the host 20 s after it asked it to stop (`shutdown_host`). Every wait
    // below takes at most what is left of SHUTDOWN_WAITS, so the host finishes on its own.
    let shutdown_deadline = Instant::now() + SHUTDOWN_WAITS;
    let left = |cap: Duration| cap.min(shutdown_deadline.saturating_duration_since(Instant::now()));
    // A launch still running its preparation commands stops too, at once: the undo commands of
    // those that finished run while the host shuts down, and it starts no game.
    h.stop_launch_for_shutdown();
    // A game moved to another device whose new stream came since the last tick stays with that
    // device: the move is settled while the stream still counts as arrived (telling
    // the clients below stops their sessions). A launch that holds
    // the lock settles its own.
    if let Ok(_transition) = h.launch_transition.try_lock() {
        h.settle_move();
    }
    // Clients hear the host stop before its sockets close: the control worker sends the USB
    // detaches and the 0x0109 termination through the path a normal stop uses, within what is
    // left of a second. What it could not tell then ends with the process, as before.
    match media.clients_told_of_shutdown(left(Duration::from_secs(1))) {
        Some((clients, elapsed_ms)) => {
            tracing::info!(clients, elapsed_ms, "shutdown: clients told")
        }
        None => tracing::warn!(
            "shutdown: the control worker had not told the clients when its wait ran out"
        ),
    }
    // Forwarded USB devices end with the host too, including any a session without a control
    // peer still held; the clients told above detach theirs already.
    h.usb.disable();
    h.stop.store(true, Ordering::Release);
    // UPnP mappings and IPv6 pinholes come down while the rest shuts down.
    if let Some(port_forward) = &port_forward {
        port_forward.stop();
    }
    // Nothing waits on a monitor's wake on the way out; the next start puts back what is left.
    butterpollo_win_display::display_arrangement::close_wake_watch();
    let canceled = h.sessions.lock().unwrap().request_stop(None);
    drop(canceled);
    // The clients know, and the session's end, if it waits, may go on.
    session_end.clients_told();
    // The Radeon settings and game file keys the sessions changed go back from now on, on
    // their own threads, while the streams and the game close: neither needs the game gone
    // (End session puts them back under a running game too).
    h.radeon.release_all();
    let files = h.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("game file restore at shutdown".into())
        .spawn(move || files.game_files.release_all(&files.directory))
    {
        tracing::error!(%error, "the game file restore could not start early; it runs below");
    }
    let deadline = Instant::now() + left(Duration::from_secs(5));
    while !h.sessions.lock().unwrap().idle() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    drop(webrtc);
    guest_listener.abort();
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    // Tunnelled devices are told the host stops; their streams ended above.
    invite_sweep.abort();
    if let Some(internet_play) = internet_play {
        internet_play.stop();
    }
    // Closing the game waits for it and runs its undo commands: for what is left of the
    // shutdown's time, after which the host goes on and the rest ends with it.
    let stopping = h.clone();
    let stop_deadline = shutdown_deadline - SHUTDOWN_MARGIN;
    if !within(
        left(Duration::from_secs(8)),
        "application stop at shutdown",
        move || {
            stopping.stop_app(state::AppStop::Shutdown {
                deadline: stop_deadline,
            })
        },
    )
    .await
    {
        tracing::warn!("the application had not closed when the host's shutdown time ran out");
    }
    let resetting = h.clone();
    within(
        left(Duration::from_secs(5)),
        "session reset at shutdown",
        move || resetting.session_shutdown(),
    )
    .await;
    // Whatever a session changed goes back before the host exits; the restore thread has had the
    // work since the start of the shutdown. The wait is blocking, so it leaves the async executor.
    // It is shorter than the worker's 150 s limit on one driver call: a restore still stuck
    // then stays journaled, is logged, and the next host start puts it back.
    let radeon = h.clone();
    let wait = left(Duration::from_secs(30));
    if let Err(error) = tokio::task::spawn_blocking(move || radeon.radeon.shutdown(wait)).await {
        tracing::error!(%error, "waiting for the Radeon restore at shutdown failed; the next host start restores what is left");
    }
    // Every game file key goes back too, on its own thread; one it cannot reach in time stays
    // journaled for the next start.
    let files = h.clone();
    let wait = left(Duration::from_secs(10));
    if let Err(error) =
        tokio::task::spawn_blocking(move || files.game_files.shutdown(&files.directory, wait)).await
    {
        tracing::error!(%error, "waiting for the game file restore at shutdown failed; the next host start restores what is left");
    }
    remote_display::disconnect(&h, None);
    drop(discovery);
    if let Some(port_forward) = port_forward {
        port_forward.finish(left(Duration::from_secs(10))).await;
    }
    drop(tray);
    drop(session_end_watch);
    let reason = stop_source.or_else(|| h.stop_reason()).unwrap_or("unknown");
    if let Some(code) = session_end::exit_code(supervised, session_ended) {
        tracing::info!(reason, exit_code = code, "host exiting");
        return Ok(ExitAction::Exit(code as i32));
    }
    if h.restart.load(Ordering::Acquire) {
        let exit_code = if supervised {
            butterpollo_win_io::service::RESTART_EXIT_CODE
        } else {
            0
        };
        tracing::info!(reason, exit_code, supervised, "host exiting for restart");
        if supervised {
            return Ok(ExitAction::Exit(exit_code as i32));
        } else {
            use std::os::windows::process::CommandExt;
            std::process::Command::new(std::env::current_exe()?)
                .args(std::env::args_os().skip(1))
                .creation_flags(0x08000000)
                .spawn()?;
            outcome?;
            return Ok(ExitAction::Return);
        }
    }
    outcome?;
    tracing::info!(reason, exit_code = 0u32, "host exiting");
    Ok(ExitAction::Return)
}
/// What the host's own waits at a service stop take together, at most: under the 20 s after
/// which the service ends it, with room for the steps that do not wait.
const SHUTDOWN_WAITS: Duration = Duration::from_secs(17);
/// Kept free at the end of the shutdown deadline for the audio route to go
/// back and the rest of the stop to finish.
const SHUTDOWN_MARGIN: Duration = Duration::from_secs(1);
/// Runs `work` on a thread of its own and waits for it, at most `wait`. A thread still running
/// then ends with the host, and does not hold up the runtime's shutdown as a blocking task
/// would. True when it finished.
async fn within(wait: Duration, name: &str, work: impl FnOnce() + Send + 'static) -> bool {
    let (done, finished) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            work();
            let _ = done.send(());
        });
    if let Err(error) = spawned {
        tracing::error!(%error, name, "a shutdown step could not start");
        return false;
    }
    tokio::task::spawn_blocking(move || finished.recv_timeout(wait).is_ok())
        .await
        .unwrap_or(false)
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;

    #[tokio::test]
    async fn a_shutdown_step_is_waited_for_only_while_the_shutdown_has_time_left() {
        // The service ends the host 20 s after it asked it to stop.
        assert!(SHUTDOWN_WAITS < Duration::from_secs(20));
        let started = Instant::now();
        // A game that ignores the close request: the host goes on without it.
        let slow = within(Duration::from_millis(200), "slow step", || {
            std::thread::sleep(Duration::from_secs(3))
        })
        .await;
        assert!(!slow);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(within(Duration::from_secs(5), "quick step", || {}).await);
    }

    #[test]
    fn panic_line_is_written_and_flushed_to_rotating_log() {
        let dir = std::env::temp_dir().join(format!(
            "butterpollo-panic-log-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("butterpollo.log");
        let file = butterpollo_session::logfile::RotatingFile::open_default(&path).unwrap();
        let shared = SharedLogFile::new(file);
        let stamp = utc_timestamp(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1));
        assert_eq!(stamp, "1970-01-01T00:00:01.000000Z");
        let line = format_panic_line(&stamp, "session", Some("src/foo.rs:42:5"), "boom");
        assert!(write_panic_line(&shared, &line));
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            on_disk.contains("ERROR butterpollo: host panicked"),
            "{on_disk}"
        );
        assert!(on_disk.contains("thread=\"session\""), "{on_disk}");
        assert!(
            on_disk.contains("location=\"src/foo.rs:42:5\""),
            "{on_disk}"
        );
        assert!(on_disk.contains("message=\"boom\""), "{on_disk}");
        drop(shared);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_request_stop_records_first_reason_and_restart_flag() {
        let fixture = butterpollo_stream::state::test_support::Fixture::new();
        let host = &fixture.host;
        assert_eq!(host.stop_reason(), None);
        host.request_stop("tray_restart", true);
        host.note_stop_reason("service_stop");
        assert_eq!(host.stop_reason(), Some("tray_restart"));
        assert!(host.stop.load(Ordering::Acquire));
        assert!(host.restart.load(Ordering::Acquire));
    }
}
