mod child_session;
mod console;
mod controller_sessions;
mod couch_menu;
mod display_session;
mod extensions;
mod game_files;
mod launcher_watch;
mod library;
mod logging;
mod lossless;
mod maintenance;
mod mic;
mod native_api;
mod network;
mod network_path;
mod nucleus;
mod nvhttp;
mod playnite;
mod power_action;
mod process;
mod pyrowave_probe;
mod pyrowave_send;
mod radeon_profile;
mod radeon_session;
mod remote_display;
mod rtsp_server;
mod runtime;
mod session_control;
mod session_notes;
#[cfg(any(debug_assertions, test))]
mod soak_fault;
mod stall_watch;
mod state;
mod steam;
mod stream;
mod stream_focus;
mod telemetry;
mod timing_record;
mod tls;
mod updater;
mod usb_forward;
mod video_send;
mod wake;
mod web;
mod web_sessions;
mod webhooks;
mod webrtc;

use anyhow::{Context, Result};
use clap::Parser;
use std::{
    net::IpAddr,
    path::PathBuf,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(version, about = "Rubylight's Rust Moonlight streaming host")]
struct Args {
    #[arg(long)]
    config_dir: Option<PathBuf>,
    /// Copy a Vibepollo/Apollo profile into an empty --config-dir, then exit.
    #[arg(long, requires = "config_dir")]
    import_config: Option<PathBuf>,
    #[arg(long)]
    assets: Option<PathBuf>,
    #[arg(long)]
    port: Option<u16>,
    #[arg(long)]
    bind: Option<IpAddr>,
    #[arg(long)]
    diagnostics: bool,
    #[arg(long)]
    capture_smoke: bool,
    #[arg(long)]
    encoder_smoke: Option<String>,
    #[arg(long)]
    encoder_output: Option<PathBuf>,
    #[arg(long, default_value = "wgc")]
    capture: String,
    #[arg(long)]
    hdr: bool,
    #[arg(long, default_value = "h264")]
    codec: String,
    #[arg(long)]
    virtual_display_smoke: bool,
    #[arg(long)]
    no_tray: bool,
    /// Set the web console's username and password, then exit. Without
    /// --config-dir this changes the installed host's profile.
    #[arg(long, num_args = 2, value_names = ["USERNAME", "PASSWORD"])]
    creds: Option<Vec<String>>,
    #[arg(long, hide = true)]
    service_stop_source: Option<String>,
    #[arg(long, hide = true)]
    display_watch: Option<u32>,
    #[arg(long, hide = true)]
    crash_reporter: bool,
    /// Run the display self-test and write its report; see
    /// butterpollo-service.exe --display-self-test.
    #[arg(long, hide = true)]
    display_self_test: Option<PathBuf>,
    #[arg(long, hide = true, requires = "rtss_parent")]
    rtss_worker: Option<String>,
    #[arg(long, hide = true, requires = "rtss_worker")]
    rtss_parent: Option<u32>,
    #[arg(long, hide = true, requires = "codec_probe_parent")]
    codec_probe_worker: Option<String>,
    #[arg(long, hide = true, requires = "codec_probe_worker")]
    codec_probe_parent: Option<u32>,
    #[arg(long, hide = true)]
    open_web: Option<u16>,
    #[arg(long, hide = true, requires = "wgc_parent")]
    wgc_worker: Option<String>,
    #[arg(long, hide = true, requires = "wgc_worker")]
    wgc_parent: Option<u32>,
    #[arg(long, hide = true, num_args = 2, value_names = ["SOURCE", "TARGET"])]
    playnite_install: Option<Vec<PathBuf>>,
    #[arg(long, hide = true, value_name = "TARGET")]
    playnite_uninstall: Option<PathBuf>,
    /// One Radeon read or write in a fresh process: stdin carries the request, stdout the answer.
    #[arg(long, hide = true)]
    radeon_helper: bool,
    /// Seconds since the last input in this session, as the exit code (the
    /// host runs it in the signed-in user's session).
    #[arg(long, hide = true)]
    idle_seconds: bool,
    /// Apply the Wake-on-LAN settings to this PC's adapters and print what
    /// changed as JSON; exits 1 if anything failed. Needs administrator rights.
    #[arg(long, hide = true)]
    apply_wake_settings: bool,
    /// With --apply-wake-settings: also turn off Fast Startup.
    #[arg(long, hide = true, requires = "apply_wake_settings")]
    fast_startup_off: bool,
    /// Put back the Windows settings the removed child-session setup changed,
    /// from the PC's record, and delete the password it saved (the
    /// uninstaller runs it). Needs administrator rights.
    #[arg(long, hide = true)]
    child_session_restore: bool,
}
fn main() -> Result<()> {
    butterpollo_windows::capture::enable_dpi_awareness();
    let args = Args::parse();
    if args.crash_reporter {
        return butterpollo_windows::crash::reporter();
    }
    if args.radeon_helper {
        std::process::exit(butterpollo_windows::radeon::helper::run());
    }
    if args.idle_seconds {
        std::process::exit(wake::idle_seconds_exit_code());
    }
    if args.apply_wake_settings {
        std::process::exit(wake::apply_settings_exit_code(args.fast_startup_off));
    }
    if args.child_session_restore {
        std::process::exit(child_session::restore_exit_code());
    }
    if let Some(paths) = &args.playnite_install {
        std::process::exit(playnite::helper_exit_code(&playnite::install_worker(
            &paths[0], &paths[1],
        )));
    }
    if let Some(target) = &args.playnite_uninstall {
        std::process::exit(playnite::helper_exit_code(&playnite::uninstall_worker(
            target,
        )));
    }
    if let Some(pipe) = &args.codec_probe_worker {
        return butterpollo_windows::codec_probe::worker(pipe, args.codec_probe_parent.unwrap());
    }
    if let Some(pipe) = &args.wgc_worker {
        return butterpollo_windows::capture::run_wgc_worker(pipe, args.wgc_parent.unwrap());
    }
    if let Some(source) = &args.import_config {
        // Setup keeps this output in its log: what was left out, and why.
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .init();
        return butterpollo_core::migration::import(source, args.config_dir.as_ref().unwrap());
    }
    if let Some(pipe) = &args.rtss_worker {
        return butterpollo_windows::rtss::worker(pipe, args.rtss_parent.unwrap());
    }
    if let Some(port) = args.open_web {
        return butterpollo_windows::tray::open_web(port);
    }
    if let Some(pid) = args.display_watch {
        return butterpollo_windows::display_recovery::wait_and_recover(
            pid,
            &args
                .config_dir
                .context("display watcher requires a config directory")?,
        );
    }
    if args.diagnostics {
        let _com = butterpollo_windows::capture::ComGuard::new()?;
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"version":env!("CARGO_PKG_VERSION"),"displays":butterpollo_windows::capture::displays()?,"monitors":butterpollo_windows::display::monitors()?,"virtual_display_driver":butterpollo_windows::display::virtual_display_available(),"virtual_display_status":butterpollo_windows::display::virtual_display_status(),"firewall":std::env::current_exe().map_err(anyhow::Error::from).and_then(|exe| butterpollo_windows::firewall::problems(&exe)).unwrap_or_else(|error| vec![format!("firewall rules could not be read: {error:#}")]),"wake":wake::diagnostics()})
            )?
        );
        return Ok(());
    }
    if let Some(report) = &args.display_self_test {
        let _com = butterpollo_windows::capture::ComGuard::new()?;
        // The host log is not open in this process: keep what the checks log
        // (why Windows refused a layout) beside the report.
        if let Ok(log) = std::fs::File::create(report.with_extension("log")) {
            let _ = tracing_subscriber::fmt()
                .with_writer(std::sync::Mutex::new(log))
                .with_ansi(false)
                .with_max_level(tracing::Level::INFO)
                .try_init();
        }
        let passed = butterpollo_windows::display::self_test::run(report)?;
        std::process::exit(if passed { 0 } else { 1 });
    }
    if args.virtual_display_smoke {
        let _com = butterpollo_windows::capture::ComGuard::new()?;
        let display = butterpollo_windows::display::VirtualDisplay::create(
            &uuid::Uuid::new_v4().to_string(),
            640,
            480,
            30,
        )?;
        println!("Created Rust virtual display {}", display.name);
        drop(display);
        println!("Virtual display lease removed");
        return Ok(());
    }
    if args.capture_smoke || args.encoder_smoke.is_some() {
        return smoke(&args);
    }
    let assets = args.assets.clone().unwrap_or_else(|| {
        std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .join("assets/web")
    });
    if let Some(creds) = &args.creds {
        // The service's profile, unless another one is named.
        let directory = match args.config_dir.clone() {
            Some(directory) => directory,
            None => {
                let installed = butterpollo_core::paths::installed_profile();
                if installed.join("sunshine.conf").is_file() {
                    installed
                } else {
                    butterpollo_core::paths::portable_profile()?
                }
            }
        };
        let h = state::Host::load(directory, assets, args.port)?;
        let credentials = butterpollo_core::state::Credentials::new(creds[0].clone(), &creds[1])?;
        // As with a password change in the console, signed-in browsers sign
        // in again.
        h.save_web_sessions(&Default::default())?;
        h.save_credentials(&credentials)
            .context("saving the credentials (an installed host needs an administrator)")?;
        println!(
            "Saved the web console sign-in in {}. Sign in with it now; a running Rubylight takes it up without a restart.",
            h.directory.display()
        );
        return Ok(());
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(args, assets))
}

/// Rubylight streams only with an AMD GPU. A PC without one still starts the
/// console, whose warnings and the encoder probe say why no stream can start;
/// this puts the same words in the log. The AMF runtime is the probe's to judge.
fn log_amd_check() {
    let adapters = butterpollo_windows::capture::gpus()
        .map(|gpus| butterpollo_windows::capture::amd_adapters(&gpus))
        .unwrap_or_default();
    match butterpollo_core::amd_gpu::check(&adapters, true) {
        Ok(()) => {}
        // No driver may only mean the GPU is not up yet, as at boot.
        Err(refusal @ butterpollo_core::amd_gpu::Refusal::NoGpu) => {
            tracing::warn!(%refusal, "no graphics adapter yet; streams wait for an AMD GPU");
        }
        Err(refusal) => tracing::error!(%refusal, "this PC cannot stream; no AMD GPU"),
    }
}

async fn run(args: Args, assets: PathBuf) -> Result<()> {
    let supervised = args.service_stop_source.is_some();
    let stop_signal =
        butterpollo_windows::process::StopSignal::new(args.service_stop_source.as_deref())?;
    let directory = match args.config_dir {
        Some(directory) => directory,
        None => butterpollo_core::paths::portable_profile()?,
    };
    let h = state::Host::load(directory, assets, args.port)?;
    // Crash reporting must not keep the host from starting, e.g. when the
    // reporter process is blocked.
    let crash_reporting = butterpollo_windows::crash::initialize(&h.directory).err();
    let display_recovery = butterpollo_windows::display_recovery::initialize(&h.directory)?;
    let log_path = h
        .config
        .read()
        .unwrap()
        .path("log_path", &h.directory, "logs/butterpollo.log");
    let open = |path: &std::path::Path| -> Result<_> {
        std::fs::create_dir_all(path.parent().context("log directory missing")?)?;
        Ok(butterpollo_core::logfile::RotatingFile::open_default(path)?)
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
    let (writer, _log_guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .buffered_lines_limit(8192)
        .lossy(true)
        .finish(appender);
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
    // The first launcher scan for the native library runs on its own thread, so the first request
    // finds it done and startup does not wait.
    library::installed(&h);
    // Radeon settings a crashed run left changed are put back by the Radeon worker thread, which
    // owns every later Radeon write too and queues them behind the recovery. Startup does not
    // wait for it, and with no journal it never touches ADLX. The worker logs failures and retries.
    if let Err(error) = radeon_session::start(&h.radeon, &h.directory) {
        tracing::error!(
            error = %error,
            "the Radeon worker could not start; settings from an interrupted session stay in the journal"
        );
    }
    // Keys in games' settings files a crashed run left changed go back before any stream.
    game_files::start(&h);
    // The service's keys: SYSTEM and Administrators only, also after a
    // profile was copied in or restored from a backup.
    let credentials = h.directory.join("credentials");
    if butterpollo_windows::process::is_system()
        && credentials.is_dir()
        && let Err(error) = butterpollo_windows::process::restrict_to_administrators(&credentials)
    {
        tracing::warn!(%error, "the credentials folder permissions could not be repaired");
    }
    let ports = h.config.read().unwrap().ports()?;
    if let Err(error) = butterpollo_windows::vulkan::reconcile(
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
    let port_forward = network::port_forward(h.clone(), bind);
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
        match butterpollo_windows::tray::Tray::new_options(
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
    log_amd_check();
    h.probe_codecs();
    let mut tasks = tokio::task::JoinSet::new();
    for (port, https, web) in [
        (ports.http, false, false),
        (ports.https, true, false),
        (ports.web, true, true),
    ] {
        let router = if web {
            web::router_with(h.clone(), webrtc::server::Console::new(Some(media.clone())))
        } else {
            nvhttp::router(h.clone(), https)
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
    // None when the setting is off or the port is taken; the console says which.
    let webrtc = webrtc::server::start(h.clone(), bind, Some(media));
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
    if bind.is_unspecified() {
        std::thread::spawn(|| {
            let Ok(program) = std::env::current_exe() else {
                return;
            };
            match butterpollo_windows::firewall::problems(&program) {
                Ok(problems) => {
                    for problem in problems {
                        tracing::warn!(program = %program.display(), "{problem}");
                    }
                }
                Err(error) => tracing::debug!(%error, "Windows Firewall rules could not be read"),
            }
        });
    }
    let outcome: Result<()> = tokio::select! {
        signal=tokio::signal::ctrl_c()=>signal.context("waiting for shutdown"),
        task=tasks.join_next()=>{
            match task {
                Some(Ok(Err(error))) => Err(error.context("host listener failed")),
                Some(Err(error)) => Err(error.into()),
                _ => Err(anyhow::anyhow!("host listener stopped unexpectedly")),
            }
        },
        _=runtime::maintain(h.clone(), stop_signal, actions, ports.web)=>Ok(())
    };
    h.stop.store(true, Ordering::Release);
    let canceled = h.sessions.lock().unwrap().request_stop(None);
    drop(canceled);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !h.sessions.lock().unwrap().idle() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    drop(webrtc);
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    h.stop_app(state::AppStop::Shutdown);
    h.session_shutdown();
    // Whatever a session changed goes back before the host exits; the restore thread has had the
    // work since the session reset above. The wait is blocking, so it leaves the async executor.
    // 30 s is shorter than the worker's 150 s limit on one driver call: a restore still stuck
    // then stays journaled, is logged, and the next host start puts it back.
    let radeon = h.clone();
    if let Err(error) =
        tokio::task::spawn_blocking(move || radeon.radeon.shutdown(Duration::from_secs(30))).await
    {
        tracing::error!(%error, "waiting for the Radeon restore at shutdown failed; the next host start restores what is left");
    }
    // Every game file key goes back too, on its own thread; one it cannot reach in time stays
    // journaled for the next start.
    let files = h.clone();
    if let Err(error) = tokio::task::spawn_blocking(move || {
        files
            .game_files
            .shutdown(&files.directory, Duration::from_secs(10))
    })
    .await
    {
        tracing::error!(%error, "waiting for the game file restore at shutdown failed; the next host start restores what is left");
    }
    remote_display::disconnect(&h, None);
    drop(discovery);
    if let Some(mut task) = port_forward
        && tokio::time::timeout(Duration::from_secs(10), &mut task)
            .await
            .is_err()
    {
        task.abort();
    }
    drop(tray);
    if h.restart.load(Ordering::Acquire) {
        if supervised {
            drop(_log_guard);
            std::process::exit(butterpollo_windows::service::RESTART_EXIT_CODE as i32);
        } else {
            use std::os::windows::process::CommandExt;
            std::process::Command::new(std::env::current_exe()?)
                .args(std::env::args_os().skip(1))
                .creation_flags(0x08000000)
                .spawn()?;
        }
    }
    outcome
}
fn smoke(args: &Args) -> Result<()> {
    let _com = butterpollo_windows::capture::ComGuard::new()?;
    let _priority = butterpollo_windows::capture::Priority::new();
    let image = if args.capture_smoke {
        let mut capture =
            butterpollo_windows::capture::Capture::new_format("", &args.capture, args.hdr)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(image) = capture.next_frame()? {
                break image;
            }
            if Instant::now() > deadline {
                anyhow::bail!("capture did not deliver a frame");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    } else {
        butterpollo_windows::capture::Image {
            width: 128,
            height: 128,
            stride: 512,
            bytes: vec![128; 128 * 128 * 4],
            captured: Instant::now(),
            pixel: butterpollo_windows::capture::Pixel::Bgra8,
        }
    };
    let mut output = serde_json::json!({"capture":args.capture,"width":image.width,"height":image.height,"bytes":image.bytes.len()});
    if let Some(name) = &args.encoder_smoke {
        let cfg = butterpollo_core::rtsp::Negotiated {
            width: if args.codec == "h264" { 128 } else { 256 },
            height: if args.codec == "h264" { 128 } else { 256 },
            hdr: args.hdr,
            codec: match args.codec.as_str() {
                "hevc" | "h265" => 1,
                "av1" => 2,
                "pyrowave" => 3,
                _ => 0,
            },
            ..Default::default()
        };
        let mut encoder = butterpollo_windows::encoder::Encoder::new(&cfg, "")?;
        let mut packets = 0;
        let mut bytes = 0;
        for i in 0..30 {
            for frame in encoder.encode(&image, i == 0, cfg.bitrate_kbps)? {
                if packets == 0
                    && let Some(path) = &args.encoder_output
                {
                    std::fs::write(path, &frame.bytes)?;
                }
                packets += 1;
                bytes += frame.bytes.len();
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        if packets == 0 {
            anyhow::bail!("encoder delivered no packets");
        }
        output["encoder"] = serde_json::json!(name);
        output["encoded_frames"] = serde_json::json!(packets);
        output["encoded_bytes"] = serde_json::json!(bytes);
    }
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
