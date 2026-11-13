//! What the Rubylight native API tells of a PC: its capabilities, library and session, the stream that
//! moved to another device, and the session's actions.

use super::*;

/// Takes what the native API said into serverinfo's capabilities, so every gate on them
/// (shared folders, co-op, the image clipboard) sees it. Without a native read nothing changes,
/// and a PC without the native API offers none of those features either way.
pub fn merge_native(info: &mut ServerInfo, native: Option<&NativeCapabilities>) {
    if let Some(native) = native {
        info.capabilities = info.capabilities.with_native(native);
    }
}

/// How long selecting a game waits to learn who plays it, before it starts the stream as it always did.
const SELECT_READ_WAIT: Duration = Duration::from_millis(1500);
/// How often an open "Now playing on ..." asks the PC whether the move failed.
const MOVE_WATCH_EVERY: Duration = Duration::from_secs(3);
/// How many times it asks: past the two minutes the PC keeps its note, even one that never answers.
pub const MOVE_WATCH_READS: u32 = 50;

/// `work`'s answer, or an error when it takes longer than `wait`. It then ends on its own.
pub(super) fn within<T: Send + 'static>(wait: Duration, work: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    let (sender, answer) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    answer.recv_timeout(wait).unwrap_or_else(|_| Err(format!("no answer within {} ms", wait.as_millis())))
}

impl Hosts {
    /// Reads the native API of a paired, online PC that has it: the session every time, the
    /// permissions and library when they are due (or `force`). Does nothing while a read runs.
    pub fn refresh_native(&mut self, id: u64, force: bool) {
        let Some(identity) = self.identity.clone() else {
            return;
        };
        let Some(host) = self.get_mut(id) else {
            return;
        };
        let live = host.reach == Reach::Online
            && host.paired()
            && host.info.as_ref().is_some_and(|i| i.capabilities.has(Feature::NativeApi));
        if !live || host.native.loading {
            return;
        }
        let full = force
            || host.native.library.is_none()
            || !host.native.library_live
            || host.native.full_at.is_none_or(|t| t.elapsed() >= NATIVE_FULL_INTERVAL);
        host.native.loading = true;
        if full {
            host.native.full_at = Some(Instant::now());
        }
        let approves = host.native.capabilities.as_ref().is_some_and(NativeCapabilities::may_approve_pairing);
        host.native_seq += 1;
        let (epoch, seq) = (host.native_epoch, host.native_seq);
        let record = host.connect_record();
        self.spawn("native-api", move || {
            let result = client_for(&identity, &record, &record.address).and_then(|client| {
                let (capabilities, library) = if full {
                    (
                        Some(client.native_capabilities_with_files().map_err(|e| describe(&e))?),
                        Some(client.library().map_err(|e| describe(&e))?),
                    )
                } else {
                    (None, None)
                };
                let wake = capabilities.as_ref().and_then(|c| {
                    if !c.capabilities.features.iter().any(|f| f == feature::WAKE) {
                        return Some((Vec::new(), Vec::new()));
                    }
                    match client.wake() {
                        Ok(wake) => Some(crate::wol::targets(&wake)),
                        Err(e) => {
                            eprintln!("nightjar: wake targets not read: {}", describe(&e));
                            None
                        }
                    }
                });
                let update = capabilities.as_ref().and_then(|c| {
                    if !c.capabilities.features.iter().any(|f| f == nightjar_core::http::FEATURE_UPDATE) {
                        return None;
                    }
                    client.host_update().map_err(|e| eprintln!("nightjar: host update not read: {}", describe(&e))).ok()
                });
                let pairing_requests = if capabilities.as_ref().map_or(approves, NativeCapabilities::may_approve_pairing) {
                    client.pairing_requests().map(|r| r.requests).unwrap_or_else(|e| {
                        eprintln!("nightjar: devices waiting to pair not read: {}", describe(&e));
                        Vec::new()
                    })
                } else {
                    Vec::new()
                };
                let session = client.session().map_err(|e| describe(&e));
                // Without a library in this read, a failed session is the whole failure; with
                // one, Home still has the games and shows the session error beside them.
                match (&session, full) {
                    (Err(e), false) => Err(e.clone()),
                    _ => Ok(Box::new(NativeRead { capabilities, library, session, wake, update, pairing_requests })),
                }
            });
            Some(Event::Native { id, epoch, seq, result })
        });
    }

    /// Whether this PC offers moving its game's stream between devices.
    pub fn offers_continue_here(&self, id: u64) -> bool {
        self.get(id).and_then(|host| host.info.as_ref()).is_some_and(|info| {
            info.capabilities.availability(Feature::ContinueHere) == Availability::Supported
        })
    }

    /// Whether this PC makes a stream of another device's game a guest of that device
    /// (`session.guests`), as its native capabilities last said.
    pub fn offers_session_guests(&self, id: u64) -> bool {
        self.get(id)
            .and_then(|host| host.native.capabilities.as_ref())
            .is_some_and(nightjar_core::http::NativeCapabilities::offers_session_guests)
    }

    /// After a stream ended, unless the player left it on purpose: asks the PC whether it moved
    /// to another device.
    pub fn read_stream_move(&mut self, id: u64, app: i32, left_on_purpose: bool) {
        if !nightjar_policy::continue_here::reads_move_after_end(self.offers_continue_here(id), left_on_purpose, false) {
            return;
        }
        let (Some(identity), Some(host)) = (self.identity.clone(), self.get(id)) else {
            return;
        };
        let record = host.connect_record();
        self.spawn("stream-move", move || {
            let read = client_for(&identity, &record, &record.address)
                .and_then(|client| client.stream_move().map_err(|e| describe(&e)));
            let device = match read {
                Ok(read) => read.moved_to.map(|moved| moved.device),
                Err(e) => {
                    eprintln!("nightjar: could not ask the PC whether the stream moved: {e}");
                    None
                }
            };
            Some(Event::StreamMoved { id, app, device })
        });
    }

    /// While "Now playing on ..." is open: reads the PC once after [`MOVE_WATCH_EVERY`], off the UI
    /// thread, and answers with [`Event::MoveWatched`]. The next read is the answer's to start, so
    /// a closed prompt ends this. False when it cannot ask, and no answer comes.
    pub fn watch_stream_move(&mut self, id: u64, app: i32, token: u64) -> bool {
        let (Some(identity), Some(host)) = (self.identity.clone(), self.get(id)) else {
            return false;
        };
        let record = host.connect_record();
        self.spawn("stream-move-watch", move || {
            std::thread::sleep(MOVE_WATCH_EVERY);
            let read = client_for(&identity, &record, &record.address)
                .and_then(|client| client.stream_move().map_err(|e| describe(&e)));
            Some(Event::MoveWatched { token, id, app, read })
        });
        true
    }

    /// Selecting a game on a PC that offers the move: reads who plays it now, off the UI thread and
    /// for at most [`SELECT_READ_WAIT`], and answers with [`Event::SelectRead`]. False when it cannot
    /// ask, and no answer comes.
    /// `too`: the user pressed Stream here too on "did not move" (`continue_here::joins_after_read`).
    pub fn read_before_start(&mut self, id: u64, app: i32, resume: bool, too: bool) -> bool {
        let (Some(identity), Some(host)) = (self.identity.clone(), self.get(id)) else {
            return false;
        };
        let record = host.connect_record();
        self.spawn("continue-here-read", move || {
            let read = within(SELECT_READ_WAIT, move || {
                client_for(&identity, &record, &record.address).and_then(|client| client.stream_move().map_err(|e| describe(&e)))
            });
            let read = match read {
                Ok(read) => Some(read),
                Err(e) => {
                    eprintln!("nightjar: could not read who plays the game; starting the stream: {e}");
                    None
                }
            };
            Some(Event::SelectRead { id, app, resume, too, read })
        });
        true
    }

    /// Continue here: the other device's stream of the running game ends. The launch follows
    /// the PC's answer.
    pub fn continue_here(&mut self, id: u64, app: i32) {
        let (Some(identity), Some(host)) = (self.identity.clone(), self.get(id)) else {
            return;
        };
        let record = host.connect_record();
        self.spawn("continue-here", move || {
            let result = client_for(&identity, &record, &record.address)
                .and_then(|client| client.continue_here().map(|_| ()).map_err(|e| describe(&e)));
            Some(Event::ContinueHere { id, app, result })
        });
    }

    /// Runs Close game (or End, Disconnect) through the native API.
    pub fn session_action(&mut self, id: u64, action: ActionId) {
        let Some(identity) = self.identity.clone() else {
            return;
        };
        let Some(host) = self.get_mut(id) else {
            return;
        };
        if host.native.acting {
            return;
        }
        host.native.acting = true;
        let record = host.connect_record();
        self.spawn("session-action", move || {
            let result = client_for(&identity, &record, &record.address).and_then(|client| {
                client.session_action(action).map(Box::new).map_err(|e| describe(&e))
            });
            Some(Event::Session { id, action, result })
        });
    }

    /// Puts back the libraries saved by earlier runs, for PCs not reached yet.
    pub fn load_cached_libraries(&mut self, store: &Store) {
        for host in &mut self.list {
            if host.record.server_certificate.is_some() && !host.record.unique_id.is_empty() {
                let saved = store.library(&host.record.unique_id);
                host.native.last_played = saved.as_ref().map(|l| l.last_played.clone()).unwrap_or_default();
                host.native.collections = saved.as_ref().map(|l| l.collections.clone()).unwrap_or_default();
                // A PC without the native library: the app list it sent last.
                let classic = saved.is_none().then(|| store.classic_apps(&host.record.unique_id)).flatten();
                host.native.library = saved.map(|l| l.library);
                host.set_classic_apps(classic);
            }
        }
    }

    /// The app lists read since the last call, by the PC's `uniqueid`: Some to save, None to
    /// forget (the PC turned out to have the native library).
    pub fn take_classic_saves(&mut self) -> Vec<(String, Option<crate::config::ClassicApps>)> {
        std::mem::take(&mut self.classic_saves)
            .into_iter()
            .filter_map(|id| {
                let host = self.get(id)?;
                // A PC unpaired while its list was in flight must not get it back.
                host.record.server_certificate.as_ref()?;
                Some((host.record.unique_id.clone(), host.classic_apps.clone()))
            })
            .filter(|(unique_id, _)| !unique_id.is_empty())
            .collect()
    }

    /// The libraries read since the last call, by the PC's `uniqueid`, to be saved.
    pub fn take_library_saves(&mut self) -> Vec<(String, NativeLibrary)> {
        std::mem::take(&mut self.library_saves)
            .into_iter()
            .filter_map(|id| {
                let host = self.get(id)?;
                // A PC unpaired while its read was in flight must not get its library back.
                host.record.server_certificate.as_ref()?;
                let library = NativeLibrary { library: host.native.library.clone()?, last_played: host.native.last_played.clone(), collections: host.native.collections.clone() };
                Some((host.record.unique_id.clone(), library))
            })
            .filter(|(unique_id, _)| !unique_id.is_empty())
            .collect()
    }

    pub fn control_action(&mut self, id: u64, app_id: i32) {
        let Some(identity) = self.identity.clone() else { return };
        let Some(host) = self.get(id) else { return };
        let Apps::Loaded(apps) = &host.apps else { return };
        let Some(app) = apps.iter().find(|app| app.id == app_id && app.is_control_action()) else { return };
        let request = nightjar_core::http::LaunchRequest {
            app_id,
            app_uuid: app.uuid.clone(),
            role: app.role(false),
            ..Default::default()
        };
        let control = app.control;
        let record = host.connect_record();
        self.spawn("host-action", move || {
            let result = client_for(&identity, &record, &record.address).and_then(|client| {
                let info = client.server_info_https().map_err(|e| describe(&e))?;
                let apps = client.app_list(&info.capabilities).map_err(|e| describe(&e))?;
                if !apps.iter().any(|app| app.id == request.app_id && app.uuid == request.app_uuid && app.control == control) {
                    return Err(en("The PC action changed. Refresh the library and try again.").into());
                }
                let mut result = client.launch(&request, &info);
                if control == AppControl::Terminate && matches!(&result,
                    Err(nightjar_core::http::Error::Status { code: 410, message })
                    if message == "This will close the active stream but leave Remote Monitor and Remote Input connected. Launch Terminate again within 60 seconds to confirm this was intentional.") {
                    // The launcher already obtained confirmation for this action.
                    result = client.launch(&request, &info);
                }
                match result {
                    Ok(Some(_)) => Ok(()),
                    Ok(None) => Err(en("The PC declined the action.").into()),
                    Err(error) if completed_action(control, &error) => Ok(()),
                    Err(error) => Err(describe(&error)),
                }
            });
            Some(Event::Action { id, result })
        });
    }
}

/// The addresses a poll tries, in order: only the pinned one when the user pinned the PC
/// ([`Hosts::pin_address`]), otherwise the last that answered, then the others it was known by.
pub(super) fn completed_action(control: AppControl, error: &nightjar_core::http::Error) -> bool {
    let nightjar_core::http::Error::Status { code: 410, message } = error else { return false; };
    let message = message.trim().trim_end_matches('.');
    let known: &[&str] = match control {
        AppControl::DisconnectMonitor => &["Remote monitor disconnected", "Remote Monitor disconnected successfully"],
        AppControl::DisconnectInput => &["Remote input disconnected", "Remote Input disconnected successfully"],
        AppControl::Terminate => &["Application terminated", "Active stream terminated. Remote Monitor and Remote Input remain connected", "App terminated"],
        _ => &[],
    };
    known.iter().any(|known| message.eq_ignore_ascii_case(known))
}

/// Whether a cover asked for at `asked` replaces the one known at `known`. No version says nothing
/// of the cover (an app list that is not read yet has none), so it never replaces a known one.
pub(super) fn replaces(known: &str, asked: &str) -> bool {
    !asked.is_empty() && known != asked
}
