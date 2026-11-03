//! The per-app audio route (`audio_app_route`) against this PC's Windows, through the host's own
//! code: run as SYSTEM through `system_launch` (staged as the test host), it checks what the
//! service host does for a game in the user's session (#59).
//!
//!   app_route_probe hold SECONDS            open a silent playback stream and keep it
//!   app_route_probe follow SECONDS          ...on Windows' default device itself, which
//!                                           Windows switches as the default changes
//!   app_route_probe self OUT                Windows refuses a process without sound, then routes
//!                                           it once it opened its sound, and puts it back
//!   app_route_probe route PID SECONDS OUT   route process PID, keep it SECONDS, put it back
//!
//! Every line also goes to OUT, since a SYSTEM task shows no console.
use anyhow::{Context, Result};
use butterpollo_core::config::Config;
use butterpollo_windows::audio_app_route::{AppRoute, no_sound_yet};
use std::{io::Write, path::Path, sync::mpsc, time::Duration};
use windows::{
    Win32::{
        Media::Audio::{
            AUDCLNT_SHAREMODE_SHARED, ActivateAudioInterfaceAsync, DEVINTERFACE_AUDIO_RENDER,
            IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
            IActivateAudioInterfaceCompletionHandler_Impl, IAudioClient, IMMDeviceEnumerator,
            MMDeviceEnumerator, eConsole, eRender,
        },
        System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, StringFromIID},
    },
    core::{Interface, PCWSTR, implement},
};

/// A shared stream on the default playback device, started: silence.
fn open_sound() -> Result<IAudioClient> {
    // SAFETY: COM is initialised by the caller; every interface is owned here.
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let format = client.GetMixFormat()?;
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 10_000_000, 0, format, None)?;
        client.Start()?;
        Ok(client)
    }
}

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Done(mpsc::SyncSender<()>);
impl IActivateAudioInterfaceCompletionHandler_Impl for Done_Impl {
    fn ActivateCompleted(
        &self,
        _operation: windows::core::Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        let _ = self.0.try_send(());
        Ok(())
    }
}

/// [`open_sound`] on Windows' default playback device itself
/// (`DEVINTERFACE_AUDIO_RENDER`), which Windows switches when the default
/// changes, as engines that follow the default do.
fn open_following_sound() -> Result<IAudioClient> {
    let (sender, done) = mpsc::sync_channel(1);
    let handler: IActivateAudioInterfaceCompletionHandler = Done(sender).into();
    // SAFETY: COM is initialised by the caller; the device string and handler
    // outlive the call, and the result is read after completion.
    unsafe {
        let device = StringFromIID(&DEVINTERFACE_AUDIO_RENDER)?;
        let operation =
            ActivateAudioInterfaceAsync(PCWSTR(device.0), &IAudioClient::IID, None, &handler)?;
        CoTaskMemFree(Some(device.0 as _));
        done.recv_timeout(Duration::from_secs(5))?;
        let mut result = windows::core::HRESULT(0);
        let mut interface = None;
        operation.GetActivateResult(&mut result, &mut interface)?;
        result.ok()?;
        let client: IAudioClient = interface.context("no audio client")?.cast()?;
        let format = client.GetMixFormat()?;
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 10_000_000, 0, format, None)?;
        client.Start()?;
        Ok(client)
    }
}

struct Out(Option<std::fs::File>);
impl Out {
    fn line(&mut self, text: String) {
        println!("{text}");
        if let Some(file) = &mut self.0 {
            let _ = writeln!(file, "{text}");
        }
    }
}

/// Route `pid` and say where its sound plays.
fn route(out: &mut Out, config: &Config, directory: &Path, pid: u32) -> Option<AppRoute> {
    match AppRoute::acquire(config, directory, pid) {
        Ok(route) => {
            std::thread::sleep(Duration::from_millis(500));
            let elsewhere = AppRoute::plays_elsewhere(&[&route], &[]);
            out.line(format!(
                "route pid={pid} sink={} users={:?} on_the_pc={elsewhere:?}",
                route.sink,
                route.users_device()
            ));
            Some(route)
        }
        Err(error) => {
            out.line(format!(
                "route pid={pid} failed no_sound_yet={} error={error:#}",
                no_sound_yet(&error)
            ));
            None
        }
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let _com = butterpollo_windows::capture::ComGuard::new()?;
    let arg = |at: usize| args.get(at).context("missing argument");
    let config = Config::default();
    match arg(0)?.as_str() {
        mode @ ("hold" | "follow") => {
            let _sound = if mode == "hold" {
                open_sound()?
            } else {
                open_following_sound()?
            };
            println!("pid={} holds a silent stream", std::process::id());
            std::thread::sleep(Duration::from_secs(arg(1)?.parse()?));
        }
        "self" => {
            let mut out = Out(Some(std::fs::File::create(arg(1)?)?));
            let directory = Path::new(arg(1)?).with_extension("state");
            std::fs::create_dir_all(&directory)?;
            out.line(format!(
                "system={}",
                butterpollo_windows::process::is_system()
            ));
            let pid = std::process::id();
            let before = route(&mut out, &config, &directory, pid);
            out.line(format!("before_sound routed={}", before.is_some()));
            let _sound = open_sound()?;
            let after = route(&mut out, &config, &directory, pid);
            drop(after);
            out.line("put back".into());
        }
        "route" => {
            let pid: u32 = arg(1)?.parse()?;
            let seconds: u64 = arg(2)?.parse()?;
            let mut out = Out(Some(std::fs::File::create(arg(3)?)?));
            let directory = Path::new(arg(3)?).with_extension("state");
            std::fs::create_dir_all(&directory)?;
            out.line(format!(
                "system={}",
                butterpollo_windows::process::is_system()
            ));
            let routed = route(&mut out, &config, &directory, pid);
            std::thread::sleep(Duration::from_secs(seconds));
            drop(routed);
            out.line("put back".into());
        }
        other => anyhow::bail!("unknown mode {other}"),
    }
    Ok(())
}
