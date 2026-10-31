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
