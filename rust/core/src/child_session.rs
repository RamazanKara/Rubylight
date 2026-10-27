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
/// What setup changed ([`SetupState`]), in the host's profile; while it is
/// there, restore (and the uninstaller) have something to put back.
pub const SETUP_FILE: &str = "child_session_setup.json";
/// The flag that restores it: `butterpollo.exe --child-session-restore`.
pub const RESTORE_FLAG: &str = "--child-session-restore";

/// What the host setting asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeSetting {
    Auto,
    ChildSession,
    VirtualDisplay,
}
impl ModeSetting {
    /// Unset means `virtual_display`: child sessions are Experimental and
    /// only run when `auto` or `child_session` asks for them.
    pub fn parse(text: &str) -> Result<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "child_session" => Ok(Self::ChildSession),
            "" | "virtual_display" => Ok(Self::VirtualDisplay),
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
            format!(
                "{MODE_KEY} = virtual_display, the default; child sessions are Experimental and need {MODE_KEY} = auto or child_session (Windows edition {shown})"
            ),
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

/// Whether the host may start the child-session host in `child`: a real
/// session that is not the console's, signed in by the same Windows account
/// (SIDs) as the console, and reported by a viewer (in session `viewer`)
/// that runs in the console session. The session ID comes from the viewer, a
/// process of the unprivileged user, so the SYSTEM host checks it before it
/// attaches anything there: another account's session is refused, never
/// streamed.
pub fn check_child_owner(
    child: u32,
    console: u32,
    viewer: u32,
    child_user: &str,
    console_user: &str,
) -> Result<()> {
    if child == 0 || child > i32::MAX as u32 || child == console {
        bail!("session {child} is not a child session of this PC's screen; nothing is attached");
    }
    if viewer != console {
        bail!(
            "the child-session viewer runs in session {viewer}, not in the session at this PC's screen ({console}); session {child} is not attached or streamed"
        );
    }
    if console_user.trim().is_empty() {
        bail!("nobody is signed in at this PC's screen; the child session is not attached");
    }
    if !child_user.eq_ignore_ascii_case(console_user) {
        bail!(
            "session {child} belongs to another Windows account ({child_user}) than the one at this PC's screen ({console_user}); it is not attached or streamed"
        );
    }
    Ok(())
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
    pub fn connect(
        &mut self,
        now: u64,
        state: RdpState,
        password: Option<bool>,
    ) -> Vec<ViewerAction> {
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
    pub fn reconnect(
        &mut self,
        now: u64,
        state: RdpState,
        password: Option<bool>,
    ) -> Vec<ViewerAction> {
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
        // No attempt is in flight: a new password that has not signed in yet stays unverified
        // (so the next attempt still checks it), but is never judged by this one.
        self.last_attempt = None;
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
    /// The viewer refused to connect (the RDP server check, or a setting the
    /// session needs): detach, with the reason, and no retry. The password
    /// never reached Windows, so it is neither saved nor dropped.
    pub fn refuse(&mut self, state: RdpState, reason: &str) -> Vec<ViewerAction> {
        let actions = self.detach(state);
        self.set(
            Phase::Idle,
            &format!("The child session's desktop was not opened: {reason}"),
        );
        actions
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
            RdpState::Connecting => self.set(
                Phase::Connecting,
                "Windows is opening the child session's desktop.",
            ),
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
    /// The password in a JSON string literal (quotes included), such as the
    /// raw `password` value of a console request. It is unescaped straight
    /// into one buffer that is wiped on drop: serde_json's own string
    /// decoding copies escaped text through a scratch buffer it never wipes.
    pub fn from_json_string(literal: &str) -> Result<Self> {
        let Some(inner) = literal
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .filter(|_| literal.len() >= 2)
        else {
            bail!("the password is not a JSON string");
        };
        // Unescaped text is never longer than the literal: the buffer never grows.
        let mut text = Zeroizing::new(Vec::with_capacity(inner.len()));
        let mut bytes = inner.bytes();
        while let Some(byte) = bytes.next() {
            match byte {
                b'"' => bail!("the password is not one JSON string"),
                0..0x20 => bail!("the password has an unescaped control character"),
                b'\\' => {
                    let decoded = match bytes.next() {
                        Some(b'"') => '"',
                        Some(b'\\') => '\\',
                        Some(b'/') => '/',
                        Some(b'b') => '\u{8}',
                        Some(b'f') => '\u{c}',
                        Some(b'n') => '\n',
                        Some(b'r') => '\r',
                        Some(b't') => '\t',
                        Some(b'u') => {
                            let high = json_hex4(&mut bytes)?;
                            let code = match high {
                                0xD800..0xDC00 => {
                                    if bytes.next() != Some(b'\\') || bytes.next() != Some(b'u') {
                                        bail!("the password has a lone surrogate escape");
                                    }
                                    let low = json_hex4(&mut bytes)?;
                                    if !(0xDC00..0xE000).contains(&low) {
                                        bail!("the password has a lone surrogate escape");
                                    }
                                    0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00)
                                }
                                0xDC00..0xE000 => bail!("the password has a lone surrogate escape"),
                                _ => high,
                            };
                            char::from_u32(code).context("the password has a bad escape")?
                        }
                        _ => bail!("the password has a bad escape"),
                    };
                    let mut encoded = [0u8; 4];
                    text.extend_from_slice(decoded.encode_utf8(&mut encoded).as_bytes());
                    zeroize::Zeroize::zeroize(&mut encoded);
                }
                _ => text.push(byte),
            }
        }
        // Valid: `inner` is UTF-8 and every escape adds a whole character.
        match String::from_utf8(std::mem::take(&mut *text)) {
            Ok(text) => Ok(Self::new(text)),
            Err(error) => {
                drop(Zeroizing::new(error.into_bytes()));
                bail!("the password is not UTF-8")
            }
        }
    }
}
fn json_hex4(bytes: &mut impl Iterator<Item = u8>) -> Result<u32> {
    let mut code = 0;
    for _ in 0..4 {
        let digit = bytes
            .next()
            .and_then(|byte| char::from(byte).to_digit(16))
            .context("the password has a bad \\u escape")?;
        code = code * 16 + digit;
    }
    Ok(code)
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
    /// None to use the saved one. `rdp_port` is the one setup recorded.
    Start {
        display: DisplaySettings,
        #[serde(default = "default_rdp_port")]
        rdp_port: u16,
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
    EndSession {
        child: u32,
        generation: u64,
    },
    ForgetPassword,
    Quit,
}
impl ViewerCommand {
    /// The line written to the viewer's pipe; it holds the password, so it is
    /// wiped after the write and never logged.
    pub fn line(&self) -> Result<Zeroizing<String>> {
        // Sized up front: a growing buffer would leave unwiped copies behind.
        let password = match self {
            Self::Start {
                password: Some(password),
                ..
            } => password.expose().len(),
            _ => 0,
        };
        let mut line = Zeroizing::new(Vec::with_capacity(1024 + 2 * password));
        serde_json::to_writer(&mut *line, self)?;
        line.push(b'\n');
        let line = String::from_utf8(std::mem::take(&mut *line)).map_err(|error| {
            drop(Zeroizing::new(error.into_bytes()));
            anyhow::anyhow!("a viewer command is not UTF-8")
        })?;
        Ok(Zeroizing::new(line))
    }
}
/// The password travels as hex: serde never escapes it, so the viewer reads
/// it straight from its wiped line instead of through serde's scratch and
/// buffered copies, which are not wiped.
mod wire_secret {
    use super::Secret;
    use serde::{Deserializer, Serializer, de};
    use std::fmt;
    use zeroize::Zeroizing;
    pub fn serialize<S: Serializer>(
        value: &Option<Secret>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(secret) => {
                let bytes = secret.expose().as_bytes();
                let mut text = Zeroizing::new(vec![0u8; 2 * bytes.len()]);
                hex::encode_to_slice(bytes, &mut text).map_err(serde::ser::Error::custom)?;
                serializer
                    .serialize_some(std::str::from_utf8(&text).map_err(serde::ser::Error::custom)?)
            }
            None => serializer.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Secret>, D::Error> {
        deserializer.deserialize_option(Wire)
    }
    struct Wire;
    impl<'de> de::Visitor<'de> for Wire {
        type Value = Option<Secret>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a hex-encoded password or null")
        }
        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_some<D: Deserializer<'de>>(
            self,
            deserializer: D,
        ) -> Result<Self::Value, D::Error> {
            deserializer.deserialize_str(self)
        }
        fn visit_str<E: de::Error>(self, text: &str) -> Result<Self::Value, E> {
            let mut bytes = Zeroizing::new(vec![0u8; text.len() / 2]);
            // No detail: an error message must not quote any of the password.
            hex::decode_to_slice(text, &mut bytes)
                .map_err(|_| E::custom("the password is not hex"))?;
            let text = String::from_utf8(std::mem::take(&mut *bytes)).map_err(|error| {
                drop(Zeroizing::new(error.into_bytes()));
                E::custom("the password is not UTF-8")
            })?;
            Ok(Some(Secret::new(text)))
        }
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

/// The longest line the host reads from the viewer's pipes.
pub const STATUS_LINE_CAP: usize = 64 * 1024;

/// Lines from a pipe the other side controls, each at most `cap` bytes with
/// its newline. The buffer is allocated once at `cap` and never grows, and
/// each line is wiped once the next one is read (and on drop), so a line that
/// holds a password leaves no copy behind. Read it unbuffered: a `BufReader`
/// under it keeps copies of its own.
pub struct LineReader<R> {
    reader: R,
    buffer: Zeroizing<Vec<u8>>,
    /// Bytes read into `buffer`.
    filled: usize,
    /// The line handed out last, with its newline, at the front of `buffer`.
    taken: usize,
    /// The rest of a line over the cap is dropped until its newline.
    skipping: bool,
}
impl<R: std::io::Read> LineReader<R> {
    pub fn new(reader: R, cap: usize) -> Self {
        Self {
            reader,
            buffer: Zeroizing::new(vec![0; cap.max(1)]),
            filled: 0,
            taken: 0,
            skipping: false,
        }
    }
    /// The next line without its newline; None once the pipe is closed. A
    /// line over the cap is an `InvalidData` error that holds none of its
    /// content, and reading goes on at the line after it.
    pub fn read_line(&mut self) -> std::io::Result<Option<&[u8]>> {
        self.drop_front(self.taken);
        self.taken = 0;
        loop {
            let newline = self.buffer[..self.filled].iter().position(|&b| b == b'\n');
            match (newline, self.skipping) {
                (Some(end), true) => {
                    self.drop_front(end + 1);
                    self.skipping = false;
                    continue;
                }
                (Some(end), false) => {
                    self.taken = end + 1;
                    return Ok(Some(&self.buffer[..end]));
                }
                (None, skipping) if self.filled == self.buffer.len() => {
                    self.drop_front(self.filled);
                    self.skipping = true;
                    if !skipping {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("a line longer than {} bytes", self.buffer.len()),
                        ));
                    }
                }
                (None, _) => {}
            }
            let read = match self.reader.read(&mut self.buffer[self.filled..]) {
                Ok(read) => read,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if read == 0 {
                // A last line without a newline still counts; what is left of
                // one over the cap does not.
                if self.skipping || self.filled == 0 {
                    self.drop_front(self.filled);
                    self.skipping = false;
                    return Ok(None);
                }
                self.taken = self.filled;
                return Ok(Some(&self.buffer[..self.filled]));
            }
            self.filled += read;
        }
    }
    /// Move what follows the first `count` bytes to the front and wipe the
    /// bytes that are no longer in use.
    fn drop_front(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        self.buffer.copy_within(count..self.filled, 0);
        let rest = self.filled - count;
        zeroize::Zeroize::zeroize(&mut self.buffer[rest..self.filled]);
        self.filled = rest;
    }
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
        // Checked in place: String::from_utf8's error would keep an unwiped copy of the bytes.
        let text = std::str::from_utf8(&plain).map_err(|_| {
            anyhow::anyhow!("the saved password is damaged; forget it and enter it again")
        })?;
        Ok(Some(Secret::new(text.to_owned())))
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
/// Group policy for Remote Desktop; its `fDenyTSConnections` overrides [`TERMINAL_SERVER`]'s.
pub const TERMINAL_SERVICES_POLICY: &str =
    r"SOFTWARE\Policies\Microsoft\Windows NT\Terminal Services";
pub const RDP_CLIENT: &str = r"Software\Microsoft\Terminal Server Client";
/// The firewall group of Windows' own "Remote Desktop" inbound rules.
pub const REMOTE_DESKTOP_RULES: &str = "@FirewallAPI.dll,-28752";
/// The inbound block rule setup adds for the RDP port; block rules win over
/// any allow rule, and loopback traffic bypasses the firewall.
pub const BLOCK_RULE: &str = "Rubylight child session: RDP only on this PC";
const DEFAULT_RDP_PORT: u32 = 3389;

/// The registry's DWORD values: all [`turn_rdp_off`] needs, so it works when
/// the rest of [`SystemSettings`] (Windows Firewall) can't be opened.
pub trait Registry {
    fn read_dword(&self, hive: &Hive, path: &str, name: &str) -> Result<Option<u32>>;
    fn write_dword(&mut self, hive: &Hive, path: &str, name: &str, value: u32) -> Result<()>;
}

/// The system settings setup changes; the registry, child sessions and the
/// firewall behind one injectable seam.
pub trait SystemSettings: Registry {
    /// Removing a value that does not exist succeeds.
    fn delete_value(&mut self, hive: &Hive, path: &str, name: &str) -> Result<()>;
    fn child_sessions_enabled(&self) -> Result<bool>;
    fn set_child_sessions(&mut self, enabled: bool) -> Result<()>;
    /// Why Windows Firewall can't keep RDP on this PC, None when it can: it is
    /// off for a network profile, or group policy keeps local rules from
    /// taking effect.
    fn firewall_problem(&self) -> Result<Option<String>>;
    /// What is in effect now (the active store: local rules merged with
    /// group policy), for a rule group and the block rules under a name.
    fn active_firewall(&self, group: &str, block_rule: &str) -> Result<ActiveFirewall>;
    /// IDs of the enabled rules in a firewall group. An ID is the rule's
    /// unique name in the firewall store (`Get-NetFirewallRule`'s `Name`),
    /// not its display name, which several rules can share.
    fn enabled_rules(&self, group: &str) -> Result<Vec<String>>;
    /// Turn exactly the rule with this ID on or off; it fails when the rule
    /// is gone or is not in the group.
    fn set_rule_enabled(&mut self, group: &str, id: &str, enabled: bool) -> Result<()>;
    /// Add (or replace) inbound TCP and UDP block rules for a port under this name.
    fn add_block_rule(&mut self, name: &str, port: u16) -> Result<()>;
    /// Remove the rules [`SystemSettings::add_block_rule`] added; removing
    /// rules that do not exist succeeds.
    fn remove_rule(&mut self, name: &str) -> Result<()>;
    /// Make `HKEY_USERS\<sid>` writable: load the account's own hive when it
    /// is signed out. True when this call loaded it, so it is unloaded after.
    fn load_user_hive(&mut self, sid: &str) -> Result<bool>;
    fn unload_user_hive(&mut self, sid: &str) -> Result<()>;
    /// The TCP listeners of the Remote Desktop service's process now, none
    /// when it is not running; [`RdpListeners::ports`] are RDP's. It reads
    /// `PortNumber` only when it starts, so these can differ from the registry.
    fn rdp_listeners(&self) -> Result<RdpListeners>;
}

/// The firewall as it is in effect now ([`SystemSettings::active_firewall`]);
/// every field is required, so a short answer never reads as safe.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct ActiveFirewall {
    /// IDs of the group's enabled inbound allow rules.
    pub allowed: Vec<String>,
    /// Enabled inbound block rules under the name, for TCP and UDP both.
    pub blocked: bool,
    /// The local ports of each of those block rules
    /// (`Get-NetFirewallPortFilter`'s `LocalPort`, comma-joined).
    pub block_ports: Vec<String>,
    /// Network profiles where the firewall is off or ignores local rules.
    pub unprotected: Vec<String>,
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
    /// The IDs (see [`SystemSettings::enabled_rules`]) of the Remote Desktop
    /// rules that were on; restore turns on exactly these.
    pub remote_desktop_rule_ids: Vec<String>,
    /// Windows' default when a record has none.
    #[serde(default = "default_rdp_port")]
    pub rdp_port: u16,
}
fn default_rdp_port() -> u16 {
    DEFAULT_RDP_PORT as u16
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
    Ok(SetupState {
        version: 1,
        user_sid: user_sid.into(),
        deny_ts_connections: read(&CHANGES[0])?,
        dwm_frame_interval: read(&CHANGES[1])?,
        suppress_when_minimized: read(&CHANGES[2])?,
        child_sessions: system
            .child_sessions_enabled()
            .context("asking Windows whether child sessions are on")?,
        remote_desktop_rule_ids: system
            .enabled_rules(REMOTE_DESKTOP_RULES)
            .context("listing the Remote Desktop firewall rules")?,
        rdp_port: live_rdp_port(system)?,
    })
}

/// The TCP ports the Remote Desktop service listens on now ([`RdpListeners::ports`]).
fn rdp_ports(system: &dyn SystemSettings) -> Result<Vec<u16>> {
    system
        .rdp_listeners()
        .and_then(|listeners| listeners.ports())
        .context("reading the ports Remote Desktop listens on")
}

/// The port RDP-Tcp listens on now (`PortNumber`), Windows' default when unset.
fn live_rdp_port(system: &dyn SystemSettings) -> Result<u16> {
    let port = system
        .read_dword(&Hive::LocalMachine, RDP_TCP, "PortNumber")
        .context("reading the RDP port")?
        .unwrap_or(DEFAULT_RDP_PORT);
    u16::try_from(port).context("the RDP port is out of range")
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
    // with the firewall off, or group policy overriding the local rules, it
    // would be open to the network.
    if let Some(problem) = system
        .firewall_problem()
        .context("reading the Windows Firewall state")?
    {
        bail!(
            "{problem}, so turning on RDP would open it to the network. Fix that, then set up child sessions again. Nothing was changed."
        );
    }
    // The block rule closes the registry's port; the service must listen on that one.
    let live = live_rdp_port(system)?;
    let listening = rdp_ports(system)?;
    if let Some(port) = listening.iter().find(|port| **port != live) {
        bail!(
            "Remote Desktop listens on port {port} but is set to port {live}: Windows reads the port only when the Remote Desktop service starts. Restart the PC, then set up child sessions again. Nothing was changed."
        );
    }
    let state = match saved {
        // RDP moved to another port since: the record follows it (restore never writes the
        // port), so the block rule and the viewer use the port RDP listens on now.
        Some(mut state) if state.user_sid == user_sid => {
            let live = live_rdp_port(system)?;
            if state.rdp_port != live {
                state.rdp_port = live;
                save(&state).context("saving the new RDP port")?;
            }
            state
        }
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
    for id in system.enabled_rules(REMOTE_DESKTOP_RULES)? {
        system
            .set_rule_enabled(REMOTE_DESKTOP_RULES, &id, false)
            .with_context(|| format!("turning off the firewall rule {id}"))?;
    }
    // Checked where it counts, before RDP is on: what is in effect now.
    let open = firewall_missing(system, state.rdp_port)?;
    if !open.is_empty() {
        bail!(
            "{}, so turning on RDP would open it to the network. RDP was not turned on; Restore Windows settings puts the firewall rules back.",
            open.join("; ")
        );
    }
    // From the first write on, RDP may be on: whatever fails after it turns RDP off again.
    if let Err(error) = turn_on(system, user_sid) {
        let outcome = match rdp_off(system) {
            Ok(()) => format!("Remote Desktop was turned off again ({} = 1)", CHANGES[0].name),
            Err(off) => format!(
                "Remote Desktop is still on: turning it off failed ({off:#}); turn it off in Windows' settings or choose Restore Windows settings now"
            ),
        };
        bail!("{error:#}. {outcome}");
    }
    Ok(state)
}
/// Setup's changes once the firewall keeps RDP on this PC: RDP on, then child sessions.
fn turn_on(system: &mut dyn SystemSettings, user_sid: &str) -> Result<()> {
    for change in &CHANGES {
        system
            .write_dword(
                &hive(change, user_sid),
                change.path,
                change.name,
                change.value,
            )
            .with_context(|| format!("setting {}", change.name))?;
    }
    system
        .set_child_sessions(true)
        .context("turning on Windows child sessions")?;
    if !system.child_sessions_enabled()? {
        bail!("Windows did not turn on child sessions; this edition may not have them");
    }
    Ok(())
}

/// Put every value back as it was before setup. Every step runs even when an
/// earlier one fails; the failures come back together. The firewall rules
/// come back only once RDP reads back as it was: until then the block rule
/// keeps it on this PC.
pub fn restore(system: &mut dyn SystemSettings, state: &SetupState) -> Result<()> {
    let mut problems = Vec::new();
    let originals = [
        state.deny_ts_connections,
        state.dwm_frame_interval,
        state.suppress_when_minimized,
    ];
    let mut rdp_restored = false;
    for (index, (change, original)) in CHANGES.iter().zip(originals).enumerate() {
        let hive = hive(change, &state.user_sid);
        // A signed-out account's hive is not under HKEY_USERS until loaded.
        let loaded = match &hive {
            Hive::User(sid) => match system.load_user_hive(sid) {
                Ok(loaded) => loaded.then(|| sid.clone()),
                Err(error) => {
                    problems.push(format!("{}: {error:#}", change.name));
                    continue;
                }
            },
            Hive::LocalMachine => None,
        };
        let done = match original {
            Some(value) => system.write_dword(&hive, change.path, change.name, value),
            None => system.delete_value(&hive, change.path, change.name),
        }
        .and_then(
            |()| match system.read_dword(&hive, change.path, change.name)? {
                now if now == original => Ok(()),
                now => bail!("it reads {now:?} after restoring {original:?}"),
            },
        );
        match done {
            Ok(()) => rdp_restored |= index == 0,
            Err(error) => problems.push(format!("{}: {error:#}", change.name)),
        }
        if let Some(sid) = loaded
            && let Err(error) = system.unload_user_hive(&sid)
        {
            problems.push(format!("unloading the registry hive of {sid}: {error:#}"));
        }
    }
    if rdp_restored {
        for id in &state.remote_desktop_rule_ids {
            if let Err(error) = system.set_rule_enabled(REMOTE_DESKTOP_RULES, id, true) {
                problems.push(format!("firewall rule {id}: {error:#}"));
            }
        }
        if let Err(error) = system.remove_rule(BLOCK_RULE) {
            problems.push(format!("firewall rule {BLOCK_RULE}: {error:#}"));
        }
    } else {
        problems.push(format!(
            "the firewall rules stay as setup left them (\"{BLOCK_RULE}\" on, Remote Desktop's own rules off) until {} is restored",
            CHANGES[0].name
        ));
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
/// `rdp_port` is the port setup recorded, None when there is no record yet.
pub fn missing(
    system: &dyn SystemSettings,
    user_sid: &str,
    rdp_port: Option<u16>,
) -> Result<Vec<String>> {
    let mut missing = settings_missing(system, user_sid)?;
    let rdp_port = match rdp_port {
        Some(port) => port,
        None => live_rdp_port(system)?,
    };
    missing.extend(firewall_missing(system, rdp_port)?);
    Ok(missing)
}

/// [`missing`] short of the firewall: the Windows settings setup changes.
pub fn settings_missing(system: &dyn SystemSettings, user_sid: &str) -> Result<Vec<String>> {
    let mut missing = Vec::new();
    if !system.child_sessions_enabled()? {
        missing.push("Windows child sessions are off".to_string());
    }
    for change in &CHANGES {
        if system.read_dword(&hive(change, user_sid), change.path, change.name)?
            != Some(change.value)
        {
            missing.push(format!("{} is not {}", change.name, change.value));
        }
    }
    if !system.enabled_rules(REMOTE_DESKTOP_RULES)?.is_empty() {
        missing.push("Windows' Remote Desktop firewall rules are on".to_string());
    }
    Ok(missing)
}

/// [`firewall_missing`] on a set-up PC, and when it finds anything, Remote
/// Desktop turned off at once (`fDenyTSConnections = 1`, read back): a
/// firewall that no longer keeps RDP on this PC never leaves it open to the
/// network. The host runs it at its start and before every child-session
/// start. Empty when the check passed; otherwise what it found, then what
/// was done about it. It fails closed: a check that cannot run counts as a
/// failed one. Setting up again turns RDP back on, and restore still puts
/// back the value from before setup.
pub fn enforce_firewall(system: &mut dyn SystemSettings, rdp_port: u16) -> Vec<String> {
    check_firewall(system, rdp_port).unwrap_or_else(|error| {
        vec![
            format!("the firewall could not be checked ({error:#})"),
            turn_rdp_off(system),
        ]
    })
}
/// [`enforce_firewall`] for a check that can be tried again: Err when the
/// check could not run, with nothing changed. What it finds still turns RDP
/// off at once.
pub fn check_firewall(system: &mut dyn SystemSettings, rdp_port: u16) -> Result<Vec<String>> {
    let mut found = firewall_missing(system, rdp_port)?;
    if !found.is_empty() {
        found.push(turn_rdp_off(system));
    }
    Ok(found)
}

/// Turn Remote Desktop off (`fDenyTSConnections = 1`), read it back, and
/// make sure no group policy keeps it on ([`TERMINAL_SERVICES_POLICY`] wins
/// over the value written here). What was done, for the report.
pub fn turn_rdp_off(system: &mut dyn Registry) -> String {
    let change = &CHANGES[0];
    match rdp_off(system) {
        Ok(()) => format!(
            "Remote Desktop was turned off ({} = 1), so it is not open to the network",
            change.name
        ),
        Err(error) => format!(
            "turning Remote Desktop off failed ({}: {error:#}): turn it off in Windows' settings or choose Restore Windows settings now",
            change.name
        ),
    }
}
/// [`turn_rdp_off`], Err when RDP may still be on.
fn rdp_off(system: &mut dyn Registry) -> Result<()> {
    let change = &CHANGES[0];
    system
        .write_dword(&Hive::LocalMachine, change.path, change.name, 1)
        .and_then(
            |()| match system.read_dword(&Hive::LocalMachine, change.path, change.name)? {
                Some(1) => Ok(()),
                now => bail!("it reads {now:?} after writing 1"),
            },
        )
        .and_then(|()| {
            match system.read_dword(&Hive::LocalMachine, TERMINAL_SERVICES_POLICY, change.name)? {
                Some(0) => bail!(
                    "group policy keeps Remote Desktop on ({} = 0 under HKLM\\{TERMINAL_SERVICES_POLICY}), which wins over this setting",
                    change.name
                ),
                _ => Ok(()),
            }
        })
}

/// What keeps the firewall from holding RDP to this PC now, empty when it
/// does: `rdp_port` is the port setup recorded, which RDP must still listen
/// on and the block rule must still close. Setup checks it before RDP goes
/// on, the host at its start and every child-session start.
pub fn firewall_missing(system: &dyn SystemSettings, rdp_port: u16) -> Result<Vec<String>> {
    let mut missing = Vec::new();
    let live = live_rdp_port(system)?;
    if live != rdp_port {
        missing.push(format!(
            "Remote Desktop is set to port {live} now, not to the port {rdp_port} setup closed to the network; set up child sessions again"
        ));
    }
    // The registry can say one port while the service, which read it at its start, listens on
    // another.
    for port in rdp_ports(system)? {
        if port != rdp_port {
            missing.push(format!(
                "Remote Desktop listens on port {port}, not on the port {rdp_port} setup closed to the network; set up child sessions again"
            ));
        }
    }
    if let Some(problem) = system
        .firewall_problem()
        .context("reading the Windows Firewall state")?
    {
        missing.push(problem);
    }
    let active = system
        .active_firewall(REMOTE_DESKTOP_RULES, BLOCK_RULE)
        .context("reading the firewall rules in effect")?;
    if !active.unprotected.is_empty() {
        missing.push(format!(
            "Windows Firewall is off or ignores local rules (group policy) for the {} profile",
            active.unprotected.join(", ")
        ));
    }
    if !active.blocked {
        missing.push(format!(
            "the firewall rule \"{BLOCK_RULE}\" is not in effect"
        ));
    }
    let wanted = rdp_port.to_string();
    let mut others: Vec<&str> = active
        .block_ports
        .iter()
        .map(|ports| ports.trim())
        .filter(|ports| *ports != wanted)
        .collect();
    others.sort_unstable();
    others.dedup();
    if active.blocked && active.block_ports.is_empty() {
        missing.push(format!(
            "the ports the firewall rule \"{BLOCK_RULE}\" closes could not be read"
        ));
    }
    if !others.is_empty() {
        missing.push(format!(
            "the firewall rule \"{BLOCK_RULE}\" closes port {}, not the RDP port {rdp_port}; set up child sessions again",
            others.join(" and ")
        ));
    }
    if !active.allowed.is_empty() {
        missing.push(format!(
            "Remote Desktop firewall rules are in effect: {}",
            active.allowed.join(", ")
        ));
    }
    Ok(missing)
}

/// A listener as Windows' TCP and UDP tables report it
/// (`GetExtendedTcpTable`, `GetExtendedUdpTable`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listener {
    pub address: std::net::IpAddr,
    pub port: u16,
    pub pid: u32,
    /// A UDP socket rather than a TCP listener: RDP's UDP transport.
    pub udp: bool,
}

/// What the viewer reads before every connect ([`check_rdp_server`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RdpServer {
    /// The Remote Desktop service's (TermService) process, 0 when it is not
    /// running.
    pub service_pid: u32,
    /// Every IPv4 and IPv6 TCP listener and UDP socket on this PC.
    pub listeners: Vec<Listener>,
    /// RDP-Tcp's `PortNumber`; Windows' default when it has none.
    pub port_number: Option<u32>,
    pub deny_ts_connections: Option<u32>,
}

/// The Remote Desktop service's name (TermService), which Windows reports as
/// the owner of its sockets.
pub const RDP_SERVICE: &str = "TermService";

/// The TCP listeners (IPv4 and IPv6) of the Remote Desktop service's process,
/// as the host reads them ([`SystemSettings::rdp_listeners`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RdpListeners {
    /// The process hosts other running services too: below Windows' svchost
    /// split threshold the Remote Desktop service shares one with other
    /// NetworkService services, such as Delivery Optimization (TCP 7680).
    pub shared: bool,
    pub listeners: Vec<ServiceListener>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceListener {
    pub port: u16,
    /// The service whose socket it is (its owner module,
    /// `GetOwnerModuleFromTcpEntry`), read when the process is shared;
    /// `svchost.exe` for a socket no service is tagged on.
    pub owner: Option<String>,
}
impl RdpListeners {
    /// The ports RDP listens on, sorted: every listener of the process, or,
    /// when it is shared, those Windows names [`RDP_SERVICE`] the owner of.
    /// Another service's socket, or one no service is tagged on (svchost.exe
    /// itself), is not RDP's; a shared listener without an owner fails.
    pub fn ports(&self) -> Result<Vec<u16>> {
        let mut ports = Vec::new();
        for listener in &self.listeners {
            match &listener.owner {
                _ if !self.shared => ports.push(listener.port),
                Some(owner) if owner.eq_ignore_ascii_case(RDP_SERVICE) => ports.push(listener.port),
                Some(_) => {}
                None => bail!(
                    "the Remote Desktop service shares its process with other services, and which of them listens on port {} was not read",
                    listener.port
                ),
            }
        }
        ports.sort_unstable();
        ports.dedup();
        Ok(ports)
    }
}

/// Refuse, with every reason, unless what answers on `127.0.0.1:rdp_port`
/// is Windows' own RDP server: the Remote Desktop service listens there, no
/// other process listens on that port (TCP or UDP, which RDP's UDP
/// transport uses, over IPv4 or IPv6), and RDP is still on,
/// on that port. The viewer's client does not authenticate the server
/// (`AuthenticationLevel` 0: its certificate is self-signed and loopback has
/// no Kerberos), so this check, before every connect and reconnect, is what
/// keeps the password from another program on the port. It matches by
/// process: a service sharing the Remote Desktop service's svchost runs in
/// the same process and account, so it is no other program.
pub fn check_rdp_server(rdp_port: u16, server: &RdpServer) -> Result<()> {
    let mut problems = Vec::new();
    let service = server.service_pid;
    if service == 0 {
        problems.push("the Remote Desktop service (TermService) is not running".to_string());
    }
    let on_port: Vec<&Listener> = server
        .listeners
        .iter()
        .filter(|listener| listener.port == rdp_port)
        .collect();
    let mut others: Vec<u32> = on_port
        .iter()
        .filter(|listener| service == 0 || listener.pid != service)
        .map(|listener| listener.pid)
        .collect();
    others.sort_unstable();
    others.dedup();
    if !others.is_empty() {
        let pids: Vec<String> = others.iter().map(u32::to_string).collect();
        problems.push(format!(
            "another program (process {}) listens on port {rdp_port} (TCP or UDP)",
            pids.join(", ")
        ));
    }
    let loopback = on_port.iter().any(|listener| {
        listener.pid == service
            && !listener.udp
            && matches!(listener.address, std::net::IpAddr::V4(address) if address.is_unspecified() || address.is_loopback())
    });
    if service != 0 && !loopback {
        problems.push(format!(
            "the Remote Desktop service does not listen on 127.0.0.1:{rdp_port}"
        ));
    }
    let live = server.port_number.unwrap_or(DEFAULT_RDP_PORT);
    if live != u32::from(rdp_port) {
        problems.push(format!(
            "Remote Desktop is set to port {live}, not the port {rdp_port} setup recorded; set up child sessions again"
        ));
    }
    if server.deny_ts_connections != Some(0) {
        problems.push(
            "Remote Desktop is turned off (fDenyTSConnections is not 0); set up child sessions again"
                .to_string(),
        );
    }
    if problems.is_empty() {
        Ok(())
    } else {
        bail!(
            "refused to sign in to the child session over port {rdp_port}, because {}",
            problems.join("; ")
        )
    }
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
    Discrete {
        name: String,
    },
    Integrated {
        name: String,
        discrete: Option<String>,
    },
    Software {
        discrete: Option<String>,
    },
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
