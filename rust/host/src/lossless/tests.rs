use super::*;
fn sample(pid: u32, path: &str, cpu_time: u64, memory_mb: u64) -> Sample {
    Sample {
        process: butterpollo_core::steam::Process {
            pid,
            parent: 0,
            started: 1,
            name: path.rsplit(['/', '\\']).next().unwrap().into(),
        },
        path: path.into(),
        cpu_time,
        working_set: memory_mb * 1024 * 1024,
        windowed: true,
    }
}
fn observed(samples: Vec<Sample>) -> BTreeMap<(u32, u64), Candidate> {
    let mut candidates = BTreeMap::new();
    let first = samples
        .iter()
        .cloned()
        .map(|mut sample| {
            sample.cpu_time = 1;
            sample
        })
        .collect();
    observe(&mut candidates, first, Duration::ZERO);
    observe(&mut candidates, samples, OBSERVATION);
    candidates
}
#[test]
fn scoring_switches_from_equal_cpu_memory_weights_at_eight_percent() {
    let mut processes = vec![
        sample(1, r"C:\Games\launcher.exe", 16_000_001, 100),
        sample(2, r"C:\Games\game.exe", 31_600_001, 10),
    ];
    let candidates = observed(processes.clone());
    assert_eq!(
        select_game(&candidates, 0, None, None, Some(r"C:\Windows"), 4)
            .unwrap()
            .process
            .pid,
        1
    );
    processes[1].cpu_time = 32_000_001;
    let candidates = observed(processes);
    assert_eq!(
        select_game(&candidates, 0, None, None, Some(r"C:\Windows"), 4)
            .unwrap()
            .process
            .pid,
        2
    );
}
#[test]
fn scoring_uses_peak_memory_and_each_processes_observed_lifetime() {
    let mut candidates = BTreeMap::new();
    observe(
        &mut candidates,
        vec![sample(1, r"C:\game.exe", 1, 1000)],
        Duration::ZERO,
    );
    observe(
        &mut candidates,
        vec![
            sample(1, r"C:\game.exe", 1, 1),
            sample(2, r"C:\launcher.exe", 1, 100),
        ],
        Duration::from_secs(8),
    );
    observe(
        &mut candidates,
        vec![
            sample(1, r"C:\game.exe", 1, 1),
            sample(2, r"C:\launcher.exe", 1, 100),
        ],
        OBSERVATION,
    );
    assert_eq!(
        select_game(&candidates, 0, None, None, None, 1)
            .unwrap()
            .process
            .pid,
        1
    );
    // The new game consumes less total CPU, but twice as much CPU per second observed.
    observe(
        &mut candidates,
        vec![
            sample(1, r"C:\game.exe", 10_000_001, 1),
            sample(2, r"C:\launcher.exe", 4_000_001, 100),
        ],
        OBSERVATION,
    );
    assert_eq!(
        select_game(&candidates, 0, None, None, None, 1)
            .unwrap()
            .process
            .pid,
        2
    );
}
#[test]
fn folder_root_and_executable_bonuses_match_vibepollo() {
    let candidates = observed(vec![
        sample(1, r"C:\Games\Game2\launcher.exe", 10_000_001, 100),
        sample(2, r"C:\Games\Game\game.exe", 10_000_001, 100),
    ]);
    assert_eq!(
        select_game(&candidates, 1, None, None, None, 1)
            .unwrap()
            .process
            .pid,
        2
    );
    assert_eq!(
        select_game(&candidates, 0, Some("c:/GAMES/game/"), None, None, 1)
            .unwrap()
            .process
            .pid,
        2
    );
    assert_eq!(
        select_game(&candidates, 1, Some(r"C:\Games"), None, None, 1)
            .unwrap()
            .process
            .pid,
        1
    );
    assert_eq!(
        select_game(
            &candidates,
            1,
            Some(r"C:\Games"),
            Some("c:/games/game/GAME.exe"),
            None,
            1
        )
        .unwrap()
        .process
        .pid,
        2
    );
    assert!(!in_folder(
        r"C:\Games\Game2\game.exe",
        Some(r"C:\Games\Game")
    ));
}
#[test]
fn windows_processes_and_small_idle_processes_are_penalized() {
    let candidates = observed(vec![
        sample(1, r"C:\Windows\helper.exe", 1_000_001, 40),
        sample(2, r"C:\Game\game.exe", 1_000_001, 40),
    ]);
    // The system penalty outweighs the executable bonus for a small idle process.
    assert_eq!(
        select_game(
            &candidates,
            1,
            None,
            Some(r"C:\Windows\helper.exe"),
            Some("c:/windows/"),
            1
        )
        .unwrap()
        .process
        .pid,
        2
    );
    let candidates = observed(vec![
        sample(1, r"C:\Game\small.exe", 1_000_001, 31),
        sample(2, r"C:\Game\game.exe", 1_000_001, 32),
    ]);
    assert_eq!(
        select_game(&candidates, 0, Some(r"C:\Game\small.exe"), None, None, 1)
            .unwrap()
            .process
            .pid,
        1
    );
    assert_eq!(
        select_game(&candidates, 0, None, None, None, 1)
            .unwrap()
            .process
            .pid,
        2
    );
}
#[test]
fn invalid_or_single_samples_are_not_scored() {
    let mut candidates = observed(vec![
        sample(1, "", 100, 100),
        sample(2, r"C:\zero.exe", 0, 100),
    ]);
    let backwards = candidates.get_mut(&(2, 1)).unwrap();
    backwards.start_cpu = 100;
    assert!(select_game(&candidates, 0, None, None, None, 1).is_none());
    let mut candidates = BTreeMap::new();
    observe(
        &mut candidates,
        vec![sample(1, r"C:\game.exe", 100, 100)],
        Duration::ZERO,
    );
    assert!(select_game(&candidates, 0, None, None, None, 1).is_none());
}
#[test]
fn bootstrap_excludes_the_baseline_and_focus_requires_matching_game_windows() {
    let game = sample(1, r"C:\Game\game.exe", 1, 100);
    let baseline = HashSet::from([game.identity()]);
    assert!(!eligible(&game, &baseline, None, None));
    let mut restarted = game.clone();
    restarted.process.started += 1;
    assert!(eligible(&restarted, &baseline, None, None));
    assert!(eligible(&game, &baseline, Some(&game), None));
    for name in [
        "LosslessScaling.exe",
        "Lossless Scaling.exe",
        "Playnite.DesktopApp.exe",
        "Playnite.FullscreenApp.exe",
    ] {
        assert!(!eligible(
            &sample(2, &format!(r"C:\Game\{name}"), 1, 100),
            &baseline,
            Some(&game),
            Some(r"C:\Game")
        ));
    }
    assert!(!eligible(
        &sample(2, r"C:\Game2\unrelated.exe", 1, 100),
        &baseline,
        Some(&game),
        Some(r"C:\Game")
    ));
    restarted.windowed = false;
    assert!(!eligible(
        &restarted,
        &baseline,
        Some(&game),
        Some(r"C:\Game")
    ));
}
#[test]
fn retargets_a_scored_launcher_replacement_and_restarts_but_not_an_unchanged_game() {
    let launcher = sample(1, r"C:\Game\launcher.exe", 1_000_001, 10);
    let game = sample(2, r"C:\Game\game.exe", 80_000_001, 1000);
    let mut candidates = observed(vec![launcher.clone(), game.clone()]);
    let selected = select_game(
        &candidates,
        1,
        Some(r"C:\Game"),
        Some(&launcher.path),
        None,
        1,
    );
    assert_eq!(
        retarget(Some(&launcher), selected).unwrap().identity(),
        game.identity()
    );
    assert!(retarget(Some(&game), selected).is_none());
    assert_eq!(
        retarget(None, selected).unwrap().identity(),
        game.identity()
    );
    observe(&mut candidates, vec![], OBSERVATION + POLL);
    assert!(
        retarget(
            Some(&game),
            select_game(&candidates, 0, None, None, None, 1)
        )
        .is_none()
    );
    let mut restarted = game.clone();
    restarted.process.started += 1;
    candidates = observed(vec![restarted.clone()]);
    let selected = select_game(&candidates, 2, Some(r"C:\Game"), Some(&game.path), None, 1);
    assert_eq!(
        retarget(Some(&game), selected).unwrap().identity(),
        restarted.identity()
    );
}
#[test]
fn dead_launchers_and_reused_pids_do_not_keep_their_old_scores() {
    let mut candidates = observed(vec![sample(1, r"C:\Game\launcher.exe", 100_000_001, 1000)]);
    let mut game = sample(1, r"C:\Game\game.exe", 1, 10);
    game.process.started = 2;
    observe(&mut candidates, vec![game.clone()], OBSERVATION + POLL);
    assert_eq!(candidates.len(), 1);
    assert!(select_game(&candidates, 0, None, None, None, 1).is_none());
    game.cpu_time += 1_000_000;
    observe(&mut candidates, vec![game.clone()], OBSERVATION + POLL * 2);
    assert_eq!(
        select_game(&candidates, 0, None, None, None, 1)
            .unwrap()
            .identity(),
        game.identity()
    );
}
const SETTINGS: &str = "\u{feff}<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<Settings xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\n  <Hotkey>S</Hotkey>\n  <HotkeyModifierKeys>Alt Control</HotkeyModifierKeys>\n  <GameProfiles>\n    <Profile>\n      <Title>Default</Title>\n      <ScalingType>Off</ScalingType>\n      <CaptureApi>DXGI</CaptureApi>\n    </Profile>\n    <Profile>\n      <Title>Elden Ring</Title>\n      <Path>eldenring.exe</Path>\n    </Profile>\n    <Profile>\n      <Title>Vibeshine</Title>\n      <Path>old.exe</Path>\n    </Profile>\n  </GameProfiles>\n</Settings>";
#[test]
fn the_profile_is_added_from_the_default_and_removed_again() {
    let options = butterpollo_core::lossless::options(
        &serde_json::json!({"frame-generation-mode": "lossless-scaling", "lossless-scaling-profile": "recommended"}),
        &butterpollo_core::config::Config::default(),
        120.,
    )
    .unwrap();
    let updated = with_profile(SETTINGS, &options.profile, "game.exe;launcher.exe").unwrap();
    let root = parse(&updated).unwrap();
    let list = root.get_child("GameProfiles").unwrap();
    let titles: Vec<String> = list
        .children
        .iter()
        .filter_map(XMLNode::as_element)
        .map(|p| text_of(p, "Title"))
        .collect();
    assert_eq!(titles, ["Default", "Elden Ring", "Vibeshine"]);
    let ours = list
        .children
        .iter()
        .rev()
        .find_map(XMLNode::as_element)
        .unwrap();
    assert_eq!(text_of(ours, "Path"), "game.exe;launcher.exe");
    assert_eq!(text_of(ours, "CaptureApi"), "WGC");
    assert_eq!(text_of(ours, "FrameGeneration"), "LSFG3");
    assert_eq!(text_of(ours, "LSFG3Target"), "120");
    assert_eq!(text_of(ours, "AutoScale"), "false");
    assert_eq!(text_of(ours, "ScalingType"), "Off");
    assert!(updated.contains("xmlns:xsi"));
    assert_eq!(hotkey(&updated), Some((vec![0x11, 0x12], u16::from(b'S'))));
    let restored = without_profile(&updated).unwrap().unwrap();
    assert!(!restored.contains("Vibeshine") && restored.contains("Elden Ring"));
    assert_eq!(without_profile(&restored).unwrap(), None);
}
