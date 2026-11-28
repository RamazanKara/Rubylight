use super::*;
use windows::{Win32::Foundation::*, core::HRESULT};

fn launch_error(code: WIN32_ERROR) -> anyhow::Error {
    windows::core::Error::from_hresult(HRESULT::from_win32(code.0)).into()
}

#[test]
fn service_helper_runs_as_system_in_the_host_session() {
    assert!(matches!(
        helper_target(true, || Ok(3)).unwrap(),
        Target::SystemSession(3)
    ));
    assert!(matches!(
        helper_target(false, || unreachable!()).unwrap(),
        Target::User { elevated: false }
    ));
    assert!(helper_target(true, || bail!("no session")).is_err());
}

#[test]
fn service_retries_elevation_required_with_the_user_admin_token() {
    let mut attempts = Vec::new();
    let result = start_with_elevation(true, |elevated| {
        attempts.push(elevated);
        if elevated {
            Ok(42)
        } else {
            Err(launch_error(ERROR_ELEVATION_REQUIRED).context("CreateProcessAsUserW"))
        }
    });
    assert_eq!(result.unwrap(), 42);
    assert_eq!(attempts, [false, true]);
}

#[test]
fn ordinary_launches_and_unrelated_errors_never_request_elevation() {
    for service in [false, true] {
        let mut attempts = Vec::new();
        start_with_elevation(service, |elevated| {
            attempts.push(elevated);
            Ok(())
        })
        .unwrap();
        assert_eq!(attempts, [false]);
        for code in [
            ERROR_ACCESS_DENIED,
            ERROR_FILE_NOT_FOUND,
            ERROR_PRIVILEGE_NOT_HELD,
        ] {
            attempts.clear();
            let error = start_with_elevation::<()>(service, |elevated| {
                attempts.push(elevated);
                Err(launch_error(code))
            })
            .unwrap_err();
            assert_eq!(attempts, [false]);
            assert_eq!(
                error.downcast_ref::<windows::core::Error>().unwrap().code(),
                HRESULT::from_win32(code.0)
            );
        }
    }
}

#[test]
fn portable_elevation_required_has_actionable_guidance_without_retrying() {
    let mut attempts = Vec::new();
    let error = start_with_elevation::<()>(false, |elevated| {
        attempts.push(elevated);
        Err(launch_error(ERROR_ELEVATION_REQUIRED))
    })
    .unwrap_err();
    assert_eq!(attempts, [false]);
    assert!(
        error
            .to_string()
            .contains("Start RTSS manually as administrator")
    );
    assert_eq!(
        error.downcast_ref::<windows::core::Error>().unwrap().code(),
        HRESULT::from_win32(ERROR_ELEVATION_REQUIRED.0)
    );
}

#[test]
fn failed_elevated_retry_preserves_the_error_and_does_not_retry_again() {
    let mut attempts = Vec::new();
    let error = start_with_elevation::<()>(true, |elevated| {
        attempts.push(elevated);
        Err(launch_error(if elevated {
            ERROR_ACCESS_DENIED
        } else {
            ERROR_ELEVATION_REQUIRED
        }))
    })
    .unwrap_err();
    assert_eq!(attempts, [false, true]);
    assert!(error.to_string().contains("signed-in administrator"));
    assert_eq!(
        error.downcast_ref::<windows::core::Error>().unwrap().code(),
        HRESULT::from_win32(ERROR_ACCESS_DENIED.0)
    );
}

#[test]
fn profile_updates_preserve_other_sections_and_remove_absent_originals() {
    let original = "; user\r\n[Hooking]\r\nEnable=1\r\n[Framerate]\r\nLimit=60\r\nCustom=keep\r\n[OSD]\r\nColor=red\r\n";
    let before = properties(original).unwrap();
    let desired = BTreeMap::from([
        ("Limit".into(), Some(2997)),
        ("LimitDenominator".into(), Some(50)),
        ("SyncLimiter".into(), Some(3)),
    ]);
    let applied = replace(original, &desired).unwrap();
    assert_eq!(properties(&applied).unwrap(), desired);
    assert_eq!(replace(&applied, &before).unwrap(), original);
    assert!(properties("[Framerate]\nLimit=1\nlimit=2").is_err());
    assert!(applied.contains("Custom=keep\r\n"));
    assert!(applied.contains("[OSD]\r\nColor=red\r\n"));
}
#[test]
fn profile_restore_survives_rtss_ui_file_lock_and_preserves_user_fields() -> Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
    let directory = tempfile::tempdir()?;
    std::fs::create_dir(directory.path().join("Profiles"))?;
    let path = directory.path().join("Profiles/Global");
    let original = "[Framerate]\r\nLimit=60\r\nCustom=keep\r\n[OSD]\r\nColor=red\r\n";
    std::fs::write(&path, original)?;
    let _lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0)
        .open(&path)?;
    let values = BTreeMap::from([
        ("Limit".into(), Some(60000)),
        ("LimitDenominator".into(), Some(1001)),
        ("SyncLimiter".into(), Some(2)),
    ]);
    write_profile(directory.path(), &values)?;
    assert_eq!(properties(&read(directory.path())?)?, values);
    write_profile(directory.path(), &properties(original)?)?;
    assert_eq!(read(directory.path())?, original);
    Ok(())
}
#[test]
fn unchanged_profile_restores_without_write_or_delete_access() -> Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::FILE_SHARE_READ;
    let directory = tempfile::tempdir()?;
    std::fs::create_dir(directory.path().join("Profiles"))?;
    let path = directory.path().join("Profiles/Global");
    let original = "[Framerate]\nLimit=120\nLimitDenominator=1\nSyncLimiter=1\nCustom=keep\n";
    std::fs::write(&path, original)?;
    let _lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .open(&path)?;
    write_profile(directory.path(), &properties(original)?)?;
    assert!(
        write_profile(
            directory.path(),
            &BTreeMap::from([("Limit".into(), Some(60))])
        )
        .is_err()
    );
    assert_eq!(read(directory.path())?, original);
    Ok(())
}

#[test]
fn profile_sdk_starts_at_rtss_737_including_its_mislabelled_build() {
    for (version, sdk) in [
        ([7, 3, 5, 28314], true),
        ([7, 3, 7, 0], true),
        ([7, 3, 8, 1], true),
        ([7, 4, 0, 0], true),
        ([8, 0, 0, 0], true),
        ([7, 3, 5, 26975], false),
        ([7, 3, 6, 27707], false),
        ([7, 3, 4, 0], false),
        ([7, 2, 9, 0], false),
    ] {
        assert_eq!(profile_sdk(Some(version)), sdk, "{version:?}");
    }
    assert!(!profile_sdk(None));
}

/// RTSS 7.3.7 as Vibepollo found it: SaveProfile reaches the profile RTSS
/// has open, UpdateProfiles applies that one, and a file edit does not.
#[derive(Default)]
struct Rtss737 {
    disk: std::cell::RefCell<BTreeMap<&'static CStr, u32>>,
    buffer: std::cell::RefCell<BTreeMap<&'static CStr, u32>>,
    open: std::cell::RefCell<BTreeMap<&'static CStr, u32>>,
    live: std::cell::RefCell<BTreeMap<&'static CStr, u32>>,
    flags: std::cell::Cell<u32>,
    calls: std::cell::RefCell<Vec<&'static str>>,
}
impl Rtss737 {
    fn new(limit: u32, flags: u32) -> Self {
        let profile = BTreeMap::from([
            (PROPERTIES[0], limit),
            (PROPERTIES[1], 1),
            (PROPERTIES[2], 0),
        ]);
        let rtss = Self::default();
        *rtss.disk.borrow_mut() = profile.clone();
        *rtss.open.borrow_mut() = profile.clone();
        *rtss.live.borrow_mut() = profile;
        rtss.flags.set(flags);
        rtss
    }
    fn live_limit(&self) -> u32 {
        self.live.borrow()[PROPERTIES[0]]
    }
}
impl Sdk for Rtss737 {
    fn load(&self) {
        self.calls.borrow_mut().push("load");
        *self.buffer.borrow_mut() = self.disk.borrow().clone();
    }
    fn set(&self, property: &CStr, value: u32) -> Result<bool> {
        self.calls.borrow_mut().push("set");
        let property = PROPERTIES.into_iter().find(|p| *p == property).unwrap();
        self.buffer.borrow_mut().insert(property, value);
        Ok(true)
    }
    fn save(&self) -> Result<()> {
        self.calls.borrow_mut().push("save");
        *self.disk.borrow_mut() = self.buffer.borrow().clone();
        *self.open.borrow_mut() = self.buffer.borrow().clone();
        Ok(())
    }
    fn update(&self) {
        self.calls.borrow_mut().push("update");
        *self.live.borrow_mut() = self.open.borrow().clone();
    }
    fn get(&self, property: &CStr) -> Option<u32> {
        self.buffer.borrow().get(property).copied()
    }
    fn flags(&self) -> u32 {
        self.flags.get()
    }
    fn set_flags(&self, and: u32, xor: u32) {
        self.calls.borrow_mut().push("flags");
        self.flags.set((self.flags.get() & and) ^ xor);
    }
}
fn request(set: &[(&str, u32)], disabled: Option<bool>) -> Request {
    Request {
        root: PathBuf::new(),
        set: set.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        disabled,
        reload: true,
    }
}

#[test]
fn rtss_737_takes_the_stream_limit_only_through_its_profile_sdk() -> Result<()> {
    // The user's own 158 FPS limit, with the limiter switched off.
    let rtss = Rtss737::new(158, 4);
    // The file edit and reload the host made before this fix.
    rtss.disk.borrow_mut().insert(PROPERTIES[0], 100);
    let reply = run(&request(&[], Some(false)), &rtss)?;
    assert_eq!(reply.values["Limit"], Some(100));
    assert_eq!(rtss.live_limit(), 158, "the file alone never reaches RTSS");

    let rtss = Rtss737::new(158, 4);
    let applied = [("Limit", 100), ("LimitDenominator", 1), ("SyncLimiter", 1)];
    let reply = run(&request(&applied, Some(false)), &rtss)?;
    assert_eq!(rtss.live_limit(), 100);
    assert_eq!(rtss.live.borrow()[PROPERTIES[2]], 1);
    assert_eq!(reply.flags & 4, 0);
    assert_eq!(
        reply.values,
        applied
            .iter()
            .map(|(k, v)| (k.to_string(), Some(*v)))
            .collect::<BTreeMap<_, _>>()
    );
    // Vibepollo's order: values, save, UpdateProfiles, the flag, reload.
    assert_eq!(
        *rtss.calls.borrow(),
        [
            "load", "set", "set", "set", "save", "update", "flags", "load", "update"
        ]
    );

    // Restoring the user's limit at stream end goes the same way.
    let reply = run(&request(&[("Limit", 158)], Some(true)), &rtss)?;
    assert_eq!(rtss.live_limit(), 158);
    assert_eq!(reply.flags & 4, 4);

    // A query changes nothing.
    let rtss = Rtss737::new(158, 0);
    let query = Request {
        reload: false,
        ..request(&[], None)
    };
    run(&query, &rtss)?;
    assert_eq!(*rtss.calls.borrow(), ["load"]);
    Ok(())
}

#[test]
fn the_helper_names_each_rtss_call_before_making_it() -> Result<()> {
    let steps = std::cell::RefCell::new(Vec::new());
    let report = |step: &str| steps.borrow_mut().push(step.to_string());
    let sdk = Reporting {
        sdk: Rtss737::new(158, 0),
        report: &report,
    };
    run(&request(&[("Limit", 100)], Some(false)), &sdk)?;
    assert_eq!(
        steps.borrow()[..6],
        [
            "LoadProfile",
            "SetProfileProperty FramerateLimit",
            "SaveProfile",
            "UpdateProfiles",
            "SetFlags",
            "LoadProfile",
        ]
    );
    // A timeout after this names the call that never returned.
    assert_eq!(
        steps.borrow().last().unwrap(),
        "GetProfileProperty SyncLimiter"
    );
    Ok(())
}

#[test]
fn rtss_rejecting_a_property_fails_before_saving() {
    struct Rejecting(Rtss737);
    impl Sdk for Rejecting {
        fn load(&self) {
            self.0.load()
        }
        fn set(&self, _: &CStr, _: u32) -> Result<bool> {
            Ok(false)
        }
        fn save(&self) -> Result<()> {
            self.0.save()
        }
        fn update(&self) {
            self.0.update()
        }
        fn get(&self, property: &CStr) -> Option<u32> {
            self.0.get(property)
        }
        fn flags(&self) -> u32 {
            self.0.flags()
        }
        fn set_flags(&self, and: u32, xor: u32) {
            self.0.set_flags(and, xor)
        }
    }
    let rtss = Rejecting(Rtss737::new(158, 0));
    let error = run(&request(&[("Limit", 100)], Some(false)), &rtss).unwrap_err();
    assert!(error.to_string().contains("rejected Limit=100"), "{error}");
    assert_eq!(*rtss.0.calls.borrow(), ["load"]);
    assert_eq!(rtss.0.live_limit(), 158);
}

fn profile_fixture(text: &str) -> Result<tempfile::TempDir> {
    let directory = tempfile::tempdir()?;
    std::fs::create_dir(directory.path().join("Profiles"))?;
    std::fs::write(directory.path().join("Profiles/Global"), text)?;
    Ok(directory)
}
fn reply(values: &BTreeMap<String, Option<u32>>) -> Reply {
    Reply {
        flags: 0,
        values: values.clone(),
    }
}

#[test]
fn apply_sends_values_through_the_sdk_only_for_rtss_737() -> Result<()> {
    let original = "[Framerate]\r\nLimit=158\r\nLimitDenominator=1\r\nCustom=keep\r\n";
    let applied = BTreeMap::from([
        ("Limit".into(), Some(100)),
        ("LimitDenominator".into(), Some(1)),
        ("SyncLimiter".into(), Some(1)),
    ]);
    for sdk in [false, true] {
        let directory = profile_fixture(original)?;
        let mut sent = None;
        apply_with(directory.path(), &applied, Some(false), sdk, |request| {
            sent = Some((request.set.clone(), request.disabled, request.reload));
            Ok(reply(&applied))
        })?;
        let expected = if sdk {
            BTreeMap::from([
                ("Limit".into(), 100),
                ("LimitDenominator".into(), 1),
                ("SyncLimiter".into(), 1),
            ])
        } else {
            BTreeMap::new()
        };
        assert_eq!(sent, Some((expected, Some(false), true)), "sdk {sdk}");
        // The service's own write keeps the file right even if RTSS stalls.
        assert_eq!(properties(&read(directory.path())?)?, applied);
        assert!(read(directory.path())?.contains("Custom=keep\r\n"));
    }

    // Restoring a key the user's profile never had: the SDK cannot delete
    // it, so RTSS gets its default and may save that.
    let directory = profile_fixture("[Framerate]\nLimit=100\nLimitDenominator=1\nSyncLimiter=1\n")?;
    let restore = BTreeMap::from([("Limit".into(), Some(158)), ("SyncLimiter".into(), None)]);
    let mut sent = None;
    apply_with(directory.path(), &restore, Some(true), true, |request| {
        sent = Some(request.set.clone());
        write_profile(
            &request.root,
            &request
                .set
                .iter()
                .map(|(k, v)| (k.clone(), Some(*v)))
                .collect(),
        )?;
        Ok(reply(&restore))
    })?;
    assert_eq!(
        sent,
        Some(BTreeMap::from([
            ("Limit".into(), 158),
            ("SyncLimiter".into(), 0)
        ]))
    );
    Ok(())
}

#[test]
fn apply_fails_when_rtss_saves_a_different_limit() -> Result<()> {
    let directory = profile_fixture("[Framerate]\nLimit=158\nLimitDenominator=1\n")?;
    let applied = BTreeMap::from([
        ("Limit".into(), Some(100)),
        ("LimitDenominator".into(), Some(1)),
    ]);
    let error = apply_with(directory.path(), &applied, Some(false), true, |request| {
        // SaveProfile from a stale buffer.
        write_profile(
            &request.root,
            &BTreeMap::from([("Limit".into(), Some(158))]),
        )?;
        Ok(reply(&applied))
    })
    .unwrap_err();
    assert!(error.to_string().contains("different Limit"), "{error}");
    Ok(())
}

fn make_junction(link: &Path, target: &Path) {
    let out = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
}

#[test]
fn trust_verdict_accepts_only_administrators_and_rejects_links_and_non_admins() {
    use butterpollo_session::neural::{ADMINISTRATORS, Security};
    let path = Path::new(r"C:\Program Files (x86)\RivaTuner Statistics Server");
    for owner in ADMINISTRATORS {
        let sec = Security {
            link: false,
            owner: owner.into(),
            writers: ADMINISTRATORS.iter().map(|s| (*s).into()).collect(),
        };
        trust::check_security(path, &sec, TrustMode::AdministratorsOnly).unwrap();
    }
    // Non-admin owner is rejected.
    let bad_owner = Security {
        link: false,
        owner: "S-1-5-21-100-200-300-1001".into(),
        writers: vec!["S-1-5-18".into(), "S-1-5-32-544".into()],
    };
    let err = trust::check_security(path, &bad_owner, TrustMode::AdministratorsOnly).unwrap_err();
    assert!(
        err.to_string().contains("not by SYSTEM or Administrators"),
        "{err}"
    );
    // Non-admin writer (Users / Everyone / user SID / unknown ACE type) is rejected.
    for writer in ["S-1-5-32-545", "S-1-1-0", "S-1-5-11", "ACE type 9"] {
        let bad_writer = Security {
            link: false,
            owner: "S-1-5-32-544".into(),
            writers: vec!["S-1-5-18".into(), writer.into()],
        };
        let err =
            trust::check_security(path, &bad_writer, TrustMode::AdministratorsOnly).unwrap_err();
        assert!(
            err.to_string().contains("can be changed by"),
            "{writer}: {err}"
        );
    }
    // Reparse points are rejected in both modes.
    for mode in [TrustMode::AdministratorsOnly, TrustMode::ReparseOnly] {
        let linked = Security {
            link: true,
            owner: "S-1-5-32-544".into(),
            writers: vec!["S-1-5-32-544".into()],
        };
        let err = trust::check_security(path, &linked, mode).unwrap_err();
        assert!(err.to_string().contains("link"), "{mode:?}: {err}");
    }
}

#[test]
fn untrusted_user_writable_root_is_rejected_for_exec_hooks_and_admin_profile_write() -> Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("RTSS.exe"), b"MZ")?;
    std::fs::write(dir.path().join("RTSSHooks64.dll"), b"MZ")?;
    std::fs::create_dir(dir.path().join("Profiles"))?;
    std::fs::write(
        dir.path().join("Profiles/Global"),
        "[Framerate]\nLimit=60\n",
    )?;
    let err = start(dir.path()).err().unwrap();
    assert!(err.to_string().contains("untrusted RTSS path"), "{err:#}");
    let err = Hooks::open(dir.path()).err().unwrap();
    assert!(err.to_string().contains("untrusted RTSS path"), "{err:#}");
    let err = trust::write_profile_checked(
        dir.path(),
        &BTreeMap::from([("Limit".into(), Some(120))]),
        TrustMode::AdministratorsOnly,
    )
    .unwrap_err();
    assert!(err.to_string().contains("untrusted RTSS path"), "{err:#}");
    // Nothing was overwritten.
    assert_eq!(
        std::fs::read_to_string(dir.path().join("Profiles/Global"))?,
        "[Framerate]\nLimit=60\n"
    );
    Ok(())
}

#[test]
fn reparse_point_root_and_profiles_junctions_are_rejected_without_writing() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let real_root = tmp.path().join("real_root");
    let real_profiles = tmp.path().join("real_profiles");
    std::fs::create_dir_all(real_root.join("Profiles"))?;
    std::fs::create_dir_all(&real_profiles)?;
    let original = "[Framerate]\nLimit=60\n";
    std::fs::write(real_root.join("Profiles/Global"), original)?;
    std::fs::write(real_profiles.join("Global"), original)?;

    // 1. Root itself is a junction.
    let link_root = tmp.path().join("link_root");
    make_junction(&link_root, &real_root);
    let err = write_profile(&link_root, &BTreeMap::from([("Limit".into(), Some(90))])).unwrap_err();
    assert!(err.to_string().contains("link"), "{err:#}");
    assert!(read(&link_root).is_err());
    assert_eq!(
        std::fs::read_to_string(real_root.join("Profiles/Global"))?,
        original
    );

    // 2. Profiles directory under root is a junction.
    let host_root = tmp.path().join("host_root");
    std::fs::create_dir_all(&host_root)?;
    let link_profiles = host_root.join("Profiles");
    make_junction(&link_profiles, &real_profiles);
    let err = write_profile(&host_root, &BTreeMap::from([("Limit".into(), Some(90))])).unwrap_err();
    assert!(err.to_string().contains("link"), "{err:#}");
    assert!(read(&host_root).is_err());
    assert_eq!(
        std::fs::read_to_string(real_profiles.join("Global"))?,
        original
    );
    Ok(())
}

#[test]
fn trusted_windows_program_files_and_system_dll_pass_pinned_validation() -> Result<()> {
    // Standard RTSS parent directories C:\ and C:\Program Files (x86) are
    // TrustedInstaller/Administrators-only and must pass validation.
    for path in [Path::new(r"C:\"), Path::new(r"C:\Program Files (x86)")] {
        if path.is_dir() {
            trust::pin_root(path, TrustMode::AdministratorsOnly)?;
        }
    }
    // LoadLibraryExW with LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32
    // succeeds while the validated directory chain and DLL handle are pinned open.
    let system32 =
        PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| r"C:\Windows".into()))
            .join("System32");
    if system32.join("version.dll").is_file() {
        let pinned = pin_binary(
            &system32,
            &["version.dll"],
            "missing version.dll",
            TrustMode::AdministratorsOnly,
        )?;
        let _lib = load_library_pinned(&pinned)?;
    }
    Ok(())
}

#[test]
fn unc_and_device_paths_are_rejected_before_opening_handles() {
    for remote in [
        Path::new(r"\\127.0.0.1\share\rtss"),
        Path::new(r"\\?\UNC\127.0.0.1\share\rtss"),
        Path::new(r"\\.\pipe\rtss"),
    ] {
        for mode in [TrustMode::AdministratorsOnly, TrustMode::ReparseOnly] {
            let err = trust::pin_root(remote, mode).err().unwrap();
            assert!(
                err.to_string().contains("must be on a local drive"),
                "{remote:?} ({mode:?}): {err:#}"
            );
        }
        assert!(
            find_plain(remote, &EXECUTABLES).is_none(),
            "{remote:?} must not probe non-local paths"
        );
        assert!(executable(remote).is_none(), "{remote:?}");
        assert!(!available(remote), "{remote:?}");
    }
}

#[test]
fn trust_check_accepts_program_files_and_validates_second_drive_roots() {
    use butterpollo_session::neural::{ADMINISTRATORS, Security};
    let admin = || Security {
        link: false,
        owner: ADMINISTRATORS[1].into(),
        writers: vec![ADMINISTRATORS[0].into(), ADMINISTRATORS[1].into()],
    };
    let user_writable = Security {
        link: false,
        owner: "S-1-5-21-100-200-300-1001".into(),
        writers: vec![
            ADMINISTRATORS[0].into(),
            ADMINISTRATORS[1].into(),
            "S-1-5-11".into(),
        ],
    };

    // System drive Program Files chain: every ancestor and RTSS root passes Admin-only check.
    let c_drive = Path::new(r"C:\");
    let c_pf = Path::new(r"C:\Program Files (x86)");
    let c_rtss = Path::new(r"C:\Program Files (x86)\RivaTuner Statistics Server");
    let (a, b, c) = (admin(), admin(), admin());
    trust::check_chain(
        c_rtss,
        &[(c_drive, &a), (c_pf, &b), (c_rtss, &c)],
        Some(b'C'),
        TrustMode::AdministratorsOnly,
    )
    .unwrap();

    // User-writable ancestor on system drive (outside Program Files) is still rejected.
    let err = trust::check_chain(
        Path::new(r"C:\Games\RTSS"),
        &[
            (c_drive, &a),
            (Path::new(r"C:\Games"), &user_writable),
            (Path::new(r"C:\Games\RTSS"), &c),
        ],
        Some(b'C'),
        TrustMode::AdministratorsOnly,
    )
    .unwrap_err();
    assert!(err.to_string().contains(r"C:\Games"), "{err:#}");

    // Second drive: D:\ and D:\Apps may have user/Authenticated Users permissions, so an
    // Admin-only RTSS folder and binaries on D:\ pass once links are ruled out.
    let d_drive = Path::new(r"D:\");
    let d_apps = Path::new(r"D:\Apps");
    let d_rtss = Path::new(r"D:\Apps\RivaTuner Statistics Server");
    trust::check_chain(
        d_rtss,
        &[
            (d_drive, &user_writable),
            (d_apps, &user_writable),
            (d_rtss, &c),
        ],
        Some(b'C'),
        TrustMode::AdministratorsOnly,
    )
    .unwrap();

    // A reparse-point ancestor or user-writable RTSS root on the second drive is still rejected
    // with a message naming the folder and offending account.
    let link = Security {
        link: true,
        ..admin()
    };
    let err = trust::check_chain(
        d_rtss,
        &[(d_drive, &user_writable), (d_apps, &link), (d_rtss, &c)],
        Some(b'C'),
        TrustMode::AdministratorsOnly,
    )
    .unwrap_err();
    assert!(err.to_string().contains("link"), "{err:#}");
    let err = trust::check_chain(
        d_rtss,
        &[
            (d_drive, &user_writable),
            (d_apps, &user_writable),
            (d_rtss, &user_writable),
        ],
        Some(b'C'),
        TrustMode::AdministratorsOnly,
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains(r"D:\Apps\RivaTuner Statistics Server"),
        "{msg}"
    );
    assert!(msg.contains("Program Files"), "{msg}");
}

#[test]
fn empty_property_values_do_not_break_profile_parsing() {
    let parsed = properties("[Framerate]\nLimit=\nLimitDenominator=\nSyncLimiter=2\n").unwrap();
    assert_eq!(parsed["Limit"], None);
    assert_eq!(parsed["LimitDenominator"], None);
    assert_eq!(parsed["SyncLimiter"], Some(2));
}

#[test]
fn apply_accepts_canonicalized_zero_and_equivalent_rational_limits() -> Result<()> {
    type Spec = [Option<Option<u32>>; 3];
    let to_map = |spec: Spec| -> BTreeMap<String, Option<u32>> {
        KEYS.into_iter()
            .zip(spec)
            .filter_map(|(k, v)| v.map(|val| (k.to_string(), val)))
            .collect()
    };
    let check = |w: Spec, s: Spec| -> Result<()> {
        let dir = profile_fixture("[Framerate]\nLimit=120\nLimitDenominator=1\n")?;
        let (wanted, saved) = (to_map(w), to_map(s));
        apply_with(dir.path(), &wanted, Some(false), true, |req| {
            write_profile(&req.root, &saved)?;
            Ok(reply(&saved))
        })?;
        Ok(())
    };
    let s = |v| Some(Some(v));
    let n = Some(None);
    // RTSS 7.3.7 canonicalizes Limit=0 (any denominator), equivalent fractions, and denom 0 as 1.
    for (w, sv) in [
        ([s(0), s(1000), s(0)], [s(0), s(1), s(0)]),
        ([s(0), s(1000), None], [s(0), n, None]),
        ([s(2997), s(25), s(1)], [s(11988), s(100), s(1)]),
        ([s(60), s(0), None], [s(60), s(1), None]),
        ([None, s(1), None], [s(0), s(1000), None]),
    ] {
        check(w, sv)?;
    }
    // Non-equivalent caps or changed SyncLimiter still fail post-save verification.
    for (w, sv, key) in [
        (
            [s(2997), s(25), None],
            [s(2997), s(1), None],
            "LimitDenominator",
        ),
        (
            [None, s(1), None],
            [s(90), s(1000), None],
            "LimitDenominator",
        ),
        ([s(60), None, s(1)], [s(60), None, s(0)], "SyncLimiter"),
    ] {
        let err = check(w, sv).unwrap_err();
        assert!(err.to_string().contains(key), "{err}");
    }
    Ok(())
}
