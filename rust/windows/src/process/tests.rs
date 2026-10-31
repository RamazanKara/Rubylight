#[test]
fn a_process_says_whether_windows_gave_it_administrator_rights() {
    use super::*;
    let ping = std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap())
        .join("System32")
        .join("ping.exe");
    // Runs where the test runs: as this user, elevated or not.
    let elevated = is_system() || token_elevated(&token().unwrap()).unwrap();
    let args = ["-n", "30", "127.0.0.1"].map(OsString::from);
    let process = Process::spawn(
        &ping,
        &args,
        None,
        Target::User { elevated },
        &BTreeMap::new(),
        true,
    )
    .unwrap();
    let rights = process.elevated();
    process.terminate().unwrap();
    assert_eq!(rights.unwrap(), elevated);
}

#[test]
fn a_pipe_end_can_be_used_by_its_creator() {
    use super::*;
    use std::io::{Read, Write};
    let (child_end, parent_end) = pipe_pair(PipeEnd::Write).unwrap();
    let mut writer = std::fs::File::from(child_end);
    let mut reader = std::fs::File::from(parent_end);
    writer.write_all(b"ping").unwrap();
    drop(writer);
    let mut text = String::new();
    reader.read_to_string(&mut text).unwrap();
    assert_eq!(text, "ping");
}
#[test]
fn users_may_read_but_never_change_a_readable_folder() {
    use super::*;
    use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows::Win32::Storage::FileSystem::{
        DELETE, FILE_APPEND_DATA, FILE_DELETE_CHILD, FILE_LIST_DIRECTORY, FILE_WRITE_ATTRIBUTES,
        FILE_WRITE_DATA, FILE_WRITE_EA, READ_CONTROL, WRITE_DAC, WRITE_OWNER,
    };
    let users = USERS_READ
        .split('(')
        .find(|ace| ace.ends_with(";;;BU)"))
        .unwrap();
    let mask = u32::from_str_radix(
        users.split(';').nth(2).unwrap().trim_start_matches("0x"),
        16,
    )
    .unwrap();
    let write = DELETE.0
        | FILE_APPEND_DATA.0
        | FILE_DELETE_CHILD.0
        | FILE_WRITE_ATTRIBUTES.0
        | FILE_WRITE_DATA.0
        | FILE_WRITE_EA.0
        | WRITE_DAC.0
        | WRITE_OWNER.0
        | 0xf000_0000;
    assert_eq!(mask & write, 0, "{mask:#x}");
    // Open to check and read, as a host without administrator rights does.
    assert_eq!(
        mask & (FILE_LIST_DIRECTORY.0 | READ_CONTROL.0),
        FILE_LIST_DIRECTORY.0 | READ_CONTROL.0
    );
    // The rest is the administrators-only one's.
    assert!(USERS_READ.starts_with(ADMINISTRATORS_ONLY));
    for sddl in [ADMINISTRATORS_ONLY, USERS_READ] {
        let text = wide(std::ffi::OsStr::new(sddl)).unwrap();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `text` is NUL-terminated and outlives the call; `descriptor` is freed once.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(text.as_ptr()),
                1,
                &mut descriptor,
                None,
            )
            .unwrap();
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        }
    }
}
#[test]
fn a_file_is_checked_and_read_through_one_handle_in_a_folder_that_stays() {
    use super::*;
    use std::io::Read;
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let folder = root.join("windows-settings");
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("record.json");
    std::fs::write(&path, "first").unwrap();
    let held = open_unfollowed(&folder, true).unwrap();
    assert_eq!(
        open_file_owner(&held).unwrap(),
        file_owner(&folder).unwrap()
    );
    // The folder checked is the one the file is opened in.
    assert!(std::fs::rename(&folder, root.join("moved")).is_err());
    let mut file = open_unfollowed(&path, false).unwrap();
    assert_eq!(open_file_owner(&file).unwrap(), file_owner(&path).unwrap());
    // Not written while open; a newer record can still take the name, and what was
    // checked is what is read.
    assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
    let newer = folder.join("record.tmp");
    std::fs::write(&newer, "second").unwrap();
    std::fs::rename(&newer, &path).unwrap();
    let mut text = String::new();
    file.read_to_string(&mut text).unwrap();
    assert_eq!(text, "first");
    drop((file, held));
    std::fs::rename(&folder, root.join("moved")).unwrap();
    assert_eq!(
        open_unfollowed(&folder, true).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
}
#[test]
fn the_settings_record_writers_are_read_from_its_permissions() {
    use super::*;
    use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    let allowed = |sddl: &str, mask: u32| {
        let text = wide(std::ffi::OsStr::new(sddl)).unwrap();
        let mut descriptor = crate::ipc::Descriptor(PSECURITY_DESCRIPTOR::default());
        // SAFETY: `text` is NUL-terminated and outlives the call, and `dacl` points into
        // `descriptor`, which is freed once it drops, after its last use.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(text.as_ptr()),
                1,
                &mut descriptor.0,
                None,
            )
            .unwrap();
            let (mut present, mut defaulted) = (BOOL(0), BOOL(0));
            let mut dacl: *mut ACL = std::ptr::null_mut();
            GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut dacl, &mut defaulted)
                .unwrap();
            assert!(present.as_bool(), "{sddl}");
            dacl_allowed(dacl, mask).unwrap()
        }
    };
    let writers = |sddl: &str| allowed(sddl, WRITE);
    // Who may move a folder's contents: not those who may only add to it, as
    // Authenticated Users may to C:\.
    assert_eq!(
        allowed("D:P(A;;FA;;;SY)(A;;0x4;;;AU)", CHILD_MOVE),
        ["S-1-5-18"]
    );
    for rights in ["WD", "WO", "GA", "GW", "0x40"] {
        assert_eq!(
            allowed(&format!("D:P(A;;{rights};;;AU)"), CHILD_MOVE),
            ["S-1-5-11"],
            "{rights}"
        );
    }
    assert_eq!(writers(USERS_READ), ["S-1-5-18", "S-1-5-32-544"]);
    assert_eq!(writers(ADMINISTRATORS_ONLY), ["S-1-5-18", "S-1-5-32-544"]);
    assert_eq!(
        writers("D:P(A;;FA;;;SY)(A;;FW;;;BU)"),
        ["S-1-5-18", "S-1-5-32-545"]
    );
    // Permissions, ownership and deletion are write access too.
    for rights in ["WD", "WO", "SD", "GA", "GW", "0x40"] {
        assert_eq!(
            writers(&format!("D:P(A;;{rights};;;AU)")),
            ["S-1-5-11"],
            "{rights}"
        );
    }
    // Reading, denials and what applies only inside it allow no writes.
    assert!(writers("D:P(A;;FRFX;;;WD)(D;;FA;;;BU)(A;OICIIO;FA;;;CO)").is_empty());
    // No permissions at all, or an allowing entry of another kind, fail closed.
    assert_eq!(writers("D:NO_ACCESS_CONTROL"), ["S-1-1-0"]);
    assert_eq!(
        writers("D:P(OA;;CC;bf967aba-0de6-11d0-a285-00aa003049e2;;WD)"),
        ["ACE type 5"]
    );
}
#[test]
fn the_settings_record_files_are_made_with_their_permissions() {
    use super::*;
    use std::io::{ErrorKind, Write};
    let temporary = tempfile::tempdir().unwrap();
    let folder = temporary.path().join("windows-settings");
    let path = folder.join("record.tmp");
    let elevated = elevated().unwrap();
    let protected = |open: &std::fs::File| {
        assert_eq!(open_file_owner(open).unwrap(), "S-1-5-32-544");
        assert_eq!(
            open_file_writers(open).unwrap(),
            ["S-1-5-18", "S-1-5-32-544"]
        );
    };
    match create_folder_administrators_readable(&folder) {
        Ok(()) => {
            assert!(elevated);
            protected(&open_unfollowed(&folder, true).unwrap());
        }
        // Only an elevated process can make Administrators the owner; nothing is made.
        Err(error) => {
            assert!(!elevated, "{error}");
            assert!(!folder.exists());
            std::fs::create_dir(&folder).unwrap();
        }
    }
    // Anything with the name is left as it is.
    assert_eq!(
        create_folder_administrators_readable(&folder)
            .unwrap_err()
            .kind(),
        ErrorKind::AlreadyExists
    );
    std::fs::write(&path, "planted").unwrap();
    assert_eq!(
        create_file_administrators_readable(&path)
            .unwrap_err()
            .kind(),
        ErrorKind::AlreadyExists
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "planted");
    std::fs::remove_file(&path).unwrap();
    match create_file_administrators_readable(&path) {
        Ok(mut file) => {
            assert!(elevated);
            // Nothing else opens it while it is written.
            assert!(std::fs::File::open(&path).is_err());
            file.write_all(b"record").unwrap();
            drop(file);
            protected(&open_unfollowed(&path, false).unwrap());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "record");
        }
        Err(error) => {
            assert!(!elevated, "{error}");
            assert!(!path.exists());
        }
    }
}
#[test]
fn a_folder_for_administrators_alone_is_made_with_its_permissions() {
    use super::*;
    use std::io::ErrorKind;
    // List or read data, all, read.
    const READ: u32 = 0x1 | 0x1000_0000 | 0x8000_0000;
    let temporary = tempfile::tempdir().unwrap();
    let folder = temporary.path().join("nucleus-locks");
    let elevated = elevated().unwrap();
    match create_folder_administrators_only(&folder) {
        Ok(()) => {
            assert!(elevated);
            // Nobody but SYSTEM and Administrators can even open it, from
            // its creation on: the temporary folder lets Users read.
            let open = open_unfollowed(&folder, true).unwrap();
            assert_eq!(open_file_owner(&open).unwrap(), "S-1-5-32-544");
            let mut readers = open_file_allowed(&open, READ).unwrap();
            readers.sort();
            readers.dedup();
            assert_eq!(readers, ["S-1-5-18", "S-1-5-32-544"]);
            assert_eq!(
                open_file_writers(&open).unwrap(),
                ["S-1-5-18", "S-1-5-32-544"]
            );
        }
        // Only an elevated process can make Administrators the owner; nothing is made.
        Err(error) => {
            assert!(!elevated, "{error}");
            assert!(!folder.exists());
            std::fs::create_dir(&folder).unwrap();
        }
    }
    // Anything with the name is left as it is.
    assert_eq!(
        create_folder_administrators_only(&folder)
            .unwrap_err()
            .kind(),
        ErrorKind::AlreadyExists
    );
}
#[test]
fn a_piped_child_reads_stdin_and_answers_on_stdout_and_stderr_inside_the_job() {
    use super::*;
    use std::io::{Read, Write};
    let cmd =
        std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap()).join("System32\\cmd.exe");
    if !is_system()
        && target_token(&Target::User { elevated: false })
            .unwrap()
            .is_some()
    {
        // An elevated administrator host starts through the shell token, which cannot hand
        // over pipes; it must say so rather than start the program without them.
        let err = Process::spawn_piped(&cmd, &[], Target::User { elevated: false });
        assert!(format!("{:#}", err.err().unwrap()).contains("pipes"));
        return;
    }
    let system32 = cmd.parent().unwrap().to_path_buf();
    let target = || Target::User { elevated: false };
    // What goes to the child's stdin comes back on its stdout.
    let args = [OsString::from("^")];
    let mut echo = Process::spawn_piped(&system32.join("findstr.exe"), &args, target()).unwrap();
    echo.stdin.take().unwrap().write_all(b"hello\r\n").unwrap();
    let mut out = String::new();
    echo.stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    assert_eq!(out.trim(), "hello");
    assert_eq!(echo.process.wait(Duration::from_secs(10)).unwrap(), 0);
    // What the child writes to stderr arrives on stderr, and its exit code is its own.
    let args = [OsString::from("nothing-by-this-name")];
    let mut failing = Process::spawn_piped(&system32.join("where.exe"), &args, target()).unwrap();
    let (mut out, mut err) = (String::new(), String::new());
    failing
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    failing
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    assert!(
        out.is_empty() && !err.is_empty(),
        "stdout {out:?} stderr {err:?}"
    );
    assert_eq!(failing.process.wait(Duration::from_secs(10)).unwrap(), 1);
    // The child runs as the signed-in user, never as the SYSTEM parent.
    if is_system() {
        let mut who = Process::spawn_piped(&system32.join("whoami.exe"), &[], target()).unwrap();
        let mut name = String::new();
        who.stdout
            .take()
            .unwrap()
            .read_to_string(&mut name)
            .unwrap();
        assert!(
            !name.trim().is_empty() && !name.to_lowercase().contains("system"),
            "{name:?}"
        );
        println!("the child ran as {}", name.trim());
    }
    // A child that does not end is ended by terminate, and its pipes then close.
    let args = [
        OsString::from("-n"),
        OsString::from("30"),
        OsString::from("127.0.0.1"),
    ];
    let mut long = Process::spawn_piped(&system32.join("ping.exe"), &args, target()).unwrap();
    assert_eq!(long.process.exit_code().unwrap(), None);
    long.process.terminate().unwrap();
    let mut rest = String::new();
    long.stdout
        .take()
        .unwrap()
        .read_to_string(&mut rest)
        .unwrap();
    assert!(long.process.wait(Duration::from_secs(10)).is_ok());
}
#[test]
fn close_image_asks_only_that_image_s_windows_and_follows_a_restarted_one() {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::*;
    // Whether `pid` owns a top-level window, shown or not.
    fn has_window(pid: u32) -> bool {
        unsafe extern "system" fn find(window: HWND, data: LPARAM) -> BOOL {
            // SAFETY: `data` is the `&mut (u32, bool)` below, alive for the enumeration.
            unsafe {
                let found = &mut *(data.0 as *mut (u32, bool));
                let mut owner = 0;
                GetWindowThreadProcessId(window, Some(&mut owner));
                found.1 |= owner == found.0;
                BOOL(1)
            }
        }
        let mut found = (pid, false);
        // SAFETY: `found` outlives the synchronous enumeration that reads it through `find`.
        unsafe {
            let _ = EnumWindows(
                Some(find),
                LPARAM((&mut found as *mut (u32, bool)) as isize),
            );
        }
        found.1
    }
    let image_pid = |process: &Process, image: &str| {
        let job = process.process_ids().unwrap();
        processes()
            .unwrap()
            .into_iter()
            .find(|p| job.contains(&p.pid) && p.name.eq_ignore_ascii_case(image))
            .map(|p| p.pid)
    };
    let cmd =
        std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap()).join("System32\\cmd.exe");
    // cmd, which has no window, starts PowerShell holding a hidden form
    // that closes on WM_CLOSE, then keeps running itself. PowerShell
    // stands in for a Nucleus Co-op that restarted itself (not the job's
    // first process), cmd for a browser or game the job also holds.
    let form = "Add-Type -AssemblyName System.Windows.Forms; \
                    $f = New-Object System.Windows.Forms.Form; \
                    $f.add_FormClosed({ [System.Windows.Forms.Application]::ExitThread() }); \
                    $null = $f.Handle; [System.Windows.Forms.Application]::Run()";
    let line = format!(
        "{} /d /c start \"\" /b powershell.exe -NoProfile -NonInteractive -Command \"{form}\" & ping -n 60 127.0.0.1 >nul",
        quote(cmd.as_os_str()).unwrap()
    );
    // Runs where the elevated test runs: as this user, elevated or not.
    let elevated = is_system() || token_elevated(&token().unwrap()).unwrap();
    let process = Process::spawn_command(
        &cmd,
        &line,
        None,
        Target::User { elevated },
        &BTreeMap::new(),
        true,
        false,
    )
    .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let shell = loop {
        if let Some(pid) = image_pid(&process, "powershell.exe").filter(|pid| has_window(*pid)) {
            break pid;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "PowerShell's form never showed up"
        );
        std::thread::sleep(Duration::from_millis(200));
    };
    assert!(process.running_image("powershell.exe").unwrap());
    // Closing cmd sends PowerShell nothing: cmd has no window and stays,
    // and PowerShell keeps its form.
    assert!(
        !process
            .close_image("cmd.exe", Duration::from_millis(500))
            .unwrap()
    );
    assert!(has_window(shell), "PowerShell got a WM_CLOSE meant for cmd");
    assert_eq!(image_pid(&process, "powershell.exe"), Some(shell));
    // Closing PowerShell finds it though the job started cmd, and leaves
    // cmd running.
    assert!(
        process
            .close_image("powershell.exe", Duration::from_secs(20))
            .unwrap()
    );
    assert!(!process.running_image("powershell.exe").unwrap());
    assert!(process.running_image("cmd.exe").unwrap());
    process.terminate().unwrap();
}
#[test]
fn console_reattachment_refuses_other_users_and_transitional_sessions() {
    use super::*;
    assert!(reattach_blocker(4, 1, WTSDisconnected, 2, true, false, false).is_none());
    assert!(reattach_blocker(4, 1, WTSActive, 2, true, false, false).is_none());
    for (source, console, state, protocol, user, occupied) in [
        (4, 1, WTSDisconnected, 2, true, true),
        (4, 1, WTSDisconnected, 2, false, false),
        (4, 1, WTSConnected, 2, true, false),
        (4, 1, WTSDisconnected, 0, true, false),
        (4, u32::MAX, WTSDisconnected, 2, true, false),
        (4, 4, WTSDisconnected, 2, true, false),
        (4, 4, WTSActive, 0, true, false),
        (0, 1, WTSDisconnected, 2, true, false),
    ] {
        assert!(
            reattach_blocker(source, console, state, protocol, user, occupied, false).is_some()
        );
    }
}
#[test]
fn console_sign_in_host_explains_why_return_to_console_is_unavailable() {
    use super::*;
    // The installed service follows the console: after Remote Desktop it
    // runs at the sign-in screen, and the reason must say what to do.
    let reason = reattach_blocker(4, 4, WTSConnected, 0, false, false, false).unwrap();
    for text in [
        "sign-in screen",
        "started inside a Remote Desktop session",
        "reconnect Moonlight",
    ] {
        assert!(reason.contains(text), "missing {text} in {reason}");
    }
    // A signed-in console host keeps the existing explanation.
    assert_eq!(
        reattach_blocker(1, 1, WTSActive, 0, true, true, false),
        Some("This host is already attached to the physical console.")
    );
}
#[test]
fn occupied_console_requires_a_disconnected_verified_matching_owner() {
    use super::*;
    const OWNER: &str = "S-1-5-21-100-200-300-1001";
    const OTHER: &str = "S-1-5-21-100-200-300-1002";
    for (state, source, target, allowed) in [
        (Some(WTSDisconnected), Some(OWNER), Some(OWNER), true),
        (Some(WTSActive), Some(OWNER), Some(OWNER), false),
        (Some(WTSConnected), Some(OWNER), Some(OWNER), false),
        (None, Some(OWNER), Some(OWNER), false),
        (Some(WTSDisconnected), Some(OWNER), Some(OTHER), false),
        (Some(WTSDisconnected), None, Some(OWNER), false),
        (Some(WTSDisconnected), Some(OWNER), None, false),
        (Some(WTSDisconnected), None, None, false),
        (Some(WTSDisconnected), Some(""), Some(""), false),
    ] {
        let verified = same_disconnected_owner(state, source, target);
        for host_state in [WTSActive, WTSDisconnected] {
            assert_eq!(
                reattach_blocker(4, 1, host_state, 2, true, true, verified).is_none(),
                allowed,
                "target state {state:?}, source {source:?}, target {target:?}"
            );
        }
    }
    // An owner match cannot authorize a different source or a changing console.
    for (source, console, state, protocol, signed_in) in [
        (0, 1, WTSDisconnected, 2, true),
        (4, u32::MAX, WTSDisconnected, 2, true),
        (4, 4, WTSDisconnected, 2, true),
        (4, 1, WTSConnected, 2, true),
        (4, 1, WTSDisconnected, 0, true),
        (4, 1, WTSDisconnected, 2, false),
    ] {
        assert!(
            reattach_blocker(source, console, state, protocol, signed_in, true, true).is_some()
        );
    }
}
#[test]
fn console_owner_verification_rechecks_the_target_after_reading_both_tokens() {
    use super::*;
    use std::cell::Cell;
    for (final_state, final_console, allowed) in [
        (WTSDisconnected, 1, true),
        (WTSActive, 1, false),
        (WTSDisconnected, 2, false),
    ] {
        let state_reads = Cell::new(0);
        let sid_reads = Cell::new(0);
        let verified = verify_disconnected_console_owner(
            4,
            1,
            |session| {
                assert_eq!(session, 1);
                let read = state_reads.get();
                state_reads.set(read + 1);
                assert_eq!(sid_reads.get(), if read == 0 { 0 } else { 2 });
                Ok(if read == 0 {
                    WTSDisconnected
                } else {
                    final_state
                })
            },
            |session| {
                assert_eq!(state_reads.get(), 1);
                assert_eq!(session, if sid_reads.get() == 0 { 4 } else { 1 });
                sid_reads.set(sid_reads.get() + 1);
                Ok("S-1-5-21-100-200-300-1001".into())
            },
            || {
                assert_eq!(state_reads.get(), 2);
                assert_eq!(sid_reads.get(), 2);
                final_console
            },
        )
        .unwrap();
        assert_eq!(verified, allowed);
        assert_eq!(state_reads.get(), 2);
        assert_eq!(sid_reads.get(), 2);
    }
}
#[test]
fn console_owner_verification_fails_closed_when_either_token_query_fails() {
    use super::*;
    use std::cell::Cell;
    for failed_session in [4, 1] {
        let state_reads = Cell::new(0);
        let sid_reads = Cell::new(0);
        let error = verify_disconnected_console_owner(
            4,
            1,
            |session| {
                assert_eq!(session, 1);
                state_reads.set(state_reads.get() + 1);
                Ok(WTSDisconnected)
            },
            |session| {
                sid_reads.set(sid_reads.get() + 1);
                anyhow::ensure!(session != failed_session, "session token unavailable");
                Ok("S-1-5-21-100-200-300-1001".into())
            },
            || panic!("do not accept a console after an identity query failed"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("session token unavailable"));
        assert_eq!(state_reads.get(), 1);
        assert_eq!(sid_reads.get(), if failed_session == 4 { 1 } else { 2 });
    }
}
#[test]
fn command_cancellation_ends_the_owned_process_before_returning() -> anyhow::Result<()> {
    use super::*;
    let cancel = CommandCancellation::new()?;
    let elevated = user_environment().is_err();
    let child = Process::shell(
        "ping -n 120 127.0.0.1 >nul",
        None,
        elevated,
        &BTreeMap::new(),
    )?;
    cancel.cancel()?;
    let started = std::time::Instant::now();
    assert_eq!(
        child.wait_cancellable(Duration::from_secs(120), &cancel)?,
        None
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(child.exit_code()?.is_some());
    assert_eq!(child.active_processes()?, 0);
    Ok(())
}
#[test]
fn streaming_session_rejects_disconnected_rdp_and_session_transitions() {
    for state in [
        WTSConnected,
        WTSDisconnected,
        WTSConnectQuery,
        WTSReset,
        WTSDown,
        WTSInit,
    ] {
        let error = validate_streaming_session(state, false).unwrap_err();
        assert!(error.to_string().contains("disconnected or changing"));
        assert!(error.to_string().contains("RDP"));
    }
    for state in [WTSConnectQuery, WTSReset, WTSDown, WTSInit] {
        assert!(validate_streaming_session(state, true).is_err());
    }
}
#[test]
fn streaming_session_preserves_active_sessions_and_console_sign_in() {
    assert!(validate_streaming_session(WTSActive, false).is_ok());
    for state in [WTSActive, WTSConnected, WTSDisconnected] {
        assert!(validate_streaming_session(state, true).is_ok());
    }
}
#[test]
fn streaming_gate_queries_the_host_session_instead_of_the_console() {
    use super::*;
    for (state, allowed) in [(WTSActive, true), (WTSDisconnected, false)] {
        let result = ensure_streaming_session_with(
            || Ok(4),
            || 1,
            |session| {
                assert_eq!(session, 4, "query the host's own RDP session");
                Ok((state, 2))
            },
            |_| panic!("only the console exception lists other sessions"),
        );
        assert_eq!(result.is_ok(), allowed);
    }
    for state in [WTSConnected, WTSDisconnected] {
        assert!(
            ensure_streaming_session_with(|| Ok(1), || 1, |_| Ok((state, 0)), |_| Ok(vec![]))
                .is_ok()
        );
        assert!(
            ensure_streaming_session_with(
                || Ok(1),
                || 1,
                |_| Ok((state, 2)),
                |_| panic!("a refused session needs no session list")
            )
            .is_err()
        );
    }
}
#[test]
fn user_programs_start_in_the_hosts_own_session_not_the_consoles() {
    // A host in session 3 while the console (session 1) is the
    // user's main desktop: its capture worker gets session 3's user.
    let asked = super::own_session_user(|| Ok(3), Ok).unwrap();
    assert_eq!(asked, 3);
    let error = super::own_session_user::<u32>(
        || Err(anyhow::anyhow!("no session")),
        |_| panic!("no session, no user token"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("no session"));
}
fn warnings(check: impl FnOnce()) -> String {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.log");
    let file = std::fs::File::create(&path).unwrap();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_target(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || file.try_clone().unwrap())
        .finish();
    tracing::subscriber::with_default(subscriber, check);
    std::fs::read_to_string(path).unwrap()
}
#[test]
fn refused_streaming_session_is_logged_at_warn_with_the_checked_session() {
    use super::*;
    let log = warnings(|| {
        let error = ensure_streaming_session_with(
            || Ok(4),
            || 1,
            |_| Ok((WTSDisconnected, 2)),
            |_| panic!("a refused session needs no session list"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("disconnected or changing"));
    });
    for field in [
        "WARN",
        "session_id=4",
        "console_session_id=1",
        "state=4",
        "protocol=2",
        "allowed=false",
        "decision=\"disconnected_or_changing\"",
    ] {
        assert!(log.contains(field), "missing {field} in {log}");
    }
    // Accepted active sessions stay out of the warning log.
    let log = warnings(|| {
        ensure_streaming_session_with(
            || Ok(1),
            || 1,
            |_| Ok((WTSActive, 0)),
            |_| panic!("an active session needs no session list"),
        )
        .unwrap();
    });
    assert!(log.is_empty(), "{log}");
}
#[test]
fn sign_in_screen_launch_warns_about_a_disconnected_desktop_without_blocking() {
    use super::*;
    // Reporter's host: the service runs on console session 4 at the
    // sign-in screen while the RDP desktop waits as disconnected session 1.
    let log = warnings(|| {
        ensure_streaming_session_with(
            || Ok(4),
            || 4,
            |_| Ok((WTSConnected, 0)),
            |host| {
                assert_eq!(host, 4);
                Ok(vec![1])
            },
        )
        .unwrap();
    });
    for field in [
        "WARN",
        "session_id=4",
        "state=1",
        "disconnected_session_ids=[1]",
        "reconnect Moonlight",
    ] {
        assert!(log.contains(field), "missing {field} in {log}");
    }
    // No disconnected desktop, or no session list, keeps the existing
    // sign-in support silent and allowed.
    for list in [Ok(vec![]), Err(anyhow::anyhow!("enumeration failed"))] {
        let log = warnings(|| {
            ensure_streaming_session_with(|| Ok(4), || 4, |_| Ok((WTSConnected, 0)), |_| list)
                .unwrap();
        });
        assert!(log.is_empty(), "{log}");
    }
}
#[test]
fn disconnected_desktops_exclude_services_the_host_and_sessions_without_a_user() {
    use super::*;
    let sessions = [
        (0, WTSDisconnected),
        (1, WTSDisconnected),
        (2, WTSDisconnected),
        (3, WTSActive),
        (4, WTSConnected),
        (5, WTSDisconnected),
        (6, WTSDisconnected),
    ];
    let found = disconnected_desktops(6, &sessions, |session| match session {
        1 => Ok(true),
        2 => Ok(false),
        5 => anyhow::bail!("user name unavailable"),
        0 | 6 => panic!("session {session} is never a candidate"),
        _ => panic!("only disconnected sessions are checked, not {session}"),
    });
    assert_eq!(found, vec![1]);
}
#[test]
fn streaming_gate_fails_closed_when_the_host_session_cannot_be_queried() {
    use super::*;
    let error = ensure_streaming_session_with(
        || anyhow::bail!("process session unavailable"),
        || panic!("do not fall back to the console session"),
        |_| panic!("do not query a substitute session"),
        |_| panic!("do not list sessions without the host session"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("process session unavailable"));
    let error = ensure_streaming_session_with(
        || Ok(4),
        || 1,
        |session| {
            assert_eq!(session, 4);
            anyhow::bail!("connection state unavailable")
        },
        |_| panic!("do not list sessions without a connection state"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("connection state unavailable"));
}
#[test]
fn the_process_list_names_this_test_and_its_program() {
    let me = std::process::id();
    let list = super::processes().unwrap();
    let this = list.iter().find(|p| p.pid == me).unwrap();
    assert!(this.started > 0);
    assert!(this.name.to_ascii_lowercase().ends_with(".exe"));
    let image = super::image_path(me).unwrap();
    assert!(image.ends_with(&this.name), "{image}");
    // Stopping a process list whose start times do not match leaves it alone.
    super::stop_processes(
        &std::collections::BTreeMap::from([(me, this.started + 1)]),
        std::time::Duration::ZERO,
    );
}
#[test]
fn urls_and_documents_open_through_their_association() {
    use super::{command_target, shell_command};
    assert_eq!(
        shell_command("steam://open/bigpicture"),
        "start \"\" /wait \"steam://open/bigpicture\""
    );
    assert_eq!(
        shell_command("ms-settings:display"),
        "start \"\" /wait \"ms-settings:display\""
    );
    assert_eq!(
        shell_command("\"C:\\Games\\My Game.lnk\" -windowed"),
        "start \"\" /wait \"C:\\Games\\My Game.lnk\" -windowed"
    );
    for unchanged in [
        "\"C:\\Program Files\\Game\\game.exe\" -fullscreen",
        "C:\\tools\\setup.bat",
        "powershell -NoProfile -Command Get-Date",
        "start \"\" steam://run/570",
        "",
    ] {
        assert_eq!(shell_command(unchanged), unchanged);
    }
    assert_eq!(command_target("\"C:\\a b\\c.exe\" x"), "C:\\a b\\c.exe");
}
#[test]
fn a_commands_arguments_are_what_follows_its_program() {
    assert_eq!(
        command_arguments(r#" "C:\a b\c.exe" -game "x y" "#),
        r#"-game "x y""#
    );
    assert_eq!(command_arguments(r"C:\Ice\Ice.exe  -windowed"), "-windowed");
    assert_eq!(command_arguments(r"C:\Ice\Ice.exe"), "");
    assert_eq!(command_arguments(r#""C:\a b\c.exe""#), "");
}
#[test]
fn supervisor_event_is_wait_only_and_outlives_the_source_handle() -> Result<()> {
    let source = HostStop::new()?;
    let (pid, handle) = parse_stop_source(&source.source())?;
    let signal = StopSignal(Some(duplicate_stop_event(pid, handle)?));
    assert!(!signal.requested());
    let event = raw(signal.0.as_ref().unwrap());
    assert_eq!(
        // SAFETY: `event` is a handle owned by `signal`, which is alive here.
        unsafe { SetEvent(event) }.unwrap_err().code(),
        windows::core::HRESULT::from_win32(ERROR_ACCESS_DENIED.0)
    );
    source.request()?;
    drop(source);
    assert!(signal.requested());
    // SAFETY: `event` is a handle owned by `signal`, which is alive here.
    assert!(unsafe { ResetEvent(event) }.is_err());
    Ok(())
}
#[test]
fn shutdown_source_rejects_invalid_handles_and_foreign_parents() -> Result<()> {
    for source in [
        "",
        "1",
        "0:1",
        "1:0",
        "1:-1",
        "1:1:2",
        "+1:2",
        "1:xyz",
        "4294967296:1",
        "1:ffffffffffffffffffffffff",
    ] {
        assert!(parse_stop_source(source).is_err(), "{source}");
    }
    assert!(StopSignal::new(Some(&HostStop::new()?.source())).is_err());
    assert!(!StopSignal::new(None)?.requested());
    Ok(())
}
#[test]
fn shell_runs_grouped_commands_with_quoted_output_paths() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("quoted output.log");
    let command = format!("(echo NATIVE_SHELL) 1>\"{}\" 2>&1", output.display());
    let child = super::Process::shell(
        &command,
        Some(directory.path()),
        target_token(&Target::User { elevated: true }).is_ok(),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(child.wait(std::time::Duration::from_secs(5)).unwrap(), 0);
    assert_eq!(
        std::fs::read_to_string(output).unwrap().trim(),
        "NATIVE_SHELL"
    );
}
#[test]
fn portable_non_elevated_targets_never_reuse_an_elevated_token() -> Result<()> {
    if is_system() {
        return Ok(());
    }
    let selected = match target_token(&Target::User { elevated: false }) {
        Ok(selected) => selected,
        Err(_) => {
            assert!(Process::shell("exit /b 0", None, false, &BTreeMap::new()).is_err());
            return Ok(());
        }
    };
    let current = token()?;
    // With User Account Control off the desktop shell is elevated too and
    // no standard-rights token exists: the program gets the desktop's
    // rights, never more than the user's own desktop has.
    if token_elevated(&current)? && shell_token(&current)?.is_none() {
        assert!(selected.is_none());
        return Ok(());
    }
    let mut info = TOKEN_ELEVATION::default();
    let mut bytes = 0;
    // SAFETY: the selected/current token and TOKEN_ELEVATION output live through the call.
    unsafe {
        GetTokenInformation(
            raw(selected.as_ref().unwrap_or(&current)),
            TokenElevation,
            Some((&mut info as *mut TOKEN_ELEVATION).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut bytes,
        )?;
    }
    assert_eq!(info.TokenIsElevated, 0);
    Ok(())
}
#[test]
fn administrator_hosts_start_non_elevated_programs_with_standard_rights() {
    if is_system() {
        return;
    }
    // An elevated portable host previously failed every non-elevated
    // program with ERROR_BAD_IMPERSONATION_LEVEL.
    let environment = user_environment().unwrap();
    assert!(environment.contains_key("USERPROFILE"));
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("groups.txt");
    // Windows' own whoami: a PATH with Git's usr\bin first finds one without /groups.
    let windir = std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap());
    let whoami = windir.join("System32\\whoami.exe");
    let child = Process::shell(
        // Unquoted: cmd /c would strip the first and last of several quotes.
        &format!(
            "{} /groups 1>\"{}\" 2>&1",
            whoami.display(),
            output.display()
        ),
        None,
        false,
        &BTreeMap::new(),
    )
    .unwrap();
    // whoami /groups took 12 s on a PC busy with parallel builds: the limit only
    // catches a hang, not slowness.
    let code = child.wait(Duration::from_secs(60)).unwrap();
    // Localized output uses the OEM code page; the SIDs are ASCII.
    let groups = String::from_utf8_lossy(&std::fs::read(output).unwrap_or_default()).into_owned();
    assert_eq!(code, 0, "{groups}");
    // Medium mandatory level, never High, unless User Account Control is
    // off (as on CI runners): then the desktop itself is elevated and the
    // program gets what the desktop would give it rather than failing.
    let own = token().unwrap();
    let standard = !token_elevated(&own).unwrap() || shell_token(&own).unwrap().is_some();
    assert_eq!(groups.contains("S-1-16-8192"), standard, "{groups}");
    assert_eq!(groups.contains("S-1-16-12288"), !standard, "{groups}");
}
#[test]
fn programs_started_without_a_token_get_this_hosts_environment() {
    // A non-elevated host gave its programs the system's variables, and the PyroWave probe
    // worker missed the AMD shader cache under C:\Users\Default, compiled every pipeline
    // again and timed out.
    let environment = environment(None).unwrap();
    for name in ["USERPROFILE", "USERNAME", "TEMP", "PATH"] {
        assert_eq!(
            environment.get(name).map(String::as_str),
            std::env::var(name).ok().as_deref(),
            "{name}"
        );
    }
    if !is_system() {
        // Apps and client commands get this one, from whichever token the host uses.
        let user = user_environment().unwrap();
        for name in ["USERPROFILE", "USERNAME"] {
            assert_eq!(user.get(name), environment.get(name), "{name}");
        }
    }
}
#[test]
fn programs_started_for_the_user_keep_the_users_profile() {
    if is_system() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("user.txt");
    // `/u` writes UTF-16, so a profile name outside the OEM code page reads back intact.
    let child = Process::shell(
        &format!("cmd /d /u /c set USER 1>\"{}\" 2>&1", output.display()),
        None,
        false,
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(child.wait(Duration::from_secs(10)).unwrap(), 0);
    let bytes = std::fs::read(output).unwrap();
    let wide: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    let text = String::from_utf16_lossy(&wide);
    let seen: BTreeMap<_, _> = text
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(name, value)| (name.to_uppercase(), value.to_owned()))
        .collect();
    for name in ["USERPROFILE", "USERNAME"] {
        assert_eq!(
            seen.get(name),
            std::env::var(name).ok().as_ref(),
            "{name}: {text}"
        );
    }
}
#[test]
fn the_signed_in_account_is_told_apart() {
    // User Account Control: an administrator's filtered token, linked
    // to their own elevated one.
    assert_eq!(
        account_of(Some((true, true)), false, true),
        Account::Elevates
    );
    // Administrator protection: linked to another account's, or to none.
    assert_eq!(
        account_of(Some((true, false)), false, true),
        Account::Protected
    );
    assert_eq!(account_of(None, false, true), Account::Protected);
    // User Account Control off: elevated itself.
    assert_eq!(account_of(None, true, true), Account::Elevates);
    // A standard account, linked to nothing or nothing elevated.
    assert_eq!(account_of(None, false, false), Account::Standard);
    assert_eq!(
        account_of(Some((false, true)), false, false),
        Account::Standard
    );
}
#[test]
fn the_program_starts_only_as_the_signed_in_users_own_account() {
    let user = "S-1-5-21-1-2-3-1001";
    let other = "S-1-5-21-1-2-3-1002";
    // User Account Control's elevated token is the same account's.
    own_linked_account(user, user).unwrap();
    // Administrator protection's separate administrator is never started
    // as, and the refusal never advises turning it off.
    let refused = format!("{:#}", own_linked_account(user, other).unwrap_err());
    assert_eq!(
        refused,
        "Starting it elevated needs administrator rights, and Windows' Administrator protection keeps them from Rubylight"
    );
    // A start for the account whose copy a launch checked runs as it,
    // or not at all.
    same_account(user, user).unwrap();
    let changed = format!("{:#}", same_account(user, other).unwrap_err());
    assert!(changed.contains("another account signed in"), "{changed}");
    // Outside the service, a program runs as this process's account.
    if !is_system() && !elevated().unwrap() {
        let own = own_sid().unwrap();
        assert!(target_token(&Target::Account(own)).unwrap().is_none());
        assert!(target_token(&Target::Account(other.into())).is_err());
    }
}
#[test]
fn this_hosts_own_token_names_administrators_when_elevated() {
    let token = token().unwrap();
    let administrator = token_in_administrators(&token).unwrap();
    if elevated().unwrap() {
        assert!(administrator);
    }
    if !is_system() {
        // Outside the service the account is the host's own rights.
        match user_account() {
            Ok(account) => assert_eq!(account, Account::Elevates),
            Err(_) => assert!(!elevated().unwrap()),
        }
    }
}
#[test]
fn user_elevation_is_the_elevated_targets_own() {
    if is_system() {
        return;
    }
    // Outside the service only an elevated host starts elevated programs.
    match user_runs_elevated() {
        Ok(runs) => assert!(runs && elevated().unwrap()),
        Err(error) => {
            assert!(!elevated().unwrap());
            assert!(
                format!("{error:#}").contains("Windows service"),
                "{error:#}"
            );
        }
    }
}
#[test]
fn portable_user_identity_does_not_require_a_non_elevated_token() {
    if !is_system() {
        assert!(user_sid().is_some_and(|sid| !sid.is_empty()));
    }
}
use super::*;
#[test]
fn dropping_the_job_terminates_the_shell_and_its_child() {
    let process = Process::shell(
        "ping -n 30 127.0.0.1 >nul",
        None,
        target_token(&Target::User { elevated: true }).is_ok(),
        &BTreeMap::new(),
    )
    .unwrap();
    let mut ids = [0usize; 34];
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let count = loop {
        // SAFETY: `ids` outlives the call and its byte size is passed.
        unsafe {
            QueryInformationJobObject(
                Some(raw(&process.job)),
                JobObjectBasicProcessIdList,
                ids.as_mut_ptr().cast(),
                std::mem::size_of_val(&ids) as u32,
                None,
            )
            .unwrap();
        }
        // SAFETY: `ids` holds a JOBOBJECT_BASIC_PROCESS_ID_LIST whose second u32, inside the
        // array, is NumberOfProcessIdsInList.
        let count = unsafe { *ids.as_ptr().cast::<u32>().add(1) } as usize;
        if count >= 2 {
            break count;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "child never joined the owned Windows job"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(count <= 32);
    let children: Vec<_> = ids[1..=count]
        .iter()
        .map(|id| {
            // SAFETY: OpenProcess has no memory preconditions and the returned handle is owned
            // once.
            owned(unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, *id as u32).unwrap() })
        })
        .collect();
    drop(process);
    for child in children {
        assert_eq!(
            // SAFETY: `child` is an owned process handle, open for the wait.
            unsafe { WaitForSingleObject(raw(&child), 5000) },
            WAIT_OBJECT_0
        );
    }
}
