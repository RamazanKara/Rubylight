use super::*;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

const SID: &str = "S-1-5-21-1000-2000-3000-1001";

/// The system settings in memory, with every call logged.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Machine {
    values: BTreeMap<(Hive, String, String), u32>,
    child_sessions: bool,
    /// Rule ID -> (display name, group, enabled).
    rules: BTreeMap<String, (String, String, bool)>,
    /// The block rules' names.
    block_rules: BTreeSet<String>,
    /// Accounts whose hive is under HKEY_USERS: signed in, or loaded.
    loaded_hives: BTreeSet<String>,
}
struct Fake {
    machine: Machine,
    log: Rc<RefCell<Vec<String>>>,
    fail_writes: BTreeSet<String>,
    /// Writes that succeed without changing the value (a value something
    /// else puts back at once).
    stuck_writes: BTreeSet<String>,
    /// The firewall's rules cannot be read (COM failing).
    firewall_unreadable: bool,
    /// Switching child sessions succeeds while they stay as they are.
    child_sessions_stuck: bool,
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
                rules,
                block_rules: BTreeSet::new(),
                loaded_hives: [SID.to_string(), "S-1-5-21-other".to_string()].into(),
            },
            log: Rc::default(),
            fail_writes: BTreeSet::new(),
            stuck_writes: BTreeSet::new(),
            firewall_unreadable: false,
            child_sessions_stuck: false,
        }
    }
    fn note(&self, entry: String) {
        self.log.borrow_mut().push(entry);
    }
    fn enabled_rules(&self, group: &str) -> Vec<String> {
        self.machine
            .rules
            .iter()
            .filter(|(_, (_, g, enabled))| g == group && *enabled)
            .map(|(id, _)| id.clone())
            .collect()
    }
    fn rdp(&self) -> Option<u32> {
        self.read_dword(&Hive::LocalMachine, TERMINAL_SERVER, "fDenyTSConnections")
            .unwrap()
    }
}
fn key(hive: Hive, path: &str, name: &str) -> (Hive, String, String) {
    (hive, path.into(), name.into())
}
impl Registry for Fake {
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
        if self.stuck_writes.contains(name) {
            return Ok(());
        }
        self.machine
            .values
            .insert(key(hive.clone(), path, name), value);
        Ok(())
    }
}
impl SystemSettings for Fake {
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
        if !self.child_sessions_stuck {
            self.machine.child_sessions = enabled;
        }
        Ok(())
    }
    fn block_rule_present(&self, name: &str) -> Result<bool> {
        if self.firewall_unreadable {
            bail!("opening Windows Firewall failed");
        }
        Ok(self.machine.block_rules.contains(name))
    }
    fn enable_rule(&mut self, group: &str, id: &str) -> Result<()> {
        self.note(format!("rule {id} true"));
        let rule = self.machine.rules.get_mut(id).context("no such rule")?;
        anyhow::ensure!(rule.1 == group, "{id} is not in {group}");
        rule.2 = true;
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

/// [`restore`] of a copy of `state`, with nothing saved.
fn restore_copy(fake: &mut Fake, state: &SetupState) -> Result<()> {
    restore(fake, &mut state.clone(), |_| Ok(()))
}

/// What the removed setup recorded and then changed on `fake`.
fn set_up(fake: &mut Fake) -> SetupState {
    let read = |change: &Change| {
        fake.read_dword(&hive(change, SID), change.path, change.name)
            .unwrap()
    };
    let state = SetupState {
        version: 1,
        user_sid: SID.into(),
        deny_ts_connections: read(&CHANGES[0]),
        dwm_frame_interval: read(&CHANGES[1]),
        suppress_when_minimized: read(&CHANGES[2]),
        child_sessions: fake.machine.child_sessions,
        remote_desktop_rule_ids: fake.enabled_rules(REMOTE_DESKTOP_RULES),
        rdp_port: 3389,
        rdp_restored: false,
    };
    fake.machine.block_rules.insert(BLOCK_RULE.into());
    for id in &state.remote_desktop_rule_ids {
        fake.machine.rules.get_mut(id).unwrap().2 = false;
    }
    for change in &CHANGES {
        fake.machine.values.insert(
            key(hive(change, SID), change.path, change.name),
            change.value,
        );
    }
    fake.machine.child_sessions = true;
    state
}

#[test]
fn uninstall_restores_every_original_value() {
    let mut fake = Fake::new();
    let before = fake.machine.clone();
    let state = set_up(&mut fake);
    assert_ne!(fake.machine, before);
    // The state goes through its file between setup and uninstall.
    let state: SetupState = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    restore_copy(&mut fake, &state).unwrap();
    assert_eq!(fake.machine, before);
    assert!(
        fake.log
            .borrow()
            .contains(&"delete DWMFRAMEINTERVAL".to_string())
    );
}

#[test]
fn this_pcs_record_reads_and_restores() {
    // As child-session setup left it on the release test PC.
    let state: SetupState = serde_json::from_str(
        r#"{"version":1,"user_sid":"S-1-5-21-1000-2000-3000-1001","deny_ts_connections":1,"dwm_frame_interval":null,"suppress_when_minimized":null,"child_sessions":false,"remote_desktop_rule_ids":[],"rdp_port":3389}"#,
    )
    .unwrap();
    let mut fake = Fake::new();
    fake.machine.values.remove(&key(
        Hive::User(SID.into()),
        RDP_CLIENT,
        "RemoteDesktop_SuppressWhenMinimized",
    ));
    for rule in fake.machine.rules.values_mut() {
        rule.2 &= rule.1 != REMOTE_DESKTOP_RULES;
    }
    let before = fake.machine.clone();
    assert_eq!(set_up(&mut fake), state);
    restore_copy(&mut fake, &state).unwrap();
    assert_eq!(fake.machine, before);
    assert_eq!(fake.rdp(), Some(1));
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
    assert!(fake.enabled_rules(REMOTE_DESKTOP_RULES).is_empty());
    restore_copy(&mut fake, &state).unwrap();
    assert_eq!(fake.machine, before);
    assert!(
        !fake
            .log
            .borrow()
            .contains(&"rule RemoteDesktop-UserMode-In-TCP-Copy true".to_string())
    );
}

#[test]
fn the_setup_record_is_one_for_the_pc() {
    let record = setup_record();
    assert!(record.ends_with(r"Butterpollo\windows-settings\child_session_setup.json"));
    assert!(!record.starts_with(crate::paths::installed_profile()));
}

#[test]
fn child_sessions_that_stay_on_after_restore_are_a_failure() {
    let mut fake = Fake::new();
    let mut state = set_up(&mut fake);
    fake.child_sessions_stuck = true;
    let error = format!(
        "{:#}",
        restore(&mut fake, &mut state, |_| Ok(())).unwrap_err()
    );
    assert!(
        error.contains("child sessions: they read true after restoring false"),
        "{error}"
    );
    // The rest is back.
    assert_eq!(fake.rdp(), Some(1));
    assert!(fake.machine.block_rules.is_empty());
    // Once Windows switches them, restoring again finishes.
    fake.child_sessions_stuck = false;
    restore(&mut fake, &mut state, |_| Ok(())).unwrap();
    assert!(!fake.machine.child_sessions);
}

#[test]
fn a_setup_whose_record_is_gone_turns_rdp_off_then_removes_its_rule() {
    // Never set up: the user's own RDP stays as it is, on or off.
    let mut own = Fake::new();
    own.machine.values.insert(
        key(Hive::LocalMachine, TERMINAL_SERVER, "fDenyTSConnections"),
        0,
    );
    assert!(enforce_unrecorded(&mut own).unwrap().is_empty());
    assert_eq!(own.rdp(), Some(0));
    assert!(own.log.borrow().is_empty());
    // Set up, then the record went: setup's block rule says so, RDP goes off, then the rule.
    let mut fake = Fake::new();
    set_up(&mut fake);
    assert_eq!(fake.rdp(), Some(0));
    let found = enforce_unrecorded(&mut fake).unwrap();
    assert_eq!(fake.rdp(), Some(1));
    assert!(fake.machine.block_rules.is_empty());
    assert!(
        found.len() == 3
            && found[0].contains(BLOCK_RULE)
            && found[1].contains("Remote Desktop was turned off (fDenyTSConnections = 1)")
            && found[2].contains("was removed"),
        "{found:?}"
    );
    let log = fake.log.borrow().clone();
    assert_eq!(
        log,
        [
            "write fDenyTSConnections".to_string(),
            format!("remove {BLOCK_RULE}")
        ]
    );
    // RDP off already: only the rule goes.
    let mut off = Fake::new();
    off.machine.block_rules.insert(BLOCK_RULE.into());
    let found = enforce_unrecorded(&mut off).unwrap();
    assert_eq!((off.rdp(), found.len()), (Some(1), 2), "{found:?}");
    assert!(off.machine.block_rules.is_empty());
    // One that cannot turn it off says so, and keeps the rule.
    let mut stuck = Fake::new();
    set_up(&mut stuck);
    stuck.fail_writes.insert("fDenyTSConnections".into());
    let found = enforce_unrecorded(&mut stuck).unwrap();
    assert_eq!(stuck.rdp(), Some(0));
    assert!(stuck.machine.block_rules.contains(BLOCK_RULE));
    assert!(
        found[1].contains("turning Remote Desktop off failed") && found[2].contains("stays"),
        "{found:?}"
    );
    // Rules that cannot be read change nothing: without a record, RDP may be the user's own.
    let mut unreadable = Fake::new();
    set_up(&mut unreadable);
    unreadable.firewall_unreadable = true;
    let error = enforce_unrecorded(&mut unreadable).unwrap_err();
    assert!(
        format!("{error:#}").contains("opening Windows Firewall failed"),
        "{error:#}"
    );
    assert_eq!(unreadable.rdp(), Some(0));
}

#[test]
fn turning_rdp_off_is_read_back_and_policy_cannot_keep_it_on() {
    // The write succeeds, but the value stays 0.
    let mut fake = Fake::new();
    set_up(&mut fake);
    fake.stuck_writes.insert("fDenyTSConnections".into());
    let done = turn_rdp_off(&mut fake);
    assert!(
        done.contains("turning Remote Desktop off failed")
            && done.contains("reads Some(0) after writing 1"),
        "{done}"
    );
    // Group policy's value wins over the one written here.
    let mut fake = Fake::new();
    set_up(&mut fake);
    fake.machine.values.insert(
        key(
            Hive::LocalMachine,
            TERMINAL_SERVICES_POLICY,
            "fDenyTSConnections",
        ),
        0,
    );
    let done = turn_rdp_off(&mut fake);
    assert!(
        done.contains("turning Remote Desktop off failed") && done.contains("group policy"),
        "{done}"
    );
    // A policy that turns RDP off, or none, is fine.
    fake.machine.values.insert(
        key(
            Hive::LocalMachine,
            TERMINAL_SERVICES_POLICY,
            "fDenyTSConnections",
        ),
        1,
    );
    assert!(
        turn_rdp_off(&mut fake).contains("Remote Desktop was turned off"),
        "policy 1"
    );
}

#[test]
fn restore_finishes_every_step_and_reports_what_failed() {
    let mut fake = Fake::new();
    let before = fake.machine.clone();
    let state = set_up(&mut fake);
    fake.fail_writes.insert("fDenyTSConnections".into());
    let error = format!("{:#}", restore_copy(&mut fake, &state).unwrap_err());
    assert!(error.contains("fDenyTSConnections"), "{error}");
    assert!(error.contains("firewall rules stay"), "{error}");
    // Everything else is back, but RDP is still on: the firewall keeps it on
    // this PC, with the block rule on and Remote Desktop's own rules off.
    let mut expected = before.clone();
    expected.values.insert(
        key(Hive::LocalMachine, TERMINAL_SERVER, "fDenyTSConnections"),
        0,
    );
    expected.block_rules.insert(BLOCK_RULE.into());
    for id in &state.remote_desktop_rule_ids {
        expected.rules.get_mut(id).unwrap().2 = false;
    }
    assert_eq!(fake.machine, expected);
    // Once RDP can be restored, so can the firewall.
    fake.fail_writes.clear();
    restore_copy(&mut fake, &state).unwrap();
    assert_eq!(fake.machine, before);
}

const USER_MODE_UDP: &str = "RemoteDesktop-UserMode-In-UDP";
/// Set up, then a restore that puts RDP back and the block rule away but
/// fails at SuppressWhenMinimized and at turning [`USER_MODE_UDP`] back on
/// (gone meanwhile; back, off, after). What it saved.
fn restore_partly(fake: &mut Fake) -> SetupState {
    let mut state = set_up(fake);
    let udp = fake.machine.rules.remove(USER_MODE_UDP).unwrap();
    fake.fail_writes
        .insert("RemoteDesktop_SuppressWhenMinimized".into());
    let mut saves = Vec::new();
    let error = format!(
        "{:#}",
        restore(fake, &mut state, |state| {
            saves.push(state.clone());
            Ok(())
        })
        .unwrap_err()
    );
    assert!(
        error.contains("RemoteDesktop_SuppressWhenMinimized") && error.contains(USER_MODE_UDP),
        "{error}"
    );
    fake.machine.rules.insert(USER_MODE_UDP.into(), udp);
    fake.fail_writes.clear();
    // Saved once, before child sessions.
    assert_eq!(saves, [state.clone()]);
    assert!(state.rdp_restored);
    assert_eq!(state.remote_desktop_rule_ids, [USER_MODE_UDP]);
    assert_eq!(fake.rdp(), Some(1));
    assert!(fake.machine.block_rules.is_empty());
    // It goes through its file, and an unflagged record has no such field.
    let text = serde_json::to_string(&state).unwrap();
    assert!(text.contains("rdp_restored"), "{text}");
    let mut unflagged = state.clone();
    unflagged.rdp_restored = false;
    assert!(
        !serde_json::to_string(&unflagged)
            .unwrap()
            .contains("rdp_restored")
    );
    serde_json::from_str(&text).unwrap()
}

#[test]
fn a_restore_that_stops_after_rdp_resumes_without_redoing_it() {
    let mut fake = Fake::new();
    let before = fake.machine.clone();
    let mut state = restore_partly(&mut fake);
    // The user turns RDP on meanwhile.
    let deny = key(Hive::LocalMachine, TERMINAL_SERVER, "fDenyTSConnections");
    fake.machine.values.insert(deny.clone(), 0);
    fake.log.borrow_mut().clear();
    let mut saves = Vec::new();
    restore(&mut fake, &mut state, |state| {
        saves.push(state.clone());
        Ok(())
    })
    .unwrap();
    let log = fake.log.borrow().clone();
    assert!(
        !log.iter()
            .any(|entry| entry.contains("fDenyTSConnections") || entry.starts_with("remove")),
        "{log:?}"
    );
    let rules: Vec<_> = log
        .iter()
        .filter(|entry| entry.starts_with("rule"))
        .collect();
    assert_eq!(rules, [&format!("rule {USER_MODE_UDP} true")]);
    // The rule that came back is no longer kept.
    assert!(saves.len() == 1 && saves[0].remote_desktop_rule_ids.is_empty());
    let mut expected = before;
    expected.values.insert(deny, 0);
    assert_eq!(fake.machine, expected);
}

#[test]
fn restore_loads_a_signed_out_accounts_hive_and_unloads_it_after() {
    let mut fake = Fake::new();
    let before = fake.machine.clone();
    let state = set_up(&mut fake);
    // The account signed out, or the uninstaller runs as another one.
    fake.machine.loaded_hives.remove(SID);
    fake.log.borrow_mut().clear();
    restore_copy(&mut fake, &state).unwrap();
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
    restore_copy(&mut fake, &state).unwrap();
    assert!(!fake.log.borrow().iter().any(|e| e.contains("load")));
    assert!(fake.machine.loaded_hives.contains(SID));
}

/// A wipe through the file system: zeros, then the file goes.
fn wipe(path: &Path) -> std::io::Result<bool> {
    let length = match std::fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    std::fs::write(path, vec![0u8; length as usize])?;
    std::fs::remove_file(path)?;
    Ok(true)
}
fn remove_empty(folder: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir(folder) {
        Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => Ok(()),
        removed => removed,
    }
}

#[test]
fn saved_passwords_go_from_every_profile_and_only_empty_folders_with_them() {
    let root = tempfile::tempdir().unwrap();
    let profile = |name: &str| root.path().join(name);
    let folder = |name: &str| {
        profile(name)
            .join("AppData")
            .join("Local")
            .join(PASSWORD_FOLDER)
    };
    for name in ["ramaz", "other", "kept"] {
        std::fs::create_dir_all(folder(name)).unwrap();
    }
    std::fs::write(folder("ramaz").join(PASSWORD_FILES[0]), b"blob").unwrap();
    std::fs::write(folder("ramaz").join(PASSWORD_FILES[1]), b"half").unwrap();
    std::fs::write(folder("other").join(PASSWORD_FILES[0]), b"blob").unwrap();
    std::fs::write(folder("other").join("settings.json"), b"{}").unwrap();
    // A redirected AppData\Local, given as the console user's.
    let redirected = root.path().join("D").join("Local");
    std::fs::create_dir_all(redirected.join(PASSWORD_FOLDER)).unwrap();
    std::fs::write(
        redirected.join(PASSWORD_FOLDER).join(PASSWORD_FILES[0]),
        b"blob",
    )
    .unwrap();
    let folders = password_folders(
        &[
            profile("ramaz"),
            profile("other"),
            profile("kept"),
            profile("missing"),
            // Relative, or with `..`: skipped.
            PathBuf::from(r"Users\ramaz"),
            root.path().join("ramaz").join("..").join("other"),
        ],
        &[
            redirected.clone(),
            // Once each.
            profile("ramaz").join("AppData").join("Local"),
        ],
    );
    assert_eq!(
        folders,
        [
            folder("ramaz"),
            folder("other"),
            folder("kept"),
            folder("missing"),
            redirected.join(PASSWORD_FOLDER),
        ]
    );
    let cleanup = delete_passwords(&folders, wipe, remove_empty);
    assert!(cleanup.failed.is_empty(), "{:?}", cleanup.failed);
    assert_eq!(
        cleanup.deleted,
        [
            folder("ramaz").join(PASSWORD_FILES[0]),
            folder("ramaz").join(PASSWORD_FILES[1]),
            folder("other").join(PASSWORD_FILES[0]),
            redirected.join(PASSWORD_FOLDER).join(PASSWORD_FILES[0]),
        ]
    );
    // Empty now: gone. Holding something else, or never holding a password: kept.
    assert!(!folder("ramaz").exists());
    assert!(!redirected.join(PASSWORD_FOLDER).exists());
    assert!(folder("other").join("settings.json").is_file());
    assert!(folder("kept").is_dir());
    // Nothing left: nothing to do, and nothing to say.
    assert_eq!(
        delete_passwords(&folders, wipe, remove_empty),
        PasswordCleanup::default()
    );
}

#[test]
fn a_password_that_cannot_be_deleted_is_named_and_the_others_still_go() {
    let root = tempfile::tempdir().unwrap();
    let folders = password_folders(&[root.path().join("a"), root.path().join("b")], &[]);
    for folder in &folders {
        std::fs::create_dir_all(folder).unwrap();
        for name in PASSWORD_FILES {
            std::fs::write(folder.join(name), b"blob").unwrap();
        }
    }
    let stuck = folders[0].join(PASSWORD_FILES[0]);
    let cleanup = delete_passwords(
        &folders,
        |path| {
            if path == stuck {
                Err(std::io::Error::other("the file is in use"))
            } else {
                wipe(path)
            }
        },
        remove_empty,
    );
    assert_eq!(cleanup.failed.len(), 1, "{:?}", cleanup.failed);
    let failed = &cleanup.failed[0];
    assert!(
        failed.contains(&stuck.display().to_string())
            && failed.contains("the file is in use")
            && failed.contains("delete it by hand"),
        "{failed}"
    );
    assert_eq!(cleanup.deleted.len(), 3);
    assert!(stuck.is_file() && folders[0].is_dir());
    assert!(!folders[1].exists());
}
