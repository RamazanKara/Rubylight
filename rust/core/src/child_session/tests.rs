use super::*;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::{Arc, Mutex},
};

const SID: &str = "S-1-5-21-1000-2000-3000-1001";
const PASSWORD: &str = "correct horse battery staple";

#[test]
fn home_keeps_the_virtual_display_and_says_why() {
    for id in ["Core", "CoreSingleLanguage", "CoreCountrySpecific", "CoreN"] {
        for setting in [ModeSetting::Auto, ModeSetting::ChildSession] {
            let decision = decide(setting, id, false);
            assert_eq!(decision.mode, Mode::VirtualDisplay, "{id}");
            assert_eq!(
                decision.label,
                "Background streaming (virtual display): Windows Home"
            );
            assert!(decision.reason.contains(id), "{}", decision.reason);
        }
    }
}

#[test]
fn pro_and_higher_use_the_child_session() {
    for id in [
        "Professional",
        "ProfessionalWorkstation",
        "ProfessionalEducation",
        "Education",
        "Enterprise",
        "EnterpriseS",
        "IoTEnterprise",
        "ServerStandard",
    ] {
        let decision = decide(ModeSetting::Auto, id, false);
        assert_eq!(decision.mode, Mode::ChildSession, "{id}");
        assert!(decision.reason.contains(id));
        // The setting can still keep the virtual display, and says so.
        let kept = decide(ModeSetting::VirtualDisplay, id, false);
        assert_eq!(kept.mode, Mode::VirtualDisplay);
        assert!(kept.reason.contains("background_mode = virtual_display"));
    }
}

#[test]
fn an_unknown_edition_is_never_switched_silently() {
    let auto = decide(ModeSetting::Auto, "", false);
    assert_eq!(auto.mode, Mode::VirtualDisplay);
    assert!(auto.reason.contains("unknown"));
    assert!(auto.reason.contains("background_mode = child_session"));
    assert_eq!(
        decide(ModeSetting::ChildSession, "SomethingNew", false).mode,
        Mode::ChildSession
    );
    assert_eq!(
        decide(ModeSetting::Auto, "Professional", true).mode,
        Mode::InsideChildSession
    );
}

#[test]
fn mode_setting_parses_and_rejects_typos() {
    // Off by default: child sessions ship Experimental.
    assert_eq!(ModeSetting::parse("").unwrap(), ModeSetting::VirtualDisplay);
    assert_eq!(ModeSetting::parse(" auto ").unwrap(), ModeSetting::Auto);
    assert_eq!(
        ModeSetting::parse(" Child_Session ").unwrap(),
        ModeSetting::ChildSession
    );
    assert_eq!(
        ModeSetting::parse("virtual_display").unwrap(),
        ModeSetting::VirtualDisplay
    );
    assert!(ModeSetting::parse("child").is_err());
}

#[test]
fn the_child_desktop_defaults_to_1080p_at_100_percent() {
    let display = DisplaySettings::parse("", "").unwrap();
    assert_eq!(display, DisplaySettings::default());
    assert_eq!(
        (display.width, display.height, display.scale),
        (1920, 1080, 100)
    );
    assert_eq!(display.rdp_scale_factors(), (100, 180));
    let display = DisplaySettings::parse("2560X1440", "150 %").unwrap();
    assert_eq!(
        (display.width, display.height, display.scale),
        (2560, 1440, 150)
    );
    for (resolution, scale) in [
        ("1920", ""),
        ("100x100", ""),
        ("9000x1080", ""),
        ("axb", ""),
        ("", "99"),
        ("", "501"),
        ("", "big"),
    ] {
        assert!(
            DisplaySettings::parse(resolution, scale).is_err(),
            "{resolution} {scale}"
        );
    }
}

#[test]
fn only_other_real_sessions_are_targets() {
    assert!(is_target(7, 1, 2));
    for target in [0, 1, 2, u32::MAX, i32::MAX as u32 + 1] {
        assert!(!is_target(target, 1, 2), "{target}");
    }
}

#[test]
fn an_approval_does_not_survive_a_reattach_with_the_same_id() {
    let mut gate = RequestGate::default();
    gate.update(7, true);
    let approval = Confirmation::new(&gate, 7);
    assert!(approval.can_proceed(&gate, 7));
    // Windows reuses child-session 7 for the next connection.
    gate.update(7, false);
    gate.update(7, true);
    assert!(!approval.can_proceed(&gate, 7));
    let fresh = Confirmation::new(&gate, 7);
    assert!(fresh.can_proceed(&gate, 7));
    assert!(!fresh.can_proceed(&gate, 8));
    gate.select("host");
    assert!(!fresh.can_proceed(&gate, 7));
    let late = Confirmation::from_status(7, gate.generation());
    gate.close();
    assert!(!late.can_proceed(&gate, 7));
    assert!(!gate.accepts(gate.generation(), 7));
}

#[test]
fn ending_never_signs_out_the_wrong_session() {
    assert_eq!(end_confirmed_session(7, 7, 1, 2), Ok(7));
    assert_eq!(end_confirmed_session(7, 8, 1, 2), Err(EndError::Changed));
    for sentinel in [0, u32::MAX, 1, 2] {
        assert_eq!(
            end_confirmed_session(sentinel, sentinel, 1, 2),
            Err(EndError::NoChild),
            "{sentinel}"
        );
    }
}

const CURRENT: u32 = 1;
const CONSOLE: u32 = 1;

fn attached(viewer: &mut Viewer, now: u64) {
    assert_eq!(
        viewer.connect(now, RdpState::Disconnected, Some(false)),
        vec![ViewerAction::Connect]
    );
    assert!(
        viewer
            .poll(
                now + 2000,
                RdpState::Connected,
                None,
                Some(7),
                CURRENT,
                CONSOLE
            )
            .is_empty()
    );
    assert_eq!(viewer.phase(), Phase::Connected);
    assert_eq!(viewer.child(), 7);
}

#[test]
fn without_a_password_the_viewer_asks_for_one() {
    let mut viewer = Viewer::default();
    assert!(viewer.connect(0, RdpState::Disconnected, None).is_empty());
    assert_eq!(viewer.phase(), Phase::NeedsPassword);
    assert!(viewer.message().contains("not your PIN"));
    // Nothing reconnects without a password.
    assert!(
        viewer
            .poll(60_000, RdpState::Disconnected, None, None, CURRENT, CONSOLE)
            .is_empty()
    );
}

#[test]
fn a_new_password_is_saved_only_once_it_signed_in() {
    let mut viewer = Viewer::default();
    assert_eq!(
        viewer.connect(0, RdpState::Disconnected, Some(true)),
        vec![ViewerAction::Connect]
    );
    assert!(
        viewer
            .poll(1000, RdpState::Connecting, None, None, CURRENT, CONSOLE)
            .is_empty()
    );
    assert_eq!(
        viewer.poll(2000, RdpState::Connected, None, Some(7), CURRENT, CONSOLE),
        vec![ViewerAction::SavePassword]
    );
    // Saved once, not on every poll.
    assert!(
        viewer
            .poll(4000, RdpState::Connected, None, Some(7), CURRENT, CONSOLE)
            .is_empty()
    );
}

#[test]
fn a_rejected_password_is_dropped_and_never_retried() {
    let mut viewer = Viewer::default();
    viewer.connect(0, RdpState::Disconnected, Some(true));
    assert!(
        viewer
            .poll(
                2000,
                RdpState::Disconnected,
                Some(0),
                None,
                CURRENT,
                CONSOLE
            )
            .is_empty()
    );
    assert_eq!(
        viewer.poll(
            4000,
            RdpState::Disconnected,
            Some(0),
            None,
            CURRENT,
            CONSOLE
        ),
        vec![ViewerAction::DropPassword]
    );
    assert_eq!(viewer.phase(), Phase::SignInFailed);
    assert!(viewer.message().contains("Nothing was saved"));
    assert!(
        viewer
            .poll(
                30_000,
                RdpState::Disconnected,
                Some(0),
                None,
                CURRENT,
                CONSOLE
            )
            .is_empty()
    );
}

#[test]
fn a_dropped_viewer_reconnects_after_six_seconds() {
    let mut viewer = Viewer::default();
    attached(&mut viewer, 0);
    // The RDP client dropped (not a sign-out).
    assert!(
        viewer
            .poll(
                4000,
                RdpState::Disconnected,
                Some(3),
                None,
                CURRENT,
                CONSOLE
            )
            .is_empty()
    );
    assert_eq!(viewer.phase(), Phase::Reconnecting);
    assert_eq!(viewer.child(), 0);
    assert!(!viewer.gate().accepts(viewer.gate().generation(), 7));
    assert!(
        viewer
            .poll(
                6000,
                RdpState::Disconnected,
                Some(3),
                None,
                CURRENT,
                CONSOLE
            )
            .is_empty()
    );
    assert_eq!(
        viewer.poll(
            6001,
            RdpState::Disconnected,
            Some(3),
            None,
            CURRENT,
            CONSOLE
        ),
        vec![ViewerAction::Connect]
    );
}

#[test]
fn reconnect_disconnects_first_then_connects_at_once() {
    let mut viewer = Viewer::default();
    attached(&mut viewer, 0);
    assert_eq!(
        viewer.reconnect(5000, RdpState::Connected, Some(false)),
        vec![ViewerAction::Disconnect]
    );
    assert_eq!(viewer.child(), 0);
    assert_eq!(
        viewer.poll(
            5100,
            RdpState::Disconnected,
            Some(1),
            None,
            CURRENT,
            CONSOLE
        ),
        vec![ViewerAction::Connect]
    );
    // Disconnected already: connect right away.
    let mut idle = Viewer::default();
    assert_eq!(
        idle.reconnect(0, RdpState::Disconnected, Some(false)),
        vec![ViewerAction::Connect]
    );
}

#[test]
fn signing_out_inside_the_child_session_returns_to_start() {
    for reason in SIGNED_OUT_REASONS {
        let mut viewer = Viewer::default();
        attached(&mut viewer, 0);
        assert!(
            viewer
                .poll(
                    4000,
                    RdpState::Disconnected,
                    Some(reason),
                    None,
                    CURRENT,
                    CONSOLE
                )
                .is_empty()
        );
        assert_eq!(viewer.phase(), Phase::SignedOut);
        assert!(
            viewer
                .poll(
                    60_000,
                    RdpState::Disconnected,
                    Some(reason),
                    None,
                    CURRENT,
                    CONSOLE
                )
                .is_empty()
        );
    }
}

#[test]
fn a_stale_or_foreign_child_id_is_never_attached() {
    let mut viewer = Viewer::default();
    viewer.connect(0, RdpState::Disconnected, Some(false));
    for found in [None, Some(0), Some(CURRENT), Some(CONSOLE), Some(u32::MAX)] {
        viewer.poll(2000, RdpState::Connected, None, found, CURRENT, CONSOLE);
        assert_eq!(viewer.child(), 0, "{found:?}");
        assert!(!viewer.gate().accepts(viewer.gate().generation(), 0));
    }
    let approval = viewer.confirm();
    assert_eq!(
        viewer.end_session(3000, approval, 0, CURRENT, CONSOLE),
        Err(EndError::Changed)
    );
}

#[test]
fn end_session_signs_out_the_confirmed_child_only() {
    let mut viewer = Viewer::default();
    attached(&mut viewer, 0);
    let approval = Confirmation::from_status(viewer.child(), viewer.gate().generation());
    // Windows now reports another child session.
    assert_eq!(
        viewer.end_session(3000, approval, 8, CURRENT, CONSOLE),
        Err(EndError::Changed)
    );
    assert_eq!(
        viewer.end_session(3000, approval, 7, CURRENT, CONSOLE),
        Ok(vec![ViewerAction::SignOut(7)])
    );
    assert_eq!(viewer.phase(), Phase::SigningOut);
    // A second request while signing out does nothing.
    assert!(
        viewer
            .end_session(3000, approval, 7, CURRENT, CONSOLE)
            .is_err()
    );
    // The desktop is still up 15 s later: the viewer drops it.
    assert!(
        viewer
            .poll(10_000, RdpState::Connected, None, Some(7), CURRENT, CONSOLE)
            .is_empty()
    );
    assert_eq!(
        viewer.poll(18_001, RdpState::Connected, None, Some(7), CURRENT, CONSOLE),
        vec![ViewerAction::Disconnect]
    );
    assert!(
        viewer
            .poll(
                19_000,
                RdpState::Disconnected,
                Some(1),
                None,
                CURRENT,
                CONSOLE
            )
            .is_empty()
    );
    assert_eq!(viewer.phase(), Phase::SignedOut);
    assert_eq!(viewer.child(), 0);
}

#[test]
fn an_approval_from_before_a_reattach_is_refused() {
    let mut viewer = Viewer::default();
    attached(&mut viewer, 0);
    let approval = viewer.confirm();
    viewer.poll(
        4000,
        RdpState::Disconnected,
        Some(3),
        None,
        CURRENT,
        CONSOLE,
    );
    viewer.poll(
        10_100,
        RdpState::Disconnected,
        Some(3),
        None,
        CURRENT,
        CONSOLE,
    );
    viewer.poll(12_000, RdpState::Connected, None, Some(7), CURRENT, CONSOLE);
    assert_eq!(viewer.child(), 7);
    assert_eq!(
        viewer.end_session(13_000, approval, 7, CURRENT, CONSOLE),
        Err(EndError::Changed)
    );
}

#[test]
fn a_refused_sign_out_goes_back_to_the_desktop() {
    let mut viewer = Viewer::default();
    attached(&mut viewer, 0);
    let approval = viewer.confirm();
    viewer
        .end_session(3000, approval, 7, CURRENT, CONSOLE)
        .unwrap();
    viewer.end_failed("access denied");
    assert_eq!(viewer.phase(), Phase::Connected);
    assert!(viewer.message().contains("access denied"));
}

#[test]
fn stopping_detaches_and_does_not_reconnect() {
    let mut viewer = Viewer::default();
    attached(&mut viewer, 0);
    assert_eq!(
        viewer.detach(RdpState::Connected),
        vec![ViewerAction::Disconnect]
    );
    assert_eq!(viewer.phase(), Phase::Idle);
    assert!(
        viewer
            .poll(
                60_000,
                RdpState::Disconnected,
                Some(1),
                None,
                CURRENT,
                CONSOLE
            )
            .is_empty()
    );
}

/// The system settings in memory, with every call logged.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Machine {
    values: BTreeMap<(Hive, String, String), u32>,
    child_sessions: bool,
    /// What [`SystemSettings::firewall_problem`] says, None for a firewall on
    /// for every profile that takes local rules.
    firewall_problem: Option<String>,
    /// Profiles where group policy keeps local rules from taking effect.
    policy_overrides: Vec<String>,
    /// Allow rules of the Remote Desktop group group policy turns on.
    policy_allowed: Vec<String>,
    /// Rule ID -> (display name, group, enabled).
    rules: BTreeMap<String, (String, String, bool)>,
    block_rules: BTreeMap<String, u16>,
    /// Accounts whose hive is under HKEY_USERS: signed in, or loaded.
    loaded_hives: BTreeSet<String>,
}
struct Fake {
    machine: Machine,
    log: Rc<RefCell<Vec<String>>>,
    fail_writes: BTreeSet<String>,
}
impl Fake {
    fn new() -> Self {
        let mut values = BTreeMap::new();
        values.insert(
            key(Hive::LocalMachine, TERMINAL_SERVER, "fDenyTSConnections"),
            1,
        );
        values.insert(
            key(
                Hive::User(SID.into()),
                RDP_CLIENT,
                "RemoteDesktop_SuppressWhenMinimized",
            ),
            0,
        );
        // Another user's value is never touched.
        values.insert(
            key(
                Hive::User("S-1-5-21-other".into()),
                RDP_CLIENT,
                "RemoteDesktop_SuppressWhenMinimized",
            ),
            1,
        );
        let mut rules = BTreeMap::new();
        for (id, name, group, enabled) in [
            (
                "RemoteDesktop-UserMode-In-TCP",
                "Remote Desktop - User Mode (TCP-In)",
                REMOTE_DESKTOP_RULES,
                true,
            ),
            (
                "RemoteDesktop-UserMode-In-UDP",
                "Remote Desktop - User Mode (UDP-In)",
                REMOTE_DESKTOP_RULES,
                true,
            ),
            (
                "RemoteDesktop-Shadow-In-TCP",
                "Remote Desktop - Shadow (TCP-In)",
                REMOTE_DESKTOP_RULES,
                false,
            ),
            ("Rubylight-In", "Rubylight", "", true),
        ] {
            rules.insert(id.into(), (name.into(), group.into(), enabled));
        }
        Self {
            machine: Machine {
                values,
                child_sessions: false,
                firewall_problem: None,
                policy_overrides: Vec::new(),
                policy_allowed: Vec::new(),
                rules,
                block_rules: BTreeMap::new(),
                loaded_hives: [SID.to_string(), "S-1-5-21-other".to_string()].into(),
            },
            log: Rc::default(),
            fail_writes: BTreeSet::new(),
        }
    }
    fn note(&self, entry: String) {
        self.log.borrow_mut().push(entry);
    }
}
fn key(hive: Hive, path: &str, name: &str) -> (Hive, String, String) {
    (hive, path.into(), name.into())
}
impl SystemSettings for Fake {
    fn read_dword(&self, hive: &Hive, path: &str, name: &str) -> Result<Option<u32>> {
        Ok(self
            .machine
            .values
            .get(&key(hive.clone(), path, name))
            .copied())
    }
    fn write_dword(&mut self, hive: &Hive, path: &str, name: &str, value: u32) -> Result<()> {
        if self.fail_writes.contains(name) {
            bail!("access denied");
        }
        if let Hive::User(sid) = hive {
            anyhow::ensure!(
                self.machine.loaded_hives.contains(sid),
                "the hive is not loaded"
            );
        }
        self.note(format!("write {name}"));
        self.machine
            .values
            .insert(key(hive.clone(), path, name), value);
        Ok(())
    }
    fn delete_value(&mut self, hive: &Hive, path: &str, name: &str) -> Result<()> {
        self.note(format!("delete {name}"));
        self.machine.values.remove(&key(hive.clone(), path, name));
        Ok(())
    }
    fn child_sessions_enabled(&self) -> Result<bool> {
        Ok(self.machine.child_sessions)
    }
    fn set_child_sessions(&mut self, enabled: bool) -> Result<()> {
        self.note(format!("child sessions {enabled}"));
        self.machine.child_sessions = enabled;
        Ok(())
    }
    fn firewall_problem(&self) -> Result<Option<String>> {
        Ok(self.machine.firewall_problem.clone())
    }
    fn active_firewall(&self, group: &str, block_rule: &str) -> Result<ActiveFirewall> {
        // Local rules take effect only where no group policy overrides them.
        let local = self.machine.policy_overrides.is_empty();
        let mut allowed = self.machine.policy_allowed.clone();
        if local {
            allowed.extend(self.enabled_rules(group)?);
        }
        Ok(ActiveFirewall {
            allowed,
            blocked: local && self.machine.block_rules.contains_key(block_rule),
            unprotected: self.machine.policy_overrides.clone(),
        })
    }
    fn enabled_rules(&self, group: &str) -> Result<Vec<String>> {
        Ok(self
            .machine
            .rules
            .iter()
            .filter(|(_, (_, g, enabled))| g == group && *enabled)
            .map(|(id, _)| id.clone())
            .collect())
    }
    fn set_rule_enabled(&mut self, group: &str, id: &str, enabled: bool) -> Result<()> {
        self.note(format!("rule {id} {enabled}"));
        let rule = self.machine.rules.get_mut(id).context("no such rule")?;
        anyhow::ensure!(rule.1 == group, "{id} is not in {group}");
        rule.2 = enabled;
        Ok(())
    }
    fn add_block_rule(&mut self, name: &str, port: u16) -> Result<()> {
        self.note(format!("block {port}"));
        self.machine.block_rules.insert(name.into(), port);
        Ok(())
    }
    fn remove_rule(&mut self, name: &str) -> Result<()> {
        self.note(format!("remove {name}"));
        self.machine.block_rules.remove(name);
        Ok(())
    }
    fn load_user_hive(&mut self, sid: &str) -> Result<bool> {
        if self.machine.loaded_hives.contains(sid) {
            return Ok(false);
        }
        self.note(format!("load {sid}"));
        self.machine.loaded_hives.insert(sid.into());
        Ok(true)
    }
    fn unload_user_hive(&mut self, sid: &str) -> Result<()> {
        self.note(format!("unload {sid}"));
        anyhow::ensure!(self.machine.loaded_hives.remove(sid), "{sid} is not loaded");
        Ok(())
    }
}

fn set_up(fake: &mut Fake) -> SetupState {
    let saves = fake.log.clone();
    setup(fake, None, SID, |_| {
        saves.borrow_mut().push("save".into());
        Ok(())
    })
    .unwrap()
}

#[test]
fn setup_saves_the_originals_before_the_first_change() {
    let mut fake = Fake::new();
    let state = set_up(&mut fake);
    let log = fake.log.borrow().clone();
    assert_eq!(log.first().map(String::as_str), Some("save"), "{log:?}");
    assert_eq!(
        state,
        SetupState {
            version: 1,
            user_sid: SID.into(),
            deny_ts_connections: Some(1),
            dwm_frame_interval: None,
            suppress_when_minimized: Some(0),
            child_sessions: false,
            remote_desktop_rule_ids: vec![
                "RemoteDesktop-UserMode-In-TCP".into(),
                "RemoteDesktop-UserMode-In-UDP".into(),
            ],
            rdp_port: 3389,
        }
    );
    // What Annex's setup sets, plus the block rule for the RDP port.
    let read = |hive: Hive, path: &str, name: &str| fake.read_dword(&hive, path, name).unwrap();
    assert_eq!(
        read(Hive::LocalMachine, TERMINAL_SERVER, "fDenyTSConnections"),
        Some(0)
    );
    assert_eq!(
        read(Hive::LocalMachine, WINSTATIONS, "DWMFRAMEINTERVAL"),
        Some(8)
    );
    assert_eq!(
        read(
            Hive::User(SID.into()),
            RDP_CLIENT,
            "RemoteDesktop_SuppressWhenMinimized"
        ),
        Some(2)
    );
    assert_eq!(
        read(
            Hive::User("S-1-5-21-other".into()),
            RDP_CLIENT,
            "RemoteDesktop_SuppressWhenMinimized"
        ),
        Some(1)
    );
    assert!(fake.machine.child_sessions);
    assert!(fake.enabled_rules(REMOTE_DESKTOP_RULES).unwrap().is_empty());
    assert_eq!(fake.machine.block_rules.get(BLOCK_RULE), Some(&3389));
    // The block rule is in place before RDP is turned on.
    let position = |entry: &str| log.iter().position(|e| e == entry).unwrap();
    assert!(position("block 3389") < position("write fDenyTSConnections"));
    assert!(missing(&fake, SID).unwrap().is_empty());
}

#[test]
fn uninstall_restores_every_original_value() {
    let mut fake = Fake::new();
    fake.machine
        .values
        .insert(key(Hive::LocalMachine, RDP_TCP, "PortNumber"), 3390);
    let before = fake.machine.clone();
    let state = set_up(&mut fake);
    assert_eq!(fake.machine.block_rules.get(BLOCK_RULE), Some(&3390));
    assert_ne!(fake.machine, before);
    // The state goes through its file between setup and uninstall.
    let state: SetupState = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    restore(&mut fake, &state).unwrap();
    assert_eq!(fake.machine, before);
    assert!(
        fake.log
            .borrow()
            .contains(&"delete DWMFRAMEINTERVAL".to_string())
    );
}

#[test]
fn restore_turns_on_only_the_rules_that_were_on_when_names_repeat() {
    // Two rules can share a display name (one per profile, or a copy); only
    // the one that was on before setup comes back on.
    let mut fake = Fake::new();
    fake.machine.rules.insert(
        "RemoteDesktop-UserMode-In-TCP-Copy".into(),
        (
            "Remote Desktop - User Mode (TCP-In)".into(),
            REMOTE_DESKTOP_RULES.into(),
            false,
        ),
    );
    let before = fake.machine.clone();
    let state = set_up(&mut fake);
    assert!(fake.enabled_rules(REMOTE_DESKTOP_RULES).unwrap().is_empty());
    restore(&mut fake, &state).unwrap();
    assert_eq!(fake.machine, before);
    assert!(
        !fake
            .log
            .borrow()
            .contains(&"rule RemoteDesktop-UserMode-In-TCP-Copy true".to_string())
    );
}

#[test]
fn setting_up_again_keeps_the_first_originals() {
    let mut fake = Fake::new();
    let before = fake.machine.clone();
    let first = set_up(&mut fake);
    // A repair after setup: the values are Rubylight's now, the record is not.
    let again = setup(&mut fake, Some(first.clone()), SID, |_| {
        panic!("the originals are saved only once")
    })
    .unwrap();
    assert_eq!(again, first);
    restore(&mut fake, &again).unwrap();
    assert_eq!(fake.machine, before);
}

#[test]
fn setup_refuses_with_the_firewall_off_and_changes_nothing() {
    let mut fake = Fake::new();
    fake.machine.firewall_problem = Some("Windows Firewall is off for the Public profile".into());
    let before = fake.machine.clone();
    let error = setup(&mut fake, None, SID, |_| panic!("nothing is saved"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("Windows Firewall is off"), "{error}");
    assert_eq!(fake.machine, before);
    assert!(fake.log.borrow().is_empty());
}

#[test]
fn setup_never_turns_on_rdp_when_group_policy_overrides_the_firewall_rules() {
    let rdp = |fake: &Fake| {
        fake.read_dword(&Hive::LocalMachine, TERMINAL_SERVER, "fDenyTSConnections")
            .unwrap()
    };
    // Local rules ignored for a profile: the block rule would not take effect.
    let mut fake = Fake::new();
    fake.machine.policy_overrides = vec!["Domain".into()];
    let error = format!("{:#}", set_up_err(&mut fake));
    assert!(
        error.contains("group policy") && error.contains("Domain"),
        "{error}"
    );
    assert!(error.contains("RDP was not turned on"), "{error}");
    assert_eq!(rdp(&fake), Some(1));
    assert!(!fake.log.borrow().iter().any(|e| e.starts_with("write ")));
    // A Remote Desktop allow rule group policy keeps on.
    let mut fake = Fake::new();
    fake.machine.policy_allowed = vec!["{gpo-rdp}".into()];
    let error = format!("{:#}", set_up_err(&mut fake));
    assert!(error.contains("{gpo-rdp}"), "{error}");
    assert_eq!(rdp(&fake), Some(1));
    // Either way, restore puts back what setup changed before it stopped.
    let state = record(&Fake::new(), SID).unwrap();
    restore(&mut fake, &state).unwrap();
    assert!(fake.machine.block_rules.is_empty());
}
fn set_up_err(fake: &mut Fake) -> anyhow::Error {
    setup(fake, None, SID, |_| Ok(())).unwrap_err()
}

#[test]
fn the_firewall_is_checked_again_after_setup() {
    let mut fake = Fake::new();
    set_up(&mut fake);
    assert!(firewall_missing(&fake).unwrap().is_empty());
    // Someone removes the block rule, or group policy changes later: the
    // host's start and every child-session start see it.
    fake.machine.block_rules.clear();
    let found = missing(&fake, SID).unwrap();
    assert!(
        found
            .iter()
            .any(|m| m.contains(BLOCK_RULE) && m.contains("not in effect")),
        "{found:?}"
    );
    set_up(&mut fake);
    fake.machine.policy_overrides = vec!["Public".into()];
    let found = firewall_missing(&fake).unwrap();
    assert!(found.iter().any(|m| m.contains("Public")), "{found:?}");
}

#[test]
fn setup_for_another_account_is_refused() {
    let mut fake = Fake::new();
    let state = set_up(&mut fake);
    let error = setup(&mut fake, Some(state), "S-1-5-21-other", |_| Ok(()))
        .unwrap_err()
        .to_string();
    assert!(error.contains("another Windows account"), "{error}");
    assert!(setup(&mut fake, None, " ", |_| Ok(())).is_err());
}

#[test]
fn a_failed_save_stops_setup_before_any_change() {
    let mut fake = Fake::new();
    let before = fake.machine.clone();
    assert!(setup(&mut fake, None, SID, |_| bail!("disk full")).is_err());
    assert_eq!(fake.machine, before);
}

#[test]
fn restore_finishes_every_step_and_reports_what_failed() {
    let mut fake = Fake::new();
    let before = fake.machine.clone();
    let state = set_up(&mut fake);
    fake.fail_writes.insert("fDenyTSConnections".into());
    let error = format!("{:#}", restore(&mut fake, &state).unwrap_err());
    assert!(error.contains("fDenyTSConnections"), "{error}");
    assert!(error.contains("firewall rules stay"), "{error}");
    // Everything else is back, but RDP is still on: the firewall keeps it on
    // this PC, with the block rule on and Remote Desktop's own rules off.
    let mut expected = before.clone();
    expected.values.insert(
        key(Hive::LocalMachine, TERMINAL_SERVER, "fDenyTSConnections"),
        0,
    );
    expected.block_rules.insert(BLOCK_RULE.into(), 3389);
    for id in &state.remote_desktop_rule_ids {
        expected.rules.get_mut(id).unwrap().2 = false;
    }
    assert_eq!(fake.machine, expected);
    // Once RDP can be restored, so can the firewall.
    fake.fail_writes.clear();
    restore(&mut fake, &state).unwrap();
    assert_eq!(fake.machine, before);
}

#[test]
fn restore_loads_a_signed_out_accounts_hive_and_unloads_it_after() {
    let mut fake = Fake::new();
    let before = fake.machine.clone();
    let state = set_up(&mut fake);
    // The account signed out, or the uninstaller runs as another one.
    fake.machine.loaded_hives.remove(SID);
    fake.log.borrow_mut().clear();
    restore(&mut fake, &state).unwrap();
    let log = fake.log.borrow().clone();
    let position = |entry: &str| log.iter().position(|e| e == entry).unwrap();
    assert!(
        position(&format!("load {SID}")) < position("write RemoteDesktop_SuppressWhenMinimized")
    );
    assert!(
        position("write RemoteDesktop_SuppressWhenMinimized") < position(&format!("unload {SID}"))
    );
    let mut expected = before;
    expected.loaded_hives.remove(SID);
    assert_eq!(fake.machine, expected);
    // A hive that was loaded already stays loaded.
    fake.machine.loaded_hives.insert(SID.into());
    let state = set_up(&mut fake);
    fake.log.borrow_mut().clear();
    restore(&mut fake, &state).unwrap();
    assert!(!fake.log.borrow().iter().any(|e| e.contains("load")));
    assert!(fake.machine.loaded_hives.contains(SID));
}

#[test]
fn missing_lists_what_setup_still_has_to_do() {
    let fake = Fake::new();
    let missing = missing(&fake, SID).unwrap();
    assert!(missing.iter().any(|m| m.contains("child sessions are off")));
    assert!(missing.iter().any(|m| m.contains("DWMFRAMEINTERVAL")));
    assert!(missing.iter().any(|m| m.contains("firewall rules are on")));
}

/// DPAPI stand-in: XOR with the user's key, tagged with the user.
struct FakeDpapi(&'static str);
impl Dpapi for FakeDpapi {
    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>> {
        let mut blob = self.0.as_bytes().to_vec();
        blob.push(0);
        blob.extend(plain.iter().map(|byte| byte ^ 0x5a));
        Ok(blob)
    }
    fn unprotect(&self, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let tag = self.0.len();
        if blob.len() <= tag || &blob[..tag] != self.0.as_bytes() || blob[tag] != 0 {
            bail!("the data is invalid");
        }
        Ok(Zeroizing::new(
            blob[tag + 1..].iter().map(|byte| byte ^ 0x5a).collect(),
        ))
    }
}

#[test]
fn the_password_roundtrips_through_dpapi_and_can_be_forgotten() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("child-session").join("password.bin");
    let store = PasswordStore::new(path.clone(), FakeDpapi(SID));
    assert!(store.load().unwrap().is_none());
    assert!(!store.saved());
    store.save(&Secret::new(PASSWORD.into())).unwrap();
    assert!(store.saved());
    let on_disk = std::fs::read(&path).unwrap();
    assert!(
        !on_disk
            .windows(PASSWORD.len())
            .any(|w| w == PASSWORD.as_bytes())
    );
    assert_eq!(store.load().unwrap().unwrap().expose(), PASSWORD);
    // Another Windows user can't decrypt it, and the error says what to do.
    let other = PasswordStore::new(path.clone(), FakeDpapi("S-1-5-21-other"));
    let error = format!("{:#}", other.load().unwrap_err());
    assert!(error.contains("forget it"), "{error}");
    assert!(!error.contains(PASSWORD));
    assert!(store.forget().unwrap());
    assert!(!store.forget().unwrap());
    assert!(store.load().unwrap().is_none());
    assert!(store.save(&Secret::new(String::new())).is_err());
}

/// Collects everything a tracing subscriber writes.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn the_password_never_reaches_a_log() {
    let secret = Secret::new(PASSWORD.into());
    let command = ViewerCommand::Start {
        display: DisplaySettings::default(),
        password: Some(secret.clone()),
    };
    assert_eq!(format!("{secret}"), "[redacted]");
    assert_eq!(format!("{secret:?}"), "[redacted]");
    assert!(!format!("{command:?}").contains(PASSWORD));
    assert!(!format!("{command:#?}").contains(PASSWORD));
    // A request body with the password parses straight into a Secret.
    let body: BTreeMap<String, Secret> =
        serde_json::from_str(&format!(r#"{{"password":"{PASSWORD}"}}"#)).unwrap();
    assert!(!format!("{body:?}").contains(PASSWORD));

    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(?command, %secret, password = ?secret, "viewer command");
        tracing::warn!("sending {command:?} with {secret}");
        tracing::debug!(?body, "console request");
    });
    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(log.contains("[redacted]"), "{log}");
    assert!(log.contains("viewer command"), "{log}");
    assert!(!log.contains(PASSWORD), "{log}");
}

#[test]
fn a_console_request_password_unescapes_like_serde_json() {
    for password in [
        PASSWORD,
        "quo\"te back\\slash / slash",
        "\u{8}\u{c}\n\r\t\u{1}",
        "caf\u{e9} \u{1f600} \u{10ffff}",
    ] {
        // serde_json's own escaping, and every escape spelled out as \u.
        let literal = serde_json::to_string(password).unwrap();
        assert_eq!(
            Secret::from_json_string(&literal).unwrap().expose(),
            password
        );
        let mut spelled = String::from('"');
        for unit in password.encode_utf16() {
            spelled.push_str(&format!("\\u{unit:04X}"));
        }
        spelled.push('"');
        assert_eq!(
            Secret::from_json_string(&spelled).unwrap().expose(),
            password
        );
    }
    assert_eq!(Secret::from_json_string(r#""\/""#).unwrap().expose(), "/");
    for bad in [
        "",
        "\"",
        "5",
        "null",
        r#""a"b""#,
        r#""a\""#,
        r#""\x""#,
        r#""\u12""#,
        r#""\ud83d""#,
        r#""\ude00""#,
        r#""\ud83dA""#,
        "\"tab\tin\"",
    ] {
        assert!(Secret::from_json_string(bad).is_err(), "{bad}");
    }
}

#[test]
fn viewer_commands_carry_the_password_only_on_the_wire() {
    let command = ViewerCommand::Start {
        display: DisplaySettings::parse("2560x1440", "125").unwrap(),
        password: Some(Secret::new(PASSWORD.into())),
    };
    let line = command.line().unwrap();
    assert!(line.ends_with('\n'));
    assert!(!line.contains(PASSWORD), "the password travels as hex");
    assert!(line.contains(&hex::encode(PASSWORD)));
    match serde_json::from_str(line.trim_end()).unwrap() {
        ViewerCommand::Start { display, password } => {
            assert_eq!(display.scale, 125);
            assert_eq!(password.unwrap().expose(), PASSWORD);
        }
        other => panic!("{other:?}"),
    }
    // Text JSON would escape: as hex, the line needs no escapes at all, so
    // serde reads it without a scratch copy.
    let awkward = "quo\"te back\\slash \u{e9}\u{1f600}\ttab";
    let line = ViewerCommand::Start {
        display: DisplaySettings::default(),
        password: Some(Secret::new(awkward.into())),
    }
    .line()
    .unwrap();
    assert!(!line.trim_end().contains('\\'), "{}", line.as_str());
    match serde_json::from_str(line.trim_end()).unwrap() {
        ViewerCommand::Start { password, .. } => assert_eq!(password.unwrap().expose(), awkward),
        other => panic!("{other:?}"),
    }
    for bad in [r#""zz""#, r#""abc""#, "5"] {
        let error = serde_json::from_str::<ViewerCommand>(&format!(
            r#"{{"command":"start","display":{{"width":1920,"height":1080,"scale":100}},"password":{bad}}}"#
        ))
        .unwrap_err();
        assert!(!error.to_string().contains("zz"), "{error}");
    }
    let saved: ViewerCommand = serde_json::from_str(
        r#"{"command":"start","display":{"width":1920,"height":1080,"scale":100}}"#,
    )
    .unwrap();
    assert!(matches!(saved, ViewerCommand::Start { password: None, .. }));
    let end: ViewerCommand =
        serde_json::from_str(r#"{"command":"end_session","child":7,"generation":3}"#).unwrap();
    assert!(matches!(
        end,
        ViewerCommand::EndSession {
            child: 7,
            generation: 3
        }
    ));
    let status = ViewerStatus {
        phase: Phase::Connected,
        message: "attached".into(),
        child: 7,
        generation: 3,
        password_saved: true,
        notice: None,
    };
    let text = serde_json::to_string(&status).unwrap();
    assert!(text.contains(r#""phase":"connected""#));
    assert_eq!(serde_json::from_str::<ViewerStatus>(&text).unwrap(), status);
}

fn adapter(name: &str, vendor: u32, dedicated_mib: u64, uma: Option<bool>) -> Adapter {
    Adapter {
        name: name.into(),
        vendor,
        dedicated_memory: dedicated_mib << 20,
        software: false,
        uma,
    }
}

#[test]
fn an_integrated_gpu_beside_a_discrete_one_is_called_out() {
    let dgpu = adapter("AMD Radeon RX 7900 XT", 0x1002, 20_464, Some(false));
    let igpu = adapter("AMD Radeon(TM) Graphics", 0x1002, 512, Some(true));
    let basic = Adapter {
        name: "Microsoft Basic Render Driver".into(),
        vendor: 0x1414,
        dedicated_memory: 0,
        software: true,
        uma: Some(true),
    };
    let all = [igpu.clone(), dgpu.clone(), basic.clone()];

    let verdict = gpu_verdict(Some(&dgpu), &all);
    assert_eq!(
        verdict,
        GpuVerdict::Discrete {
            name: dgpu.name.clone()
        }
    );
    assert_eq!(gpu_problem(&verdict), None);

    let verdict = gpu_verdict(Some(&igpu), &all);
    assert_eq!(
        verdict,
        GpuVerdict::Integrated {
            name: igpu.name.clone(),
            discrete: Some(dgpu.name.clone())
        }
    );
    let problem = gpu_problem(&verdict).unwrap();
    assert!(
        problem.contains("integrated GPU AMD Radeon(TM) Graphics, not on AMD Radeon RX 7900 XT")
    );
    assert!(
        problem.contains("Use hardware graphics adapters for all Remote Desktop Services sessions")
    );
    assert!(problem.contains("High performance"));
    assert!(problem.contains("never encodes in software"));

    let verdict = gpu_verdict(Some(&basic), &all);
    assert_eq!(
        verdict,
        GpuVerdict::Software {
            discrete: Some(dgpu.name.clone())
        }
    );
    assert!(
        gpu_problem(&verdict)
            .unwrap()
            .contains("Basic Render Driver")
    );
    assert!(gpu_problem(&gpu_verdict(None, &all)).is_some());
}

#[test]
fn an_apu_without_a_discrete_gpu_is_fine() {
    let apu = adapter("AMD Radeon 780M Graphics", 0x1002, 512, Some(true));
    let verdict = gpu_verdict(Some(&apu), std::slice::from_ref(&apu));
    assert_eq!(
        verdict,
        GpuVerdict::Integrated {
            name: apu.name.clone(),
            discrete: None
        }
    );
    assert_eq!(gpu_problem(&verdict), None);
}

#[test]
fn without_d3d12_memory_size_decides_integrated() {
    assert!(adapter("small", 0x1002, 128, None).is_integrated());
    assert!(!adapter("big", 0x1002, 16_384, None).is_integrated());
    // D3D12's answer wins over the memory size.
    assert!(!adapter("small but discrete", 0x1002, 256, Some(false)).is_integrated());
}

#[test]
fn only_the_console_users_own_child_session_is_attached() {
    assert!(check_child_owner(5, 1, SID, SID).is_ok());
    // SIDs compare without case.
    assert!(check_child_owner(5, 1, &SID.to_lowercase(), SID).is_ok());
    let other = check_child_owner(5, 1, "S-1-5-21-other", SID)
        .unwrap_err()
        .to_string();
    assert!(other.contains("another Windows account"), "{other}");
    for (child, console) in [(0, 1), (1, 1), (u32::MAX, 1), (0x8000_0000, 1)] {
        assert!(check_child_owner(child, console, SID, SID).is_err());
    }
    assert!(check_child_owner(5, 1, "", "").is_err());
}

#[test]
fn the_child_host_gets_its_own_ports_and_name() {
    let config: BTreeMap<_, _> = child_host_config("Gaming PC", 47989).into_iter().collect();
    assert_eq!(config["port"], "48989");
    assert_eq!(config["sunshine_name"], "Gaming PC (background)");
    assert_eq!(config["webrtc_port"], "8100");
    assert_eq!(config["webrtc_media_port"], "8101");
    assert_eq!(config["capture"], "wgc");
    assert_eq!(config[CHILD_HOST_KEY], "true");
    assert_eq!(config["virtual_display_mode"], "disabled");
    let config: BTreeMap<_, _> = child_host_config("", CHILD_BASE_PORT).into_iter().collect();
    assert_eq!(config["port"], "49989");
    assert_eq!(config["sunshine_name"], "Rubylight (background)");
    // Every port of the child host stays clear of the main host's.
    let main = crate::config::Ports::from_base(47989);
    let child = crate::config::Ports::from_base(CHILD_BASE_PORT);
    let ports = |p: &crate::config::Ports| {
        [
            p.http, p.https, p.web, p.video, p.control, p.audio, p.mic, p.rtsp,
        ]
    };
    for port in ports(&child) {
        assert!(!ports(&main).contains(&port), "{port}");
        assert!(!(48518..=48544).contains(&port), "{port}");
    }
}
