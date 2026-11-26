//! Quick Suspend's refusals and order, over a fake process table.
use super::*;

const MINUTE: u64 = 60 * 10_000_000;
const HOST: u32 = 900;

fn process(pid: u32, parent: u32, started: u64, name: &str) -> Process {
    Process {
        pid,
        parent,
        started,
        name: name.into(),
    }
}

/// A PC with Steam running a game that started a crash handler and a
/// renderer, and the host and system processes beside them.
fn table() -> Vec<Process> {
    vec![
        process(4, 0, 1, "System"),
        process(600, 4, 2, "csrss.exe"),
        process(HOST, 1, 3, "butterpollo.exe"),
        process(1000, 1, 10 * MINUTE, "explorer.exe"),
        process(2000, 1000, 20 * MINUTE, "steam.exe"),
        process(2100, 2000, 21 * MINUTE, "steamwebhelper.exe"),
        process(3000, 2000, 30 * MINUTE, "game.exe"),
        process(3100, 3000, 31 * MINUTE, "crash_handler.exe"),
        process(3200, 3000, 32 * MINUTE, "renderer.exe"),
        process(3300, 3200, 33 * MINUTE, "shader_worker.exe"),
    ]
}

fn owned(pairs: &[(u32, u64)]) -> BTreeMap<u32, u64> {
    pairs.iter().copied().collect()
}

fn pids(plan: &Plan) -> Vec<u32> {
    plan.targets.iter().map(|t| t.pid).collect()
}

#[test]
fn the_game_tree_is_suspended_children_first_and_nothing_else() {
    let plan = plan(&owned(&[(3000, 30 * MINUTE)]), &table(), HOST).unwrap();
    // The deepest first; Steam, its helper, explorer, the host stay out.
    assert_eq!(pids(&plan), [3300, 3100, 3200, 3000]);
    assert!(plan.left_running.is_empty());
}

#[test]
fn a_reused_id_is_neither_a_root_nor_a_child() {
    let mut all = table();
    // The tracked root's id now belongs to a process created later.
    assert_eq!(
        plan(&owned(&[(3000, 29 * MINUTE)]), &all, HOST),
        Err(Refused {
            reason: Reason::NoOwnedProcess,
            program: None
        })
    );
    // A child older than its "parent" is another process under a reused id.
    all.push(process(3400, 3000, 5 * MINUTE, "unrelated.exe"));
    let plan = plan(&owned(&[(3000, 30 * MINUTE)]), &all, HOST).unwrap();
    assert!(!pids(&plan).contains(&3400));
}

#[test]
fn an_anti_cheat_in_the_tree_or_anywhere_on_the_pc_refuses_everything() {
    let mut all = table();
    all.push(process(3500, 3000, 34 * MINUTE, "EasyAntiCheat_EOS.exe"));
    let refused = plan(&owned(&[(3000, 30 * MINUTE)]), &all, HOST).unwrap_err();
    assert_eq!(refused.reason, Reason::AntiCheatDetected);
    assert_eq!(refused.program.as_deref(), Some("Easy Anti-Cheat"));
    // A service in another session that started with the game.
    let mut all = table();
    all.push(process(7000, 650, 29 * MINUTE, "BEService.exe"));
    assert_eq!(
        plan(&owned(&[(3000, 30 * MINUTE)]), &all, HOST)
            .unwrap_err()
            .reason,
        Reason::AntiCheatDetected
    );
    // One that has run since the PC started, or whose start is unknown, may
    // watch this game too.
    for started in [2, 0] {
        let mut all = table();
        all.push(process(7000, 650, started, "vgc.exe"));
        let refused = plan(&owned(&[(3000, 30 * MINUTE)]), &all, HOST).unwrap_err();
        assert_eq!(
            (refused.reason, refused.program.as_deref()),
            (Reason::AntiCheatDetected, Some("Riot Vanguard")),
            "{started}"
        );
    }
    for name in [
        "EAAntiCheat.GameService.exe",
        "vgc.exe",
        "BEDaisy",
        "FACEIT.exe",
        "PnkBstrA.exe",
        "mhyprot2.sys",
        "GameMon.des",
    ] {
        assert!(anti_cheat(name).is_some(), "{name}");
    }
    assert!(anti_cheat("hollow_knight.exe").is_none());
}

#[test]
fn system_processes_and_the_host_are_never_suspended() {
    for (pid, name) in [
        (3600, "dwm.exe"),
        (3600, "CSRSS.EXE"),
        (HOST, "game.exe"),
        (4, "x"),
    ] {
        let mut all = table();
        all.retain(|p| p.pid != pid);
        all.push(process(pid, 3000, 35 * MINUTE, name));
        let refused = plan(&owned(&[(3000, 30 * MINUTE)]), &all, HOST).unwrap_err();
        assert_eq!(refused.reason, Reason::DenylistedProcess, "{name}");
    }
    // The tracked root itself being the host.
    let refused = plan(&owned(&[(HOST, 3)]), &table(), HOST).unwrap_err();
    assert_eq!(refused.reason, Reason::DenylistedProcess);
}

#[test]
fn launchers_and_console_hosts_keep_running() {
    let mut all = table();
    all.push(process(3700, 3000, 36 * MINUTE, "conhost.exe"));
    all.push(process(2200, 2000, 22 * MINUTE, "UnrealCEFSubProcess.exe"));
    let plan = plan(&owned(&[(2000, 20 * MINUTE)]), &all, HOST).unwrap();
    // Steam is the tracked root here: it and its helper stay, the game goes.
    assert!(!pids(&plan).contains(&2000));
    assert!(!pids(&plan).contains(&2100));
    assert!(!pids(&plan).contains(&3700));
    assert!(!pids(&plan).contains(&2200));
    assert!(pids(&plan).contains(&3000));
    assert_eq!(plan.left_running.len(), 4);
    // Only a launcher runs: nothing to pause.
    let refused = super::plan(&owned(&[(2100, 21 * MINUTE)]), &all, HOST).unwrap_err();
    assert_eq!(refused.reason, Reason::NoOwnedProcess);
    assert_eq!(refused.program.as_deref(), Some("steamwebhelper.exe"));
    assert!(message(&refused).contains("steamwebhelper.exe"));
}

#[test]
fn every_refusal_reads_as_plain_english() {
    for value in 1..=11 {
        let refused = Refused {
            reason: Reason::from_u8(value),
            program: Some("game.exe".into()),
        };
        let text = message(&refused);
        assert!(text.ends_with('.'), "{text}");
        assert!(!text.contains("Experimental"));
    }
    assert_eq!(
        message(&Refused {
            reason: Reason::NoOwnedProcess,
            program: None
        }),
        "Quick Suspend works only for games Rubylight launched, not Desktop sessions."
    );
}
