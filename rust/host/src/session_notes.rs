//! What the host noticed about the owner's game that the session machine does not track: the
//! Radeon profile it applied for the app, and a launcher showing a prompt in place of the game.
//! Both reach the client in the native API's session (`launcher`, `radeon_profile`), for the
//! owner only. A change of either moves the session's revision, so a client that polls on the
//! revision sees it.
use butterpollo_core::{
    launcher_prompt::{Watch, Window},
    native::SessionSnapshot,
    native_api::session::State,
    native_api::session::{LauncherNotice, ProfileState, RadeonProfile},
    radeon_profile::Item,
};
use std::{
    sync::{Mutex, MutexGuard, PoisonError},
    time::Duration,
};

/// A device's applied (or applying, or failed) Radeon profile.
#[derive(Clone, Debug)]
pub struct Held {
    pub generation: u64,
    pub status: RadeonProfile,
    /// What was asked for, to tell a resume of the same app from a different one.
    pub wanted: Vec<Item>,
}

#[derive(Default)]
struct Inner {
    launcher: Option<(String, LauncherNotice)>,
    /// Follows the launcher windows of the device's launched game.
    watching: Option<(String, Watch)>,
    profile: Option<(String, Held)>,
    /// Added to the machine's revision. Moves on every change of the notes.
    bump: u64,
    generation: u64,
}

#[derive(Default)]
pub struct SessionNotes {
    inner: Mutex<Inner>,
}

fn locked(mutex: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl SessionNotes {
    /// One look at the launcher windows for `device`'s game, which started `since_launch` ago;
    /// `game_up` says the game itself has shown up.
    pub fn look(
        &self,
        device: &str,
        now_unix_ms: u64,
        since_launch: Duration,
        game_up: bool,
        windows: &[Window],
    ) {
        let mut inner = locked(&self.inner);
        if inner
            .watching
            .as_ref()
            .is_none_or(|(owner, _)| owner != device)
        {
            inner.watching = Some((device.to_owned(), Watch::default()));
        }
        let notice = inner
            .watching
            .as_mut()
            .and_then(|(_, watch)| watch.look(now_unix_ms, since_launch, game_up, windows));
        let next = notice.map(|notice| (device.to_owned(), notice));
        if inner.launcher != next {
            inner.launcher = next;
            inner.bump += 1;
        }
    }
    /// No launched game is being waited for: forget the launcher and its notice.
    pub fn end_launch(&self) {
        let mut inner = locked(&self.inner);
        inner.watching = None;
        if inner.launcher.take().is_some() {
            inner.bump += 1;
        }
    }
    /// `device` starts applying a profile. Returns the generation its result must present.
    pub fn begin_profile(&self, device: &str, status: RadeonProfile, wanted: Vec<Item>) -> u64 {
        let mut inner = locked(&self.inner);
        inner.generation += 1;
        let generation = inner.generation;
        inner.profile = Some((
            device.to_owned(),
            Held {
                generation,
                status,
                wanted,
            },
        ));
        inner.bump += 1;
        generation
    }
    /// The result of the apply that `begin_profile` returned `generation` for. A newer profile
    /// or a clear in between wins.
    pub fn finish_profile(&self, device: &str, generation: u64, status: RadeonProfile) {
        let mut inner = locked(&self.inner);
        if let Some((owner, held)) = inner.profile.as_mut()
            && owner == device
            && held.generation == generation
        {
            held.status = status;
            inner.bump += 1;
        }
    }
    /// A failure with no profile to carry it: the app named one the host cannot read.
    pub fn fail_profile(&self, device: &str, profile: &str, message: String) {
        let status = RadeonProfile {
            profile: profile.to_owned(),
            state: ProfileState::Failed,
            message: Some(message),
            settings: vec![],
        };
        let generation = self.begin_profile(device, status.clone(), vec![]);
        self.finish_profile(device, generation, status);
    }
    pub fn clear_profile(&self, device: &str) {
        let mut inner = locked(&self.inner);
        if inner
            .profile
            .as_ref()
            .is_some_and(|(owner, _)| owner == device)
        {
            inner.profile = None;
            inner.bump += 1;
        }
    }
    pub fn profile(&self, device: &str) -> Option<Held> {
        locked(&self.inner)
            .profile
            .as_ref()
            .filter(|(owner, _)| owner == device)
            .map(|(_, held)| held.clone())
    }

    /// Add the notes to the snapshot of the session. They show for the owner of a session that
    /// is not idle; an applied profile shows only while `holds` says the device still has its
    /// settings changed (a session that ended restored them).
    pub fn fill(&self, snapshot: &mut SessionSnapshot, holds: impl Fn(&str) -> bool) {
        let inner = locked(&self.inner);
        snapshot.revision += inner.bump;
        let Some(owner) = snapshot.owner.as_deref() else {
            return;
        };
        if snapshot.state == State::Idle {
            return;
        }
        snapshot.launcher = inner
            .launcher
            .as_ref()
            .filter(|(device, _)| device == owner)
            .map(|(_, notice)| notice.clone());
        snapshot.radeon_profile = inner
            .profile
            .as_ref()
            .filter(|(device, held)| {
                device == owner && (held.status.state != ProfileState::Applied || holds(owner))
            })
            .map(|(_, held)| held.status.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterpollo_core::native_api::session::LauncherKind;

    fn snapshot(owner: &str, state: State) -> SessionSnapshot {
        SessionSnapshot {
            revision: 10,
            state,
            owner: Some(owner.into()),
            ..Default::default()
        }
    }
    fn status(state: ProfileState) -> RadeonProfile {
        RadeonProfile {
            profile: "quality".into(),
            state,
            message: None,
            settings: vec![],
        }
    }
    /// A look at a Steam window with `title` for `device`'s game.
    fn look(notes: &SessionNotes, device: &str, title: &str) {
        let windows = [Window {
            process: "steam.exe".into(),
            title: title.into(),
            foreground: false,
        }];
        notes.look(device, 1, Duration::from_secs(3), false, &windows);
    }
    fn notice(title: &str) -> LauncherNotice {
        LauncherNotice {
            launcher: "steam".into(),
            kind: LauncherKind::Update,
            window_title: title.into(),
            since_unix_ms: 1,
        }
    }

    #[test]
    fn the_notes_show_for_the_owner_of_a_live_session_and_move_the_revision() {
        let notes = SessionNotes::default();
        let mut plain = snapshot("me", State::Streaming);
        notes.fill(&mut plain, |_| true);
        assert_eq!(
            (plain.revision, plain.launcher, plain.radeon_profile),
            (10, None, None)
        );

        look(&notes, "me", "Updating");
        look(&notes, "me", "Updating");
        let generation = notes.begin_profile("me", status(ProfileState::Applying), vec![]);
        let mut shown = snapshot("me", State::Streaming);
        notes.fill(&mut shown, |_| true);
        assert!(shown.revision > 10);
        assert_eq!(shown.launcher, Some(notice("Updating")));
        assert_eq!(shown.radeon_profile.unwrap().state, ProfileState::Applying);

        // A result moves the revision again; the same notice again does not.
        let before = shown.revision;
        notes.finish_profile("me", generation, status(ProfileState::Applied));
        look(&notes, "me", "Updating");
        look(&notes, "me", "Updating");
        let mut after = snapshot("me", State::Streaming);
        notes.fill(&mut after, |_| true);
        assert_eq!(after.revision, before + 1);

        // Another device, or no session, shows nothing.
        let mut other = snapshot("you", State::Streaming);
        notes.fill(&mut other, |_| true);
        assert!(other.launcher.is_none() && other.radeon_profile.is_none());
        let mut idle = snapshot("me", State::Idle);
        notes.fill(&mut idle, |_| true);
        assert!(idle.launcher.is_none() && idle.radeon_profile.is_none());
    }

    #[test]
    fn the_launcher_notice_is_forgotten_when_no_game_is_waited_for() {
        let notes = SessionNotes::default();
        look(&notes, "me", "Updating");
        look(&notes, "me", "Updating");
        let mut shown = snapshot("me", State::Streaming);
        notes.fill(&mut shown, |_| true);
        let revision = shown.revision;
        assert!(shown.launcher.is_some());
        notes.end_launch();
        let mut gone = snapshot("me", State::Streaming);
        notes.fill(&mut gone, |_| true);
        assert!(gone.launcher.is_none());
        assert_eq!(gone.revision, revision + 1);
        // Nothing to forget: the revision stays.
        notes.end_launch();
        let mut again = snapshot("me", State::Streaming);
        notes.fill(&mut again, |_| true);
        assert_eq!(again.revision, gone.revision);
    }

    #[test]
    fn a_new_owner_starts_a_new_watch() {
        let notes = SessionNotes::default();
        look(&notes, "me", "Updating");
        // One look by another device is its first look, not its second.
        look(&notes, "you", "Updating");
        let mut seen = snapshot("you", State::Streaming);
        notes.fill(&mut seen, |_| true);
        assert!(seen.launcher.is_none());
    }

    #[test]
    fn an_applied_profile_hides_once_its_settings_went_back() {
        let notes = SessionNotes::default();
        let generation = notes.begin_profile("me", status(ProfileState::Applying), vec![]);
        notes.finish_profile("me", generation, status(ProfileState::Applied));
        let mut gone = snapshot("me", State::Streaming);
        notes.fill(&mut gone, |_| false);
        assert!(gone.radeon_profile.is_none());
        // A failure stays visible: nothing was written, so nothing is held.
        notes.fail_profile("me", "quality", "no GPU".into());
        let mut failed = snapshot("me", State::Streaming);
        notes.fill(&mut failed, |_| false);
        let failed = failed.radeon_profile.unwrap();
        assert_eq!(failed.state, ProfileState::Failed);
        assert_eq!(failed.message.as_deref(), Some("no GPU"));
    }

    #[test]
    fn a_late_result_of_an_older_profile_is_dropped() {
        let notes = SessionNotes::default();
        let old = notes.begin_profile("me", status(ProfileState::Applying), vec![]);
        let new = notes.begin_profile("me", status(ProfileState::Applying), vec![]);
        notes.finish_profile("me", old, status(ProfileState::Failed));
        assert_eq!(
            notes.profile("me").unwrap().status.state,
            ProfileState::Applying
        );
        notes.finish_profile("me", new, status(ProfileState::Applied));
        assert_eq!(
            notes.profile("me").unwrap().status.state,
            ProfileState::Applied
        );
        notes.clear_profile("me");
        notes.finish_profile("me", new, status(ProfileState::Failed));
        assert!(notes.profile("me").is_none());
    }
}
