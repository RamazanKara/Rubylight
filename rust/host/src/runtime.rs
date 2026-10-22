//! Host housekeeping and user actions, independent of listener setup.
use crate::{maintenance, state::Shared};
use butterpollo_core::session::Role;
use butterpollo_windows::{process::StopSignal, tray::Action};
use std::{
    sync::{atomic::Ordering, mpsc::Receiver},
    time::{Duration, Instant},
};

/// Whether a client streams the running app.
fn streaming(h: &Shared) -> bool {
    h.sessions
        .lock()
        .unwrap()
        .active
        .values()
        .any(|session| session.launch.role == Role::Stream && !session.stopping())
}
fn application_finished(h: &Shared) -> bool {
    let connected = streaming(h);
    h.current_app.lock().unwrap().as_mut().is_some_and(|app| {
        app.connection_state(connected)
            || match app.exited() {
                Ok(finished) => finished,
                Err(error) => {
                    tracing::warn!(%error, "application exit check failed");
                    false
                }
            }
    })
}
/// Start an app that was launched before anyone signed in, keeping its
/// session identity. It replaces the placeholder only if that still runs.
fn start_deferred(h: &Shared) {
    let pending = h.current_app.lock().unwrap().as_mut().and_then(|app| {
        app.take_ready_deferred()
            .map(|(app_config, args)| (app_config, args, app.generation.clone(), app.owner.clone()))
    });
    let Some((app, args, generation, owner)) = pending else {
        return;
    };
    let h = h.clone();
    tokio::task::spawn_blocking(move || {
        let _transition = h.launch_transition.lock().unwrap();
        if !h
            .current_app
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|app| app.generation == generation)
        {
            return;
        }
        tracing::info!(app = %app.name, "user signed in; starting the deferred application");
        match crate::process::launch(&h, &app, &args) {
            Ok(mut started) => {
                let mut current = h.current_app.lock().unwrap();
                if current
                    .as_ref()
                    .is_some_and(|placeholder| placeholder.generation == generation)
                {
                    started.generation = generation;
                    started.owner = owner;
                    *current = Some(started);
                }
            }
            Err(error) => {
                tracing::warn!(%error, app = %app.name, "deferred application failed to start");
                let canceled = h.sessions.lock().unwrap().stop_role(Role::Stream, None);
                drop(canceled);
                h.stop_app();
            }
        }
    });
}
fn user_action(h: &Shared, action: Action, web_port: u16) {
    match action {
        Action::Open => {
            if let Err(error) = butterpollo_windows::tray::open_web(web_port) {
                tracing::warn!(%error, "could not open the administration console");
            }
        }
        Action::StopSessions => {
            let h = h.clone();
            tokio::task::spawn_blocking(move || {
                let _transition = h.launch_transition.lock().unwrap();
                release_displays(&h);
            });
        }
        Action::QuitApp => {
            tracing::info!("quitting the running app from the tray");
            let h = h.clone();
            // Stopping the app waits for it to exit.
            tokio::task::spawn_blocking(move || {
                let _transition = h.launch_transition.lock().unwrap();
                let canceled = h.sessions.lock().unwrap().stop_role(Role::Stream, None);
                drop(canceled);
                h.stop_app();
            });
        }
        Action::CheckUpdates => maintenance::trigger_update(h, true),
        Action::Restart => {
            h.restart.store(true, Ordering::Release);
            h.stop.store(true, Ordering::Release);
        }
        Action::Quit => h.stop.store(true, Ordering::Release),
    }
}
/// Keep routine app status in the tray instead of covering the captured
/// desktop with launch, pause and resume notifications.
fn show_app(h: &Shared, tray: &mut butterpollo_core::tray::Tracker) {
    let app = h
        .current_app
        .lock()
        .unwrap()
        .as_ref()
        .map(|app| app.name.clone());
    let current = butterpollo_core::tray::Status {
        app,
        streaming: streaming(h),
    };
    let Some(_) = tray.update(current) else {
        return;
    };
    butterpollo_windows::tray::show_app(tray.icon(), tray.app(), tray.summary().as_deref());
}
/// End every stream and remote monitor and release the displays they held,
/// which removes the virtual displays and restores the layout.
pub fn release_displays(h: &Shared) {
    let teardown = h.session_teardown(crate::session_control::Scope::Session);
    let canceled = h.sessions.lock().unwrap().request_stop(None);
    drop(canceled);
    crate::remote_display::disconnect(h, None);
    let audio = h.app_audio.lock().unwrap().take();
    let displays = std::mem::take(&mut *h.app_display.lock().unwrap());
    drop((audio, displays));
    drop(teardown);
}
/// The registered restore hotkey, following the setting.
#[derive(Default)]
struct RestoreHotkey {
    wanted: Option<(u32, u32)>,
    registered: Option<butterpollo_windows::hotkey::Hotkey>,
    presses: Option<std::sync::mpsc::Receiver<()>>,
    checked: Option<Instant>,
}
impl RestoreHotkey {
    fn poll(&mut self, h: &Shared) {
        if self
            .presses
            .as_ref()
            .is_some_and(|presses| presses.try_iter().count() > 0)
        {
            tracing::info!("restore hotkey pressed; ending streams and restoring the displays");
            let h = h.clone();
            // Restoring the layout waits on Windows.
            tokio::task::spawn_blocking(move || release_displays(&h));
        }
        if self
            .checked
            .is_some_and(|at| at.elapsed() < Duration::from_secs(2))
        {
            return;
        }
        self.checked = Some(Instant::now());
        let wanted = butterpollo_core::hotkey::restore_hotkey(&h.config.read().unwrap());
        if wanted == self.wanted {
            return;
        }
        self.wanted = wanted;
        self.registered = None;
        self.presses = None;
        let Some((key, modifiers)) = wanted else {
            return;
        };
        let (pressed, presses) = std::sync::mpsc::channel();
        match butterpollo_windows::hotkey::Hotkey::register(key, modifiers, move || {
            let _ = pressed.send(());
        }) {
            Ok(hotkey) => {
                tracing::debug!(key, modifiers, "registered the display restore hotkey");
                self.registered = Some(hotkey);
                self.presses = Some(presses);
            }
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), key, modifiers, "the display restore hotkey could not be registered");
            }
        }
    }
}
pub async fn maintain(
    h: Shared,
    stop_signal: StopSignal,
    actions: Option<Receiver<Action>>,
    web_port: u16,
) {
    let mut update_at = Instant::now();
    let mut steam_at = Instant::now() + Duration::from_secs(5);
    let mut hotkey = RestoreHotkey::default();
    let mut tray = butterpollo_core::tray::Tracker::default();
    let mut housekeeping: Option<tokio::task::JoinHandle<()>> = None;
    while !h.stop.load(Ordering::Acquire) {
        if stop_signal.requested() {
            tracing::info!("service requested host shutdown");
            h.stop.store(true, Ordering::Release);
            break;
        }
        let expired = h.sessions.lock().unwrap().expire();
        if !expired.is_empty() {
            tokio::task::spawn_blocking(move || drop(expired));
        }
        if housekeeping.as_ref().is_none_or(|task| task.is_finished())
            && (h.current_app.lock().unwrap().is_some()
                || h.app_audio.lock().unwrap().is_some()
                || !h.app_display.lock().unwrap().is_empty())
        {
            let h = h.clone();
            housekeeping = Some(tokio::task::spawn_blocking(move || {
                let Ok(_transition) = h.launch_transition.try_lock() else {
                    return;
                };
                h.reap_paused_display();
                if application_finished(&h) {
                    let canceled = h.sessions.lock().unwrap().stop_role(Role::Stream, None);
                    drop(canceled);
                    h.stop_app();
                }
            }));
        }
        h.session_tick();
        hotkey.poll(&h);
        if Instant::now() >= steam_at {
            steam_at = Instant::now() + Duration::from_secs(30);
            let h = h.clone();
            tokio::task::spawn_blocking(move || {
                crate::steam::watch(&h);
                crate::playnite::watch(&h);
            });
        }
        if Instant::now() >= update_at {
            let interval = h
                .config
                .read()
                .unwrap()
                .integer("update_check_interval", 86400);
            // Not during a stream, as in Vibepollo; try again in a minute.
            let streaming = crate::updater::busy(&h);
            if interval > 0 && !streaming {
                maintenance::trigger_update(&h, false);
            }
            update_at = Instant::now()
                + Duration::from_secs(if interval > 0 && !streaming {
                    interval.max(3600) as u64
                } else {
                    60
                });
        }
        crate::updater::poll(&h);
        start_deferred(&h);
        if actions.is_some() {
            show_app(&h, &mut tray);
        }
        if let Some(actions) = &actions {
            while let Ok(action) = actions.try_recv() {
                user_action(&h, action, web_port);
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;

    #[test]
    fn global_close_on_disconnect_waits_for_the_last_streaming_device() {
        let fixture = Fixture::new();
        fixture
            .host
            .config
            .write()
            .unwrap()
            .values
            .insert("terminate_on_pause".into(), "true".into());
        let app = serde_json::from_value(serde_json::json!({"name":"Desktop"})).unwrap();
        *fixture.host.current_app.lock().unwrap() =
            Some(crate::process::launch(&fixture.host, &app, &Default::default()).unwrap());
        assert!(!application_finished(&fixture.host));
        let client = fixture.client(u32::MAX);
        let first = fixture.session(client.clone(), Role::Stream);
        let second = fixture.session(client.clone(), Role::Stream);
        let _input = fixture.session(client, Role::InputOnly);
        assert!(!application_finished(&fixture.host));
        fixture
            .host
            .sessions
            .lock()
            .unwrap()
            .active
            .remove(&first.launch.id);
        assert!(!application_finished(&fixture.host));
        fixture
            .host
            .sessions
            .lock()
            .unwrap()
            .active
            .remove(&second.launch.id);
        assert!(application_finished(&fixture.host));
    }
}
