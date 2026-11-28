//! Non-blocking control-loop detector and asynchronous worker for live mid-stream
//! monitor switching (`Ctrl + Alt + Shift + F1`–`F12`).

use super::super::*;
use super::peer::ControlPeer;

/// Guardrail checking whether session `s` under effective `config` may switch monitors.
pub(in crate::stream) fn monitor_switch_allowed(s: &Session, config: &Config) -> bool {
    s.launch.role == Role::Stream
        && s.launch.nucleus.is_none()
        && s.launch.guest().is_none()
        && s.launch
            .client
            .allows(butterpollo_input_policy::input::KEYBOARD_PERMISSION)
        && !butterpollo_session::background::enabled(config)
}

/// Filters queued keyboard events for `p` before couch-menu handling and queues any
/// matched monitor switch non-blockingly for the background worker.
pub(super) fn filter_input(
    h: &Shared,
    p: &mut ControlPeer,
    s: &Arc<Session>,
    monitor_switches: &std::sync::mpsc::SyncSender<(Arc<Session>, u8)>,
) {
    if butterpollo_input_policy::couch_menu::Shared::is_open(s.couch_menu.load()) {
        p.monitor_switch.reset();
        return;
    }
    if !p
        .inputs
        .iter()
        .any(|event| matches!(event, input::Input::Keyboard { .. }))
    {
        return;
    }
    let allowed = match p.config(h, s) {
        Ok(config) => monitor_switch_allowed(s, &config),
        Err(error) => {
            tracing::debug!(%error, "effective config unavailable for monitor switch check");
            false
        }
    };
    p.inputs.retain(|event| {
        let input::Input::Keyboard {
            key,
            modifiers,
            down,
            ..
        } = *event
        else {
            return true;
        };
        match p.monitor_switch.on_key(key, modifiers, down, allowed) {
            butterpollo_input_policy::monitor_switch::Action::Pass => true,
            butterpollo_input_policy::monitor_switch::Action::Swallow => false,
            butterpollo_input_policy::monitor_switch::Action::Switch(idx) => {
                let _ = monitor_switches.try_send((s.clone(), idx));
                false
            }
        }
    });
}

/// Spawns the worker thread servicing matched monitor-switch requests off the ENet loop.
pub(super) fn spawn_worker(
    h: &Shared,
    control_wake: &Arc<UdpSocket>,
) -> Result<std::sync::mpsc::SyncSender<(Arc<Session>, u8)>> {
    let (tx, rx) = std::sync::mpsc::sync_channel::<(Arc<Session>, u8)>(16);
    let host = h.clone();
    let wake = control_wake.clone();
    thread::Builder::new()
        .name("monitor switch".into())
        .spawn(move || {
            for (session, index) in rx {
                if host.stop.load(Ordering::Acquire) || session.stopping() {
                    continue;
                }
                switch_monitor(&host, &wake, &session, index);
            }
        })?;
    Ok(tx)
}

fn switch_monitor(h: &Shared, control_wake: &Arc<UdpSocket>, s: &Arc<Session>, index: u8) {
    let Ok(config) = effective_config(h, &s.launch) else {
        return;
    };
    if !monitor_switch_allowed(s, &config) {
        return;
    }
    let prepared = {
        let mut targets = h.monitor_targets.lock().unwrap();
        targets.retain(|_, target| target.strong_count() > 0);
        targets.get(&s.launch.id).and_then(Weak::upgrade)
    };

    #[cfg(any(test, feature = "test-support"))]
    let fixture_displays = h
        .reconnect_fixture
        .as_ref()
        .map(|f| f.monitor_displays.lock().unwrap().clone());
    #[cfg(not(any(test, feature = "test-support")))]
    let fixture_displays: Option<Vec<String>> = None;

    let active: Vec<String> = if let Some(displays) = fixture_displays {
        displays
    } else {
        let enumerated = butterpollo_win_io::input::on_input_desktop(|| {
            butterpollo_win_graphics::capture::displays()
        });
        match enumerated {
            Ok(list) => list.into_iter().map(|d| d.display_name).collect(),
            Err(error) => {
                tracing::warn!(
                    client = %s.launch.client.name,
                    %error,
                    "cannot enumerate active displays for monitor switch"
                );
                return;
            }
        }
    };

    let Some(selected) = butterpollo_media_policy::monitor_switch::select_display(&active, index)
    else {
        return;
    };

    if let Some(prepared) = prepared {
        apply_selection(h, control_wake, s, Some(&prepared), &selected);
        return;
    }

    #[cfg(any(test, feature = "test-support"))]
    if let Some(fixture) = &h.reconnect_fixture {
        fixture.monitor_switches.lock().unwrap().push(selected.clone());
        apply_selection(h, control_wake, s, None, &selected);
    }
}

fn apply_selection(
    h: &Shared,
    control_wake: &Arc<UdpSocket>,
    s: &Arc<Session>,
    prepared: Option<&crate::display_session::Ready>,
    selected: &str,
) {
    let target_output = if let Some(prepared) = prepared {
        let prepared_display = prepared.prepared_display_target().0;
        let next_override = butterpollo_media_policy::monitor_switch::override_for_selection(
            selected,
            &prepared_display,
        );
        let changed = prepared.set_monitor_override(next_override.clone());
        let output = prepared.output();
        tracing::info!(
            client = %s.launch.client.name,
            selected = %selected,
            prepared = %prepared_display,
            overridden = next_override.is_some(),
            changed,
            "switched stream monitor"
        );
        output
    } else {
        selected.to_owned()
    };
    update_session_outputs(h, control_wake, s, &target_output, selected);
}

fn update_session_outputs(
    h: &Shared,
    control_wake: &Arc<UdpSocket>,
    s: &Arc<Session>,
    target_output: &str,
    cursor_display: &str,
) {
    *s.output.write().unwrap() = target_output.to_owned();
    s.client_cursor.follow_output(cursor_display);
    let guests: Vec<Arc<Session>> = h
        .sessions
        .lock()
        .unwrap()
        .active
        .values()
        .filter(|other| other.launch.guest().is_some() && !other.stopping())
        .cloned()
        .collect();
    for guest in guests {
        *guest.output.write().unwrap() = target_output.to_owned();
        guest.client_cursor.follow_output(cursor_display);
    }
    let _ = control_wake.send(&[1]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterpollo_input_policy::monitor_switch::{
        MODIFIER_ALT, MODIFIER_CTRL, MODIFIER_SHIFT, VK_CONTROL, VK_F1, VK_MENU, VK_SHIFT,
    };

    #[test]
    fn filter_input_queues_switches_swallows_fkeys_and_ignores_open_couch_menu() {
        let f = crate::state::test_support::Fixture::new();
        let client = f.client(u32::MAX);
        let launch = f.launch(client, Role::Stream);
        let session = Arc::new(Session::new(launch.clone(), Default::default()));
        let mut peer = ControlPeer::new(
            &f.host,
            &launch,
            rusty_enet::PeerID(0),
            Some(session.clone()),
            &ControlSeqMemory::default(),
        );
        let (tx, rx) = std::sync::mpsc::sync_channel(16);

        let f2 = VK_F1 + 1;
        peer.inputs = vec![
            input::Input::Keyboard {
                key: VK_CONTROL,
                modifiers: 0,
                flags: 0,
                down: true,
            },
            input::Input::Keyboard {
                key: VK_MENU,
                modifiers: MODIFIER_CTRL,
                flags: 0,
                down: true,
            },
            input::Input::Keyboard {
                key: VK_SHIFT,
                modifiers: MODIFIER_CTRL | MODIFIER_ALT,
                flags: 0,
                down: true,
            },
            input::Input::Keyboard {
                key: f2,
                modifiers: MODIFIER_CTRL | MODIFIER_ALT | MODIFIER_SHIFT,
                flags: 0,
                down: true,
            },
            input::Input::Keyboard {
                key: f2,
                modifiers: MODIFIER_CTRL | MODIFIER_ALT | MODIFIER_SHIFT,
                flags: 0,
                down: true,
            },
            input::Input::Keyboard {
                key: f2,
                modifiers: 0,
                flags: 0,
                down: false,
            },
        ];
        filter_input(&f.host, &mut peer, &session, &tx);
        assert_eq!(peer.inputs.len(), 3);
        assert!(peer.inputs.iter().all(|e| matches!(
            e,
            input::Input::Keyboard { key, .. } if matches!(*key, VK_CONTROL | VK_MENU | VK_SHIFT)
        )));
        let (_, idx) = rx.try_recv().expect("switch 2 should be queued");
        assert_eq!(idx, 2);
        assert!(rx.try_recv().is_err());

        // While the couch menu is open, Ctrl+Alt+Shift+F2 passes through untouched.
        session.couch_menu.open().unwrap();
        peer.inputs = vec![input::Input::Keyboard {
            key: f2,
            modifiers: MODIFIER_CTRL | MODIFIER_ALT | MODIFIER_SHIFT,
            flags: 0,
            down: true,
        }];
        filter_input(&f.host, &mut peer, &session, &tx);
        assert_eq!(peer.inputs.len(), 1);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn apply_selection_updates_override_owner_and_guest_outputs_only() {
        let f = crate::state::test_support::Fixture::new();
        let owner_client = f.client(u32::MAX);
        let owner_launch = f.launch(owner_client, Role::Stream);
        let owner = Arc::new(Session::new(owner_launch.clone(), Default::default()));

        let guest_client = f.client(u32::MAX);
        let mut guest_launch = f.launch(guest_client, Role::Stream);
        guest_launch.options.insert(
            butterpollo_session::multiclient::LAUNCH_OPTION.into(),
            "1".into(),
        );
        let guest = Arc::new(Session::new(guest_launch.clone(), Default::default()));

        let mut remote_client = f.client(u32::MAX);
        remote_client.uuid = "remote-monitor".into();
        let remote_launch = f.launch(remote_client, Role::RemoteMonitor);
        let remote = Arc::new(Session::new(remote_launch.clone(), Default::default()));

        *owner.output.write().unwrap() = r"\\.\DISPLAY1".into();
        *guest.output.write().unwrap() = r"\\.\DISPLAY1".into();
        *remote.output.write().unwrap() = r"\\.\DISPLAY1".into();

        let ready = crate::display_session::test_support::retained_output(
            &f.host,
            &Config::default(),
            r"\\.\DISPLAY1",
        );
        {
            let mut sessions = f.host.sessions.lock().unwrap();
            sessions.active.insert(owner_launch.id.clone(), owner.clone());
            sessions.active.insert(guest_launch.id.clone(), guest.clone());
            sessions.active.insert(remote_launch.id.clone(), remote.clone());
        }

        let wake_rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        wake_rx.set_nonblocking(true).unwrap();
        let wake_tx = Arc::new(UdpSocket::bind("127.0.0.1:0").unwrap());
        wake_tx.connect(wake_rx.local_addr().unwrap()).unwrap();

        apply_selection(&f.host, &wake_tx, &owner, Some(&ready), r"\\.\DISPLAY2");
        assert!(ready.has_monitor_override());
        assert_eq!(ready.output(), r"\\.\DISPLAY2");
        assert_eq!(*owner.output.read().unwrap(), r"\\.\DISPLAY2");
        assert_eq!(*guest.output.read().unwrap(), r"\\.\DISPLAY2");
        assert_eq!(*remote.output.read().unwrap(), r"\\.\DISPLAY1");
        assert!(wake_rx.recv(&mut [0; 4]).is_ok());

        // Switching back to the prepared display clears the override cleanly.
        apply_selection(&f.host, &wake_tx, &owner, Some(&ready), r"\\.\DISPLAY1");
        assert!(!ready.has_monitor_override());
        assert_eq!(ready.output(), r"\\.\DISPLAY1");
        assert_eq!(*owner.output.read().unwrap(), r"\\.\DISPLAY1");
        assert_eq!(*guest.output.read().unwrap(), r"\\.\DISPLAY1");
        assert_eq!(*remote.output.read().unwrap(), r"\\.\DISPLAY1");
    }
}
