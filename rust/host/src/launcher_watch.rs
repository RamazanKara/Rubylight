//! A launcher showing a prompt instead of the game (plan item B5). While a launched app's game
//! has not shown up, the visible windows are read every couple of seconds; a Steam, Epic, EA or
//! Ubisoft window that wants an update or a sign in (`butterpollo_core::launcher_prompt`) is put
//! in the owner's session as `launcher`, and goes when the window does or the game appears.
//! Reading only: the host never clicks or types into a launcher.
use crate::state::Shared;
use butterpollo_core::launcher_prompt::{Window, launcher_of};
use std::{
    collections::HashMap,
    time::{Duration, SystemTime},
};

/// How often the windows are read.
pub const EVERY: Duration = Duration::from_secs(2);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The launcher windows now on the desktop. Other programs' windows are not even named: the
/// program of each window is looked up and only a launcher's is kept.
fn launcher_windows() -> Vec<Window> {
    let mut names: HashMap<u32, Option<String>> = HashMap::new();
    butterpollo_windows::window_list::visible()
        .into_iter()
        .filter_map(|window| {
            let program = names
                .entry(window.pid)
                .or_insert_with(|| {
                    butterpollo_windows::process::image_path(window.pid)
                        .map(|path| path.rsplit(['/', '\\']).next().unwrap_or(&path).to_owned())
                })
                .clone()?;
            launcher_of(&program)?;
            Some(Window {
                process: program,
                title: window.title,
                foreground: window.foreground,
            })
        })
        .collect()
}

/// One look, from the housekeeping loop.
pub fn watch(h: &Shared) {
    look(h, launcher_windows);
}

/// One look with `windows` as the way to read the desktop, which it calls only when there is a
/// game still to wait for.
pub fn look(h: &Shared, windows: impl FnOnce() -> Vec<Window>) {
    let progress = h.current_app.lock().unwrap().as_ref().and_then(|app| {
        let (game_up, age) = app.launch_progress()?;
        (!app.owner.is_empty()).then(|| (app.owner.clone(), game_up, age))
    });
    let Some((owner, game_up, age)) = progress else {
        h.notes.end_launch();
        return;
    };
    let windows = if game_up { vec![] } else { windows() };
    h.notes.look(&owner, now_ms(), age, game_up, &windows);
}
