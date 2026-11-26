//! Command-line parsing and subcommand dispatch for the Rubylight host binary.
use crate::service_tasks;
use anyhow::{Context, Result};
use butterpollo_stream::{
    child_session, neural, nvidia_enhance, playnite, state, wake, win_bridge,
};
use clap::Parser;
use std::{
    net::IpAddr,
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(version, about = "Rubylight's Rust Moonlight streaming host")]
pub struct Args {
    #[arg(long)]
    pub config_dir: Option<PathBuf>,
    /// Copy a Vibepollo/Apollo profile into an empty --config-dir, then exit.
    #[arg(long, requires = "config_dir")]
    pub import_config: Option<PathBuf>,
    #[arg(long)]
    pub assets: Option<PathBuf>,
    #[arg(long)]
    pub port: Option<u16>,
    #[arg(long)]
    pub bind: Option<IpAddr>,
    #[arg(long)]
    pub diagnostics: bool,
    #[arg(long)]
    pub capture_smoke: bool,
    #[arg(long)]
    pub encoder_smoke: Option<String>,
    #[arg(long)]
    pub encoder_output: Option<PathBuf>,
    #[arg(long, default_value = "wgc")]
    pub capture: String,
    #[arg(long)]
    pub hdr: bool,
    #[arg(long, default_value = "h264")]
    pub codec: String,
    #[arg(long)]
    pub virtual_display_smoke: bool,
    #[arg(long)]
    pub no_tray: bool,
    /// Set the web console's username and password, then exit. Without
    /// --config-dir this changes the installed host's profile.
    #[arg(long, num_args = 2, value_names = ["USERNAME", "PASSWORD"])]
    pub creds: Option<Vec<String>>,
    #[arg(long, hide = true)]
    pub service_stop_source: Option<String>,
    #[arg(long, hide = true)]
    pub display_watch: Option<u32>,
    /// Quick Suspend's watchdog for the host with this id (suspend_recovery.rs).
    #[arg(long, hide = true, requires = "config_dir")]
    pub quick_suspend_watch: Option<u32>,
    #[arg(long, hide = true)]
    pub crash_reporter: bool,
    /// The NVIDIA component's check of NGX, in a process of its own; the
    /// value is the profile folder, for the component and its crash report.
    #[arg(long, hide = true, value_name = "PROFILE")]
    pub nvidia_probe: Option<PathBuf>,
    /// Run the display self-test and write its report; see
    /// butterpollo-service.exe --display-self-test.
    #[arg(long, hide = true)]
    pub display_self_test: Option<PathBuf>,
    #[arg(long, hide = true, requires = "rtss_parent")]
    pub rtss_worker: Option<String>,
    #[arg(long, hide = true, requires = "rtss_worker")]
    pub rtss_parent: Option<u32>,
    #[arg(long, hide = true, requires = "codec_probe_parent")]
    pub codec_probe_worker: Option<String>,
    #[arg(long, hide = true, requires = "codec_probe_worker")]
    pub codec_probe_parent: Option<u32>,
    #[arg(long, hide = true)]
    pub open_web: Option<u16>,
    #[arg(long, hide = true, requires = "open_web")]
    pub open_web_page: Option<String>,
    #[arg(long, hide = true, requires = "wgc_parent")]
    pub wgc_worker: Option<String>,
    #[arg(long, hide = true, requires = "wgc_worker")]
    pub wgc_parent: Option<u32>,
    #[arg(long, hide = true, num_args = 2, value_names = ["SOURCE", "TARGET"])]
    pub playnite_install: Option<Vec<PathBuf>>,
    #[arg(long, hide = true, value_name = "TARGET")]
    pub playnite_uninstall: Option<PathBuf>,
    /// One Radeon read or write in a fresh process: stdin carries the request, stdout the answer.
    #[arg(long, hide = true)]
    pub radeon_helper: bool,
    /// Print the GPUs the AMD compute runtime finds, for Neural Rendering.
    #[arg(long, hide = true)]
    pub neural_probe: bool,
    /// One Neural Rendering request on a game folder, as the signed-in user:
    /// stdin carries the request, stdout the answer.
    #[arg(long, hide = true)]
    pub neural_folder: bool,
    /// Seconds since the last input in this session, as the exit code (the
    /// host runs it in the signed-in user's session).
    #[arg(long, hide = true)]
    pub idle_seconds: bool,
    /// Apply the Wake-on-LAN settings to this PC's adapters and print what
    /// changed as JSON; exits 1 if anything failed. Needs administrator rights.
    #[arg(long, hide = true)]
    pub apply_wake_settings: bool,
    /// With --apply-wake-settings: also turn off Fast Startup.
    #[arg(long, hide = true, requires = "apply_wake_settings")]
    pub fast_startup_off: bool,
    /// Put back the Windows settings the removed child-session setup changed,
    /// from the PC's record, and delete the password it saved (the
    /// uninstaller runs it). Needs administrator rights.
    #[arg(long, hide = true)]
    pub child_session_restore: bool,
    /// The Windows service's tasks (service_tasks.rs): put back what a
    /// Nucleus Co-op session left at the console, check the virtual display
    /// driver.
    #[arg(long, hide = true, requires = "config_dir")]
    pub restore_nucleus_at_console: bool,
    #[arg(long, hide = true)]
    pub virtual_display_installed: bool,
}

pub fn run() -> Result<()> {
    butterpollo_win_graphics::capture::enable_dpi_awareness();
    let args = Args::parse();
    if args.crash_reporter {
        return butterpollo_win_io::crash::reporter();
    }
    if let Some(profile) = &args.nvidia_probe {
        std::process::exit(nvidia_enhance::probe_main(profile));
    }
    if args.radeon_helper {
        std::process::exit(butterpollo_win_graphics::radeon::helper::run());
    }
    if args.neural_probe {
        std::process::exit(neural::probe_main());
    }
    if args.neural_folder {
        std::process::exit(neural::folder::helper());
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
    if let (true, Some(profile)) = (args.restore_nucleus_at_console, &args.config_dir) {
        std::process::exit(service_tasks::restore_nucleus_exit_code(profile));
    }
    if args.virtual_display_installed {
        std::process::exit(service_tasks::virtual_display_installed_exit_code());
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
        return butterpollo_win_graphics::codec_probe::worker_with(
            pipe,
            args.codec_probe_parent.unwrap(),
            &win_bridge::HostUserLauncher,
        );
    }
    if let Some(pipe) = &args.wgc_worker {
        return butterpollo_win_graphics::capture::run_wgc_worker_with(
            pipe,
            args.wgc_parent.unwrap(),
            &win_bridge::HostUserLauncher,
        );
    }
    if let Some(source) = &args.import_config {
        // Setup keeps this output in its log: what was left out, and why.
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .init();
        return butterpollo_session::migration::import(source, args.config_dir.as_ref().unwrap());
    }
    if let Some(pipe) = &args.rtss_worker {
        return butterpollo_win_io::rtss::worker(pipe, args.rtss_parent.unwrap());
    }
    if let Some(port) = args.open_web {
        return butterpollo_win_io::tray::open_web_page(
            port,
            args.open_web_page.as_deref().unwrap_or("/"),
        );
    }
    if let (Some(pid), Some(profile)) = (args.quick_suspend_watch, &args.config_dir) {
        std::process::exit(crate::suspend_recovery::watch_exit_code(pid, profile));
    }
    if let Some(pid) = args.display_watch {
        win_bridge::install_hooks();
        return butterpollo_win_display::display_recovery::wait_and_recover_with(
            pid,
            &args
                .config_dir
                .context("display watcher requires a config directory")?,
            win_bridge::recovery_hooks(),
        );
    }
    if args.diagnostics {
        let _com = butterpollo_win_graphics::capture::ComGuard::new()?;
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"version":env!("CARGO_PKG_VERSION"),"displays":butterpollo_win_graphics::capture::displays()?,"monitors":butterpollo_win_display::display::monitors()?,"virtual_display_driver":butterpollo_win_display::display::virtual_display_available(),"virtual_display_status":butterpollo_win_display::display::virtual_display_status(),"firewall":std::env::current_exe().map_err(anyhow::Error::from).and_then(|exe| butterpollo_win_io::firewall::problems(&exe)).unwrap_or_else(|error| vec![format!("firewall rules could not be read: {error:#}")]),"wake":wake::diagnostics()})
            )?
        );
        return Ok(());
    }
    if let Some(report) = &args.display_self_test {
        let _com = butterpollo_win_graphics::capture::ComGuard::new()?;
        // The host log is not open in this process: keep what the checks log
        // (why Windows refused a layout) beside the report.
        if let Ok(log) = std::fs::File::create(report.with_extension("log")) {
            let _ = tracing_subscriber::fmt()
                .with_writer(std::sync::Mutex::new(log))
                .with_ansi(false)
                .with_max_level(tracing::Level::INFO)
                .try_init();
        }
        let passed = butterpollo_win_display::display::self_test::run(report)?;
        std::process::exit(if passed { 0 } else { 1 });
    }
    if args.virtual_display_smoke {
        let _com = butterpollo_win_graphics::capture::ComGuard::new()?;
        let display = butterpollo_win_display::display::VirtualDisplay::create(
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
                let installed = butterpollo_session::paths::installed_profile();
                if installed.join("sunshine.conf").is_file() {
                    installed
                } else {
                    butterpollo_session::paths::portable_profile()?
                }
            }
        };
        let saved_dir =
            reset_credentials(directory, assets, args.port, creds[0].clone(), &creds[1])?;
        println!(
            "Saved the web console sign-in in {}. Sign in with it now; a running Rubylight takes it up without a restart.",
            saved_dir.display()
        );
        return Ok(());
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(crate::server::run(args, assets))
}

fn reset_credentials(
    directory: PathBuf,
    assets: PathBuf,
    port: Option<u16>,
    username: String,
    password: &str,
) -> Result<PathBuf> {
    let h = state::Host::load(directory, assets, port)?;
    let credentials = butterpollo_session::state::Credentials::new(username, password)?;
    // As with a password change in the console, signed-in browsers and API
    // tokens sign in again: a token is bound only to the username.
    h.revoke_sign_ins()?;
    h.save_credentials(&credentials)
        .context("saving the credentials (an installed host needs an administrator)")?;
    Ok(h.directory.clone())
}

fn smoke(args: &Args) -> Result<()> {
    let _com = butterpollo_win_graphics::capture::ComGuard::new()?;
    let _priority = butterpollo_win_graphics::capture::Priority::new();
    let image = if args.capture_smoke {
        let mut capture =
            butterpollo_win_graphics::capture::Capture::new_format("", &args.capture, args.hdr)?;
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
        butterpollo_win_graphics::capture::Image {
            width: 128,
            height: 128,
            stride: 512,
            bytes: vec![128; 128 * 128 * 4],
            captured: Instant::now(),
            pixel: butterpollo_win_graphics::capture::Pixel::Bgra8,
        }
    };
    let mut output = serde_json::json!({"capture":args.capture,"width":image.width,"height":image.height,"bytes":image.bytes.len()});
    if let Some(name) = &args.encoder_smoke {
        let cfg = butterpollo_media_policy::rtsp::Negotiated {
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
        let mut encoder = butterpollo_win_graphics::encoder::Encoder::new(&cfg, "")?;
        let mut packets = 0;
        let mut bytes = 0;
        for i in 0..31 {
            // The last pass collects what is still in flight.
            let frames = if i < 30 {
                encoder.encode(&image, i == 0, cfg.bitrate_kbps)?
            } else {
                encoder.finish()?
            };
            for frame in frames {
                if packets == 0
                    && let Some(path) = &args.encoder_output
                {
                    std::fs::write(path, frame.payload()?.to_vec())?;
                }
                packets += 1;
                bytes += frame.wire_len();
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

#[cfg(test)]
mod tests {
    use clap::Parser;

    #[test]
    fn the_uninstallers_restore_flag_is_the_hosts() {
        let args = crate::cli::Args::try_parse_from([
            "butterpollo.exe",
            butterpollo_session::child_session::RESTORE_FLAG,
        ])
        .unwrap();
        assert!(args.child_session_restore);
    }

    #[test]
    fn cli_creds_reset_revokes_both_browser_sessions_and_api_tokens() {
        let fixture = butterpollo_stream::state::test_support::Fixture::new();
        let initial =
            butterpollo_session::state::Credentials::new("admin".into(), "oldpassword").unwrap();
        fixture.host.save_credentials(&initial).unwrap();
        let seeded = serde_json::json!({
            "root": {
                "session_tokens": [{"hash": "session-1", "username": "admin"}],
                "api_tokens": [{"hash": "token-1", "username": "admin", "scopes": []}],
            }
        });
        butterpollo_session::state::write_json(&fixture.host.aliases_path, &seeded).unwrap();

        let dir = fixture.host.directory.clone();
        let assets = fixture.host.assets.clone();
        super::reset_credentials(
            dir.clone(),
            assets.clone(),
            None,
            "admin".into(),
            "newpassword",
        )
        .unwrap();

        let reloaded = butterpollo_stream::state::Host::load(dir, assets, None).unwrap();
        let creds = reloaded.credentials.read().unwrap();
        let creds = creds.as_ref().expect("updated credentials");
        assert!(creds.verifies("admin", "newpassword"));
        assert!(!creds.verifies("admin", "oldpassword"));
        let aliases = reloaded.aliases.lock().unwrap();
        assert_eq!(aliases["root"]["session_tokens"], serde_json::json!([]));
        assert_eq!(aliases["root"]["api_tokens"], serde_json::json!([]));
    }
}
