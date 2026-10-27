//! Background streaming in a Windows child session.
//!
//! Windows Pro and higher can run one child session per user: a second
//! sign-in of the same account inside the user's own session, with its own
//! desktop, its own input focus and its own apps. A viewer in the user's
//! session signs it in through the RDP client over loopback and keeps it
//! attached (a child session only has a display while it is attached), and a
//! second host instance streams that desktop while the person at the PC keeps
//! using theirs. Windows Home has no child sessions; it keeps the isolated
//! virtual display of [`crate::background`].
//!
//! Ported from Annex (mattxslv/annex, MIT, Copyright (c) 2026 mattxslv; the
//! license text is in `third-party/licenses/annex-LICENSE.txt`): the request
//! gate and confirmation (`src/LauncherPolicy.cs`), the end-session checks and
//! display settings (`src/Annex.Core.cs`), the peer rule
//! (`src/SessionAgent.cs`), the viewer's poll loop
//! (`src/LauncherApplication.cs`) and the Windows settings that setup changes
//! and uninstall restores (`src/DesktopSetup.cs`, `scripts/setup.ps1`,
//! `scripts/setup-admin.ps1`).
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{fmt, path::PathBuf};
use zeroize::Zeroizing;

/// The host setting that picks the background streaming mode.
pub const MODE_KEY: &str = "background_mode";
/// Set in the child-session host's own configuration, so it never tries to
/// start a child session of its own.
pub const CHILD_HOST_KEY: &str = "child_session_host";
/// The child session's resolution (`WIDTHxHEIGHT`) and scale (%).
pub const RESOLUTION_KEY: &str = "child_session_resolution";
pub const SCALE_KEY: &str = "child_session_scale";
/// The child-session host's base port: the Moonlight default plus 1000, clear
/// of the main host (47984-48010), the isolated test host (48118-48144) and
/// the release harness (48518-48544).
pub const CHILD_BASE_PORT: u16 = 48989;
/// The child-session host's WebRTC ports, beside the main host's 8000/8001.
pub const CHILD_WEBRTC_PORTS: (u16, u16) = (8100, 8101);

/// What the host setting asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeSetting {
    Auto,
    ChildSession,
    VirtualDisplay,
}
impl ModeSetting {
    pub fn parse(text: &str) -> Result<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Ok(Self::Auto),
            "child_session" => Ok(Self::ChildSession),
            "virtual_display" => Ok(Self::VirtualDisplay),
            other => bail!(
                "{MODE_KEY} = {other} is not a mode: use auto, child_session or virtual_display"
            ),
        }
    }
}

/// The mode that runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// A Windows child session with its own host instance.
    ChildSession,
    /// The isolated virtual display with faked focus ([`crate::background`]).
    VirtualDisplay,
    /// This host runs inside the child session itself.
    InsideChildSession,
}

/// The Windows edition, from `EditionID` under
/// `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edition {
    Home,
    /// Pro, Pro for Workstations, Education, Enterprise, IoT Enterprise, Server.
    ProOrHigher,
    Unknown,
}
pub fn edition(edition_id: &str) -> Edition {
    let id = edition_id.trim().to_ascii_lowercase();
    if id.starts_with("core") {
        Edition::Home
    } else if [
        "professional",
        "education",
        "enterprise",
        "iotenterprise",
        "server",
    ]
    .iter()
    .any(|prefix| id.starts_with(prefix))
    {
        Edition::ProOrHigher
    } else {
        Edition::Unknown
    }
}

/// Which mode runs and why, for the log and the console.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModeDecision {
    pub mode: Mode,
    /// What the console shows, e.g. "Background streaming (virtual display): Windows Home".
    pub label: String,
    pub reason: String,
}

pub fn decide(setting: ModeSetting, edition_id: &str, inside_child_session: bool) -> ModeDecision {
    let shown = if edition_id.trim().is_empty() {
        "unknown"
    } else {
        edition_id.trim()
    };
    let decision = |mode, label: &str, reason: String| ModeDecision {
        mode,
        label: label.into(),
        reason,
    };
    if inside_child_session {
        return decision(
            Mode::InsideChildSession,
            "Background streaming: this host runs inside the child session",
            format!("{CHILD_HOST_KEY} is set: this is the child session's own host"),
        );
    }
    let child = "Background streaming (child session): Windows Pro or higher";
    let virtual_display = "Background streaming (virtual display)";
    match (setting, edition(edition_id)) {
        (ModeSetting::VirtualDisplay, _) => decision(
            Mode::VirtualDisplay,
            virtual_display,
            format!("{MODE_KEY} = virtual_display (Windows edition {shown})"),
        ),
        (_, Edition::Home) => decision(
            Mode::VirtualDisplay,
            "Background streaming (virtual display): Windows Home",
            format!(
                "Windows edition {shown} has no child sessions, so background streams use the isolated virtual display"
            ),
        ),
        (ModeSetting::Auto, Edition::ProOrHigher) => decision(
            Mode::ChildSession,
            child,
            format!("Windows edition {shown} supports child sessions ({MODE_KEY} = auto)"),
        ),
        (ModeSetting::ChildSession, _) => decision(
            Mode::ChildSession,
            child,
            format!("{MODE_KEY} = child_session (Windows edition {shown})"),
        ),
        (ModeSetting::Auto, Edition::Unknown) => decision(
            Mode::VirtualDisplay,
            virtual_display,
            format!(
                "Windows edition {shown} is not known to have child sessions; set {MODE_KEY} = child_session to use them"
            ),
        ),
    }
}

/// The child session's desktop size and scale (Annex's `DisplaySettings`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplaySettings {
    pub width: u32,
    pub height: u32,
    /// Windows scaling in percent.
    pub scale: u32,
}
impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            scale: 100,
        }
    }
}
impl DisplaySettings {
    pub fn parse(resolution: &str, scale: &str) -> Result<Self> {
        let defaults = Self::default();
        let (width, height) = if resolution.trim().is_empty() {
            (defaults.width, defaults.height)
        } else {
            let (w, h) = resolution
                .trim()
                .to_ascii_lowercase()
                .split_once('x')
                .map(|(w, h)| (w.trim().parse::<u32>(), h.trim().parse::<u32>()))
                .with_context(|| format!("{RESOLUTION_KEY} must look like 1920x1080"))?;
            (
                w.with_context(|| format!("{RESOLUTION_KEY}: the width is not a number"))?,
                h.with_context(|| format!("{RESOLUTION_KEY}: the height is not a number"))?,
            )
        };
        let scale = if scale.trim().is_empty() {
            defaults.scale
        } else {
            scale
                .trim()
                .trim_end_matches('%')
                .trim()
                .parse::<u32>()
                .with_context(|| format!("{SCALE_KEY} must be a percentage"))?
        };
        if !(200..=8192).contains(&width) || !(200..=8192).contains(&height) {
            bail!("{RESOLUTION_KEY} must be between 200x200 and 8192x8192");
        }
        if !(100..=500).contains(&scale) {
            bail!("{SCALE_KEY} must be between 100 and 500 %");
        }
        Ok(Self {
            width,
            height,
            scale,
        })
    }
    /// The RDP client's `DesktopScaleFactor` and `DeviceScaleFactor`; Annex
    /// pairs every scale with device scale 180.
    pub fn rdp_scale_factors(&self) -> (u32, u32) {
        (self.scale, 180)
    }
}

/// A session the viewer may attach to and control: a real session that is
/// neither the caller's own nor the console's.
pub fn is_target(target: u32, current: u32, console: u32) -> bool {
    target > 0 && target <= i32::MAX as u32 && target != current && target != console
}

/// Requests are stamped with a generation and only accepted while the same
/// child session is attached (Annex's `SessionRequestGate`).
#[derive(Debug, Default)]
pub struct RequestGate {
    child: u32,
    connected: bool,
    closed: bool,
    selection: String,
    generation: u64,
}
impl RequestGate {
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn update(&mut self, session: u32, attached: bool) {
        if self.child != session || self.connected != attached {
            self.child = session;
            self.connected = attached;
            self.generation += 1;
        }
    }
    pub fn accepts(&self, generation: u64, session: u32) -> bool {
        !self.closed && self.connected && session == self.child && generation == self.generation
    }
    pub fn select(&mut self, identity: &str) {
        if self.selection != identity {
            self.selection = identity.into();
            self.generation += 1;
        }
    }
    pub fn close(&mut self) {
        self.closed = true;
        self.generation += 1;
    }
}

/// An approval belongs to the connection it was given for, even when Windows
/// later reuses the same numeric child-session ID (Annex's `SessionConfirmation`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Confirmation {
    generation: u64,
    child: u32,
}
impl Confirmation {
    pub fn new(gate: &RequestGate, child: u32) -> Self {
        Self {
            generation: gate.generation(),
            child,
        }
    }
    /// The confirmation the console sends back: the child session and
    /// generation of the status the user confirmed.
    pub fn from_status(child: u32, generation: u64) -> Self {
        Self { generation, child }
    }
    pub fn child(&self) -> u32 {
        self.child
    }
    pub fn can_proceed(&self, gate: &RequestGate, current_child: u32) -> bool {
        current_child == self.child && gate.accepts(self.generation, self.child)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndError {
    /// A different child session is attached now than the one confirmed.
    Changed,
    /// No child session of this viewer to sign out.
    NoChild,
}
impl fmt::Display for EndError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Changed => {
                "The child session changed after confirmation. Review it before signing out."
            }
            Self::NoChild => "No child session belonging to this viewer is available to sign out.",
        })
    }
}
impl std::error::Error for EndError {}

/// The session to sign out, checked again right before the sign-out
/// (Annex's `ChildSessionController.EndConfirmedSession`). The no-child
/// sentinel never becomes `WTS_CURRENT_SESSION`, and the caller's own and the
/// console session are never signed out.
pub fn end_confirmed_session(
    approved: u32,
    actual: u32,
    current: u32,
    console: u32,
) -> Result<u32, EndError> {
    if actual != approved {
        return Err(EndError::Changed);
    }
    if actual == u32::MAX || actual == 0 || actual == current || actual == console {
        return Err(EndError::NoChild);
    }
    Ok(actual)
}

/// The RDP client's `Connected` property.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RdpState {
    Disconnected,
    Connected,
    Connecting,
}
impl RdpState {
    pub fn from_connected(value: i32) -> Self {
        match value {
            0 => Self::Disconnected,
            1 => Self::Connected,
            _ => Self::Connecting,
        }
    }
}
/// `ExtendedDisconnectReason` values that mean the child session signed out:
/// `exDiscReasonAPIInitiatedLogoff` and `exDiscReasonLogoffByUser`.
pub const SIGNED_OUT_REASONS: [i32; 2] = [2, 12];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Idle,
    NeedsPassword,
    Connecting,
    Connected,
    Reconnecting,
    SigningOut,
    SignInFailed,
    SignedOut,
}

/// What the viewer has to do with the RDP client and the saved password.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewerAction {
    Connect,
    Disconnect,
    /// The newly entered password signed in: save it now.
    SavePassword,
    /// The newly entered password did not sign in: drop it, never save it.
    DropPassword,
    SignOut(u32),
}

/// After a disconnect the viewer reconnects after this long.
pub const RECONNECT_MS: u64 = 6000;
/// A newly entered password that has not reached the desktop after this long failed.
pub const SIGN_IN_MS: u64 = 3000;
/// A signed-out desktop the RDP client does not drop is disconnected after this long.
pub const SIGN_OUT_MS: u64 = 15000;

/// The viewer's state machine, ported from Annex's `LauncherApplication`
/// (`Connect`, `Reconnect`, `Poll`, `SignedOut`, `SignInFailed`,
/// `ReturnToStart`, `EndSession`). Times are milliseconds on a monotonic clock.
#[derive(Debug)]
pub struct Viewer {
    gate: RequestGate,
    phase: Phase,
    message: String,
    child: u32,
    last_state: RdpState,
    /// When the last connect started; None when a reconnect is due at once.
    last_attempt: Option<u64>,
    sign_out_started: u64,
    want_reconnect: bool,
    ending: bool,
    signing_out: bool,
    password_unverified: bool,
    reached_desktop: bool,
}
impl Default for Viewer {
    fn default() -> Self {
        Self {
            gate: RequestGate::default(),
            phase: Phase::Idle,
            message: "Start the child session's desktop to stream it.".into(),
            child: 0,
            last_state: RdpState::Disconnected,
            last_attempt: None,
            sign_out_started: 0,
            want_reconnect: false,
            ending: false,
            signing_out: false,
            password_unverified: false,
            reached_desktop: false,
        }
    }
}
impl Viewer {
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn message(&self) -> &str {
        &self.message
    }
    /// The attached child session, 0 when none.
    pub fn child(&self) -> u32 {
        self.child
    }
    pub fn gate(&self) -> &RequestGate {
        &self.gate
    }
    fn set(&mut self, phase: Phase, message: &str) {
        self.phase = phase;
        self.message = message.into();
    }
    /// Start the desktop. `password` is None when none is saved or entered,
    /// `Some(true)` for a newly entered one, which is saved only once it has
    /// signed in, so a typo is never remembered.
    pub fn connect(&mut self, now: u64, state: RdpState, password: Option<bool>) -> Vec<ViewerAction> {
        if self.ending || state != RdpState::Disconnected {
            return Vec::new();
        }
        let Some(new) = password else {
            self.want_reconnect = false;
            self.set(
                Phase::NeedsPassword,
                "Enter your Windows password (not your PIN) in the console to start the child session.",
            );
            return Vec::new();
        };
        if new {
            self.password_unverified = true;
        }
        self.reached_desktop = false;
        self.last_attempt = Some(now);
        self.want_reconnect = true;
        self.set(
            Phase::Connecting,
            "Opening the child session's desktop. This takes a few seconds.",
        );
        vec![ViewerAction::Connect]
    }
    pub fn reconnect(&mut self, now: u64, state: RdpState, password: Option<bool>) -> Vec<ViewerAction> {
        if self.ending {
            return Vec::new();
        }
        self.gate.update(0, false);
        self.child = 0;
        if state != RdpState::Disconnected {
            self.want_reconnect = true;
            self.last_attempt = None;
            return vec![ViewerAction::Disconnect];
        }
        self.connect(now, state, password)
    }
    /// Stop the desktop: detach the viewer. The child session and its apps
    /// keep running, without a display, until it attaches again.
    pub fn detach(&mut self, state: RdpState) -> Vec<ViewerAction> {
        self.want_reconnect = false;
        self.gate.update(0, false);
        self.child = 0;
        self.set(
            Phase::Idle,
            "The viewer is detached. The child session's apps keep running; start the desktop to attach it again.",
        );
        if state == RdpState::Disconnected {
            Vec::new()
        } else {
            vec![ViewerAction::Disconnect]
        }
    }
    /// One poll of the RDP client. `found` is the child-session ID Windows
    /// reports for the viewer's session, `current` the viewer's own session.
    pub fn poll(
        &mut self,
        now: u64,
        state: RdpState,
        disconnect_reason: Option<i32>,
        found: Option<u32>,
        current: u32,
        console: u32,
    ) -> Vec<ViewerAction> {
        let mut actions = Vec::new();
        if self.signing_out {
            if state != RdpState::Disconnected
                && now.saturating_sub(self.sign_out_started) > SIGN_OUT_MS
            {
                actions.push(ViewerAction::Disconnect);
            }
            if state == RdpState::Disconnected {
                self.return_to_start();
            }
            return actions;
        }
        if self.ending {
            return actions;
        }
        if state == RdpState::Disconnected
            && self.last_state != RdpState::Disconnected
            && disconnect_reason.is_some_and(|reason| SIGNED_OUT_REASONS.contains(&reason))
        {
            self.last_state = state;
            self.return_to_start();
            return actions;
        }
        if state == RdpState::Disconnected
            && self.password_unverified
            && !self.reached_desktop
            && self
                .last_attempt
                .is_some_and(|started| now.saturating_sub(started) > SIGN_IN_MS)
        {
            self.last_state = state;
            self.password_unverified = false;
            self.want_reconnect = false;
            self.set(
                Phase::SignInFailed,
                "Windows didn't accept that password. Use your Windows account password, not your Windows Hello PIN. Nothing was saved.",
            );
            actions.push(ViewerAction::DropPassword);
            return actions;
        }
        if state == RdpState::Connected && !self.reached_desktop {
            self.reached_desktop = true;
            if self.password_unverified {
                self.password_unverified = false;
                actions.push(ViewerAction::SavePassword);
            }
        }
        self.last_state = state;
        let actual = match (state, found) {
            (RdpState::Connected, Some(found)) if is_target(found, current, console) => found,
            _ => 0,
        };
        self.gate
            .update(actual, state == RdpState::Connected && actual != 0);
        self.child = actual;
        match state {
            RdpState::Connected => self.set(
                Phase::Connected,
                "The child session is attached. You can keep using your own desktop.",
            ),
            RdpState::Connecting => self.set(Phase::Connecting, "Windows is opening the child session's desktop."),
            RdpState::Disconnected if self.want_reconnect => self.set(
                Phase::Reconnecting,
                "The viewer disconnected. The child session's apps keep running; reconnecting.",
            ),
            RdpState::Disconnected => {}
        }
        if state == RdpState::Disconnected
            && self.want_reconnect
            && self
                .last_attempt
                .is_none_or(|started| now.saturating_sub(started) > RECONNECT_MS)
        {
            self.last_attempt = Some(now);
            actions.push(ViewerAction::Connect);
        }
        actions
    }
    /// The confirmation an "End session" prompt carries.
    pub fn confirm(&self) -> Confirmation {
        Confirmation::new(&self.gate, self.child)
    }
    /// Sign out the confirmed child session; `actual` is the ID Windows reports now.
    pub fn end_session(
        &mut self,
        now: u64,
        approval: Confirmation,
        actual: u32,
        current: u32,
        console: u32,
    ) -> Result<Vec<ViewerAction>, EndError> {
        if self.ending || !approval.can_proceed(&self.gate, self.child) {
            return Err(EndError::Changed);
        }
        let child = end_confirmed_session(approval.child(), actual, current, console)?;
        self.ending = true;
        self.want_reconnect = false;
        self.gate.update(0, false);
        self.signing_out = true;
        self.sign_out_started = now;
        self.set(
            Phase::SigningOut,
            "Signing out the child session and closing the apps running in it.",
        );
        Ok(vec![ViewerAction::SignOut(child)])
    }
    /// Windows refused the sign-out: back to the attached desktop.
    pub fn end_failed(&mut self, error: &str) {
        self.ending = false;
        self.signing_out = false;
        self.want_reconnect = true;
        self.set(Phase::Connected, &format!("Could not sign out: {error}"));
    }
    fn return_to_start(&mut self) {
        self.signing_out = false;
        self.ending = false;
        self.want_reconnect = false;
        self.last_state = RdpState::Disconnected;
        self.child = 0;
        self.gate.update(0, false);
        self.set(
            Phase::SignedOut,
            "The child session was signed out. Start a new one when you're ready.",
        );
    }
}

/// A Windows password. It never prints: `Debug` and `Display` show
/// `[redacted]`, it has no `Serialize` (only [`ViewerCommand`]'s wire
/// encoding writes it), and its memory is wiped on drop.
#[derive(Clone)]
pub struct Secret(Zeroizing<String>);
impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::new)
    }
}
impl Secret {
    pub fn new(text: String) -> Self {
        Self(Zeroizing::new(text))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}
impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

/// A command from the host to the viewer: one JSON line on the viewer's
/// standard input, a pipe only the host holds.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum ViewerCommand {
    /// Attach the child session's desktop; `password` is a newly entered one,
    /// None to use the saved one.
    Start {
        display: DisplaySettings,
        #[serde(default, with = "wire_secret")]
        password: Option<Secret>,
    },
    /// Detach the viewer; the child session keeps running.
    Stop,
    Reconnect,
    /// Bring the viewer window up to use the child session's desktop.
    Show,
    /// Sign out the child session the console showed (`child` and
    /// `generation` from the status the user confirmed).
    EndSession { child: u32, generation: u64 },
    ForgetPassword,
    Quit,
}
impl ViewerCommand {
    /// The line written to the viewer's pipe; it holds the password, so it is
    /// wiped after the write and never logged.
    pub fn line(&self) -> Result<Zeroizing<String>> {
        let mut line = Zeroizing::new(serde_json::to_string(self)?);
        line.push('\n');
        Ok(line)
    }
}
mod wire_secret {
    use super::Secret;
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(value: &Option<Secret>, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(secret) => serializer.serialize_some(secret.expose()),
            None => serializer.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Secret>, D::Error> {
        Option::<Secret>::deserialize(deserializer)
    }
}

/// What the viewer reports: one JSON line on its standard output after every
/// change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewerStatus {
    pub phase: Phase,
    pub message: String,
    /// The attached child session, 0 when none.
    pub child: u32,
    /// The request generation an "End session" confirmation carries.
    pub generation: u64,
    pub password_saved: bool,
    /// A problem the viewer hit that the console should show (a setting the
    /// RDP client refused, a password that could not be saved).
    #[serde(default)]
    pub notice: Option<String>,
}

/// Encryption bound to the Windows user (DPAPI's `CryptProtectData` with the
/// current user's key); injectable for tests.
pub trait Dpapi {
    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>>;
    fn unprotect(&self, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>>;
}

/// The saved Windows password: one DPAPI blob in the user's own profile.
pub struct PasswordStore<D> {
    path: PathBuf,
    dpapi: D,
}
impl<D: Dpapi> PasswordStore<D> {
    pub fn new(path: PathBuf, dpapi: D) -> Self {
        Self { path, dpapi }
    }
    pub fn save(&self, password: &Secret) -> Result<()> {
        if password.is_empty() {
            bail!("an empty password is not saved");
        }
        let blob = self.dpapi.protect(password.expose().as_bytes())?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let temporary = self.path.with_extension("tmp");
        std::fs::write(&temporary, blob)
            .with_context(|| format!("writing {}", temporary.display()))?;
        std::fs::rename(&temporary, &self.path)
            .with_context(|| format!("saving {}", self.path.display()))
    }
    /// None when no password is saved; an error when one is saved but can't be read.
    pub fn load(&self) -> Result<Option<Secret>> {
        let blob = match std::fs::read(&self.path) {
            Ok(blob) => blob,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", self.path.display()));
            }
        };
        let plain = self
            .dpapi
            .unprotect(&blob)
            .context("the saved password can't be decrypted for this Windows user; forget it and enter it again")?;
        let text = String::from_utf8(plain.to_vec())
            .map_err(|_| anyhow::anyhow!("the saved password is damaged; forget it and enter it again"))?;
        Ok(Some(Secret::new(text)))
    }
    /// Forget the saved password; false when none was saved.
    pub fn forget(&self) -> Result<bool> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error).with_context(|| format!("removing {}", self.path.display())),
        }
    }
    pub fn saved(&self) -> bool {
        self.path.is_file()
    }
}

/// Where a registry value lives.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Hive {
    LocalMachine,
    /// `HKEY_USERS\<sid>`: the streaming user's own settings, written by an
    /// administrator process that is not that user.
    User(String),
}

pub const TERMINAL_SERVER: &str = r"SYSTEM\CurrentControlSet\Control\Terminal Server";
pub const WINSTATIONS: &str = r"SYSTEM\CurrentControlSet\Control\Terminal Server\WinStations";
pub const RDP_TCP: &str = r"SYSTEM\CurrentControlSet\Control\Terminal Server\WinStations\RDP-Tcp";
pub const RDP_CLIENT: &str = r"Software\Microsoft\Terminal Server Client";
/// The firewall group of Windows' own "Remote Desktop" inbound rules.
pub const REMOTE_DESKTOP_RULES: &str = "@FirewallAPI.dll,-28752";
/// The inbound block rule setup adds for the RDP port; block rules win over
/// any allow rule, and loopback traffic bypasses the firewall.
pub const BLOCK_RULE: &str = "Rubylight child session: RDP only on this PC";
const DEFAULT_RDP_PORT: u32 = 3389;

/// The system settings setup changes; the registry, child sessions and the
/// firewall behind one injectable seam.
pub trait SystemSettings {
    fn read_dword(&self, hive: &Hive, path: &str, name: &str) -> Result<Option<u32>>;
    fn write_dword(&mut self, hive: &Hive, path: &str, name: &str, value: u32) -> Result<()>;
    /// Removing a value that does not exist succeeds.
    fn delete_value(&mut self, hive: &Hive, path: &str, name: &str) -> Result<()>;
    fn child_sessions_enabled(&self) -> Result<bool>;
    fn set_child_sessions(&mut self, enabled: bool) -> Result<()>;
    /// Whether Windows Firewall is on for every network profile.
    fn firewall_on(&self) -> Result<bool>;
    /// Names of the enabled rules in a firewall group.
    fn enabled_rules(&self, group: &str) -> Result<Vec<String>>;
    fn set_rule_enabled(&mut self, name: &str, enabled: bool) -> Result<()>;
    /// Add (or replace) an inbound TCP and UDP block rule for a port.
    fn add_block_rule(&mut self, name: &str, port: u16) -> Result<()>;
    /// Removing a rule that does not exist succeeds.
    fn remove_rule(&mut self, name: &str) -> Result<()>;
}

/// Every value setup changes, as it was before setup; saved once, before the
/// first change, and put back on uninstall.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupState {
    pub version: u32,
    pub user_sid: String,
    pub deny_ts_connections: Option<u32>,
    pub dwm_frame_interval: Option<u32>,
    pub suppress_when_minimized: Option<u32>,
    pub child_sessions: bool,
    pub remote_desktop_rules_enabled: Vec<String>,
    pub rdp_port: u16,
}

struct Change {
    machine: bool,
    path: &'static str,
    name: &'static str,
    value: u32,
}
/// What setup writes: RDP on (it stays loopback-only, see [`BLOCK_RULE`]),
/// the session frame interval raised to 8 (about 120 fps), and the RDP
/// client keeps rendering while minimized.
const CHANGES: [Change; 3] = [
    Change {
        machine: true,
        path: TERMINAL_SERVER,
        name: "fDenyTSConnections",
        value: 0,
    },
    Change {
        machine: true,
        path: WINSTATIONS,
        name: "DWMFRAMEINTERVAL",
        value: 8,
    },
    Change {
        machine: false,
        path: RDP_CLIENT,
        name: "RemoteDesktop_SuppressWhenMinimized",
        value: 2,
    },
];
fn hive(change: &Change, sid: &str) -> Hive {
    if change.machine {
        Hive::LocalMachine
    } else {
        Hive::User(sid.into())
    }
}

/// Read the values setup is about to change.
pub fn record(system: &dyn SystemSettings, user_sid: &str) -> Result<SetupState> {
    if user_sid.trim().is_empty() {
        bail!("child-session setup needs the streaming user's account");
    }
    let read = |change: &Change| {
        system
            .read_dword(&hive(change, user_sid), change.path, change.name)
            .with_context(|| format!("reading {}", change.name))
    };
    let port = system
        .read_dword(&Hive::LocalMachine, RDP_TCP, "PortNumber")
        .context("reading the RDP port")?
        .unwrap_or(DEFAULT_RDP_PORT);
    Ok(SetupState {
        version: 1,
        user_sid: user_sid.into(),
        deny_ts_connections: read(&CHANGES[0])?,
        dwm_frame_interval: read(&CHANGES[1])?,
        suppress_when_minimized: read(&CHANGES[2])?,
        child_sessions: system
            .child_sessions_enabled()
            .context("asking Windows whether child sessions are on")?,
        remote_desktop_rules_enabled: system
            .enabled_rules(REMOTE_DESKTOP_RULES)
            .context("listing the Remote Desktop firewall rules")?,
        rdp_port: u16::try_from(port).context("the RDP port is out of range")?,
    })
}

/// Set up child sessions. The original values are recorded and saved (through
/// `save`) before the first change, unless an earlier setup saved them
/// already: setting up again never overwrites what was there before Rubylight.
pub fn setup(
    system: &mut dyn SystemSettings,
    saved: Option<SetupState>,
    user_sid: &str,
    save: impl FnOnce(&SetupState) -> Result<()>,
) -> Result<SetupState> {
    // RDP is reachable only over loopback because the firewall blocks it;
    // with the firewall off it would be open to the network.
    if !system.firewall_on().context("reading the Windows Firewall state")? {
        bail!(
            "Windows Firewall is off for at least one network profile, so turning on RDP would open it to the network. Turn the firewall on, then set up child sessions again. Nothing was changed."
        );
    }
    let state = match saved {
        Some(state) if state.user_sid == user_sid => state,
        Some(state) => bail!(
            "child sessions were set up for another Windows account ({}); restore that setup first",
            state.user_sid
        ),
        None => {
            let state = record(system, user_sid)?;
            save(&state).context("saving the original Windows settings")?;
            state
        }
    };
    system
        .add_block_rule(BLOCK_RULE, state.rdp_port)
        .context("adding the firewall rule that keeps RDP on this PC")?;
    for name in system.enabled_rules(REMOTE_DESKTOP_RULES)? {
        system
            .set_rule_enabled(&name, false)
            .with_context(|| format!("turning off the firewall rule {name}"))?;
    }
    for change in &CHANGES {
        system
            .write_dword(&hive(change, user_sid), change.path, change.name, change.value)
            .with_context(|| format!("setting {}", change.name))?;
    }
    system
        .set_child_sessions(true)
        .context("turning on Windows child sessions")?;
    if !system.child_sessions_enabled()? {
        bail!("Windows did not turn on child sessions; this edition may not have them");
    }
    Ok(state)
}

/// Put every value back as it was before setup. Every step runs even when an
/// earlier one fails; the failures come back together.
pub fn restore(system: &mut dyn SystemSettings, state: &SetupState) -> Result<()> {
    let mut problems = Vec::new();
    let originals = [
        state.deny_ts_connections,
        state.dwm_frame_interval,
        state.suppress_when_minimized,
    ];
    for (change, original) in CHANGES.iter().zip(originals) {
        let hive = hive(change, &state.user_sid);
        let done = match original {
            Some(value) => system.write_dword(&hive, change.path, change.name, value),
            None => system.delete_value(&hive, change.path, change.name),
        };
        if let Err(error) = done {
            problems.push(format!("{}: {error:#}", change.name));
        }
    }
    for name in &state.remote_desktop_rules_enabled {
        if let Err(error) = system.set_rule_enabled(name, true) {
            problems.push(format!("firewall rule {name}: {error:#}"));
        }
    }
    if let Err(error) = system.remove_rule(BLOCK_RULE) {
        problems.push(format!("firewall rule {BLOCK_RULE}: {error:#}"));
    }
    if let Err(error) = system.set_child_sessions(state.child_sessions) {
        problems.push(format!("child sessions: {error:#}"));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        bail!(
            "some Windows settings were not restored: {}",
            problems.join("; ")
        )
    }
}

/// What is still missing for child sessions, empty when setup is complete.
pub fn missing(system: &dyn SystemSettings, user_sid: &str) -> Result<Vec<String>> {
    let mut missing = Vec::new();
    if !system.child_sessions_enabled()? {
        missing.push("Windows child sessions are off".to_string());
    }
    for change in &CHANGES {
        if system.read_dword(&hive(change, user_sid), change.path, change.name)? != Some(change.value) {
            missing.push(format!("{} is not {}", change.name, change.value));
        }
    }
    if !system.enabled_rules(REMOTE_DESKTOP_RULES)?.is_empty() {
        missing.push("Windows' Remote Desktop firewall rules are on".to_string());
    }
    if !system.firewall_on()? {
        missing.push("Windows Firewall is off for a network profile".to_string());
    }
    Ok(missing)
}

/// A graphics adapter as DXGI reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Adapter {
    pub name: String,
    pub vendor: u32,
    pub dedicated_memory: u64,
    /// DXGI's software flag (Microsoft Basic Render Driver).
    pub software: bool,
    /// D3D12's `UMA` architecture flag, when it could be read.
    pub uma: Option<bool>,
}
const MICROSOFT: u32 = 0x1414;
impl Adapter {
    pub fn is_software(&self) -> bool {
        self.software || self.vendor == MICROSOFT
    }
    /// An integrated GPU shares system memory: D3D12 says UMA, or (without
    /// D3D12) it has under 512 MiB of its own memory.
    pub fn is_integrated(&self) -> bool {
        !self.is_software() && self.uma.unwrap_or(self.dedicated_memory < 512 << 20)
    }
}

/// Which GPU the child session's desktop renders on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GpuVerdict {
    Discrete { name: String },
    Integrated { name: String, discrete: Option<String> },
    Software { discrete: Option<String> },
    Unknown,
}
/// `render` is the adapter of the display the child-session host captures.
pub fn gpu_verdict(render: Option<&Adapter>, adapters: &[Adapter]) -> GpuVerdict {
    let discrete = adapters
        .iter()
        .find(|adapter| !adapter.is_software() && !adapter.is_integrated())
        .map(|adapter| adapter.name.clone());
    match render {
        None => GpuVerdict::Unknown,
        Some(adapter) if adapter.is_software() => GpuVerdict::Software { discrete },
        Some(adapter) if adapter.is_integrated() => GpuVerdict::Integrated {
            name: adapter.name.clone(),
            discrete: discrete.filter(|name| *name != adapter.name),
        },
        Some(adapter) => GpuVerdict::Discrete {
            name: adapter.name.clone(),
        },
    }
}
const GPU_FIX: &str = "Turn on the policy \"Use hardware graphics adapters for all Remote Desktop Services sessions\" (Computer Configuration > Administrative Templates > Windows Components > Remote Desktop Services > Remote Desktop Session Host > Remote Session Environment), or set butterpollo.exe to High performance in Settings > System > Display > Graphics, then end the child session and start it again. Rubylight never encodes in software.";
/// The loud message for a desktop that is not on the discrete GPU.
pub fn gpu_problem(verdict: &GpuVerdict) -> Option<String> {
    match verdict {
        // An APU without a discrete GPU renders and encodes on its only GPU.
        GpuVerdict::Discrete { .. } | GpuVerdict::Integrated { discrete: None, .. } => None,
        GpuVerdict::Integrated {
            name,
            discrete: Some(discrete),
        } => Some(format!(
            "The child session renders on the integrated GPU {name}, not on {discrete}. {GPU_FIX}"
        )),
        GpuVerdict::Software { discrete } => Some(format!(
            "The child session renders in software (Microsoft Basic Render Driver){}, so it can't be encoded. {GPU_FIX}",
            discrete
                .as_deref()
                .map(|d| format!(", not on {d}"))
                .unwrap_or_default()
        )),
        GpuVerdict::Unknown => Some(
            "The child-session host could not find which GPU renders its desktop. Check its log before streaming.".into(),
        ),
    }
}

/// The child-session host's configuration: its own ports, name and WebRTC
/// ports, WGC capture (Desktop Duplication does not run in remote sessions),
/// no virtual displays or display changes of its own, no tray.
pub fn child_host_config(main_name: &str, main_base_port: u16) -> Vec<(&'static str, String)> {
    let base = if main_base_port == CHILD_BASE_PORT {
        CHILD_BASE_PORT + 1000
    } else {
        CHILD_BASE_PORT
    };
    let name = if main_name.trim().is_empty() {
        "Rubylight (background)".to_string()
    } else {
        format!("{} (background)", main_name.trim())
    };
    vec![
        (CHILD_HOST_KEY, "true".into()),
        ("port", base.to_string()),
        ("sunshine_name", name),
        ("webrtc_port", CHILD_WEBRTC_PORTS.0.to_string()),
        ("webrtc_media_port", CHILD_WEBRTC_PORTS.1.to_string()),
        ("capture", "wgc".into()),
        ("virtual_display_mode", "disabled".into()),
        ("dd_configuration_option", "disabled".into()),
        ("system_tray", "false".into()),
    ]
}

/// The limits the console and the docs state for the child session.
pub const LIMITS: [&str; 3] = [
    "No HDR in the child session.",
    "The RDP compositor caps the child session at about 120 fps.",
    "Windows may render the child session on an integrated GPU; the host says so and does not stream it in software.",
];

#[cfg(test)]
mod tests;
