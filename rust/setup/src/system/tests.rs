use super::*;
fn sid_of(text: &str) -> Result<PSID> {
    let mut sid = PSID::default();
    // SAFETY: the test leaks the SID; it lives as long as the test.
    unsafe {
        windows::Win32::Security::Authorization::ConvertStringSidToSidW(
            &HSTRING::from(text),
            &mut sid,
        )?;
    }
    Ok(sid)
}
fn set_owner(path: &Path, sid: &str) -> Result<()> {
    let (code, output) = run(
        &system32("icacls.exe"),
        &[
            &path.display().to_string(),
            "/setowner",
            &format!("*{sid}"),
            "/Q",
        ],
        Duration::from_secs(10),
    )?;
    assert_eq!(code, 0, "{output}");
    Ok(())
}
#[test]
fn elevated_arguments_read_back_as_they_were_given() {
    use windows::Win32::UI::Shell::CommandLineToArgvW;
    let given = [
        "--quiet",
        r"D:\My Games\Rubylight\",
        r"D:\Games\Rubylight\",
        r#"a "b" c"#,
        r#"ends in \" "#,
        r"two\\ slashes\\",
        "",
        "tab\there",
    ];
    assert_eq!(argument("--quiet"), "--quiet");
    assert_eq!(argument(r"D:\Games\Rubylight\"), r"D:\Games\Rubylight\");
    assert_eq!(
        argument(r"D:\My Games\Rubylight\"),
        r#""D:\My Games\Rubylight\\""#
    );
    assert_eq!(argument("").len(), 2);
    let line = std::iter::once("setup.exe".to_owned())
        .chain(given.iter().map(|a| argument(a)))
        .collect::<Vec<_>>()
        .join(" ");
    let mut count = 0;
    // SAFETY: the line is a terminated string that outlives the call, and
    // the returned array of `count` terminated strings is read before it
    // is freed.
    let read: Vec<String> = unsafe {
        let parts = CommandLineToArgvW(&HSTRING::from(line), &mut count);
        assert!(!parts.is_null());
        let read = std::slice::from_raw_parts(parts, count as usize)
            .iter()
            .map(|part| part.to_string().unwrap())
            .collect();
        let _ = windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(
            parts.cast(),
        )));
        read
    };
    assert_eq!(read[1..], given);
}
#[test]
fn an_entry_removed_during_the_tree_check_is_told_from_other_errors() -> Result<()> {
    // An update checks the whole profile while the host runs and
    // replaces its files: one gone since its folder was listed must not
    // fail the update.
    let temp = tempfile::tempdir()?;
    let gone = temp.path().join("gone");
    for error in [
        anyhow::Error::from(std::fs::symlink_metadata(&gone).unwrap_err()),
        std::fs::read_dir(&gone).unwrap_err().into(),
        trusted_owner(&gone, false).unwrap_err(),
        other_writer(&gone).unwrap_err(),
    ] {
        assert!(vanished(&gone, &error), "{error:#}");
    }
    let child = gone.join("child");
    assert!(vanished(&child, &trusted_owner(&child, false).unwrap_err()));
    assert!(!vanished(&gone, &Untrusted("planted".into()).into()));
    // Not-found about an entry that is still there (a path too long for
    // the permission calls) is no reason to skip it.
    std::fs::create_dir(temp.path().join("there"))?;
    let not_found_error =
        || anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::NotFound));
    assert!(!vanished(&temp.path().join("there"), &not_found_error()));
    // Those calls read a link itself, not what it leads to (measured on
    // Windows 11): an entry its owner swaps for a junction to nowhere
    // after it was looked at is still read as theirs, never not found.
    let swapped = temp.path().join("swapped");
    let (code, output) = run(
        &system32("cmd.exe"),
        &[
            "/c",
            "mklink",
            "/J",
            &swapped.display().to_string(),
            &gone.display().to_string(),
        ],
        Duration::from_secs(10),
    )?;
    assert_eq!(code, 0, "{output}");
    if let Err(error) = trusted_owner(&swapped, false) {
        assert!(!not_found(&error), "{error:#}");
    }
    other_writer(&swapped)?;
    assert!(!vanished(&swapped, &not_found_error()));
    // The folder checked must be there.
    assert!(trusted_tree(&gone, &[]).is_err());
    Ok(())
}
#[test]
fn only_the_system_drive_root_may_belong_to_an_administrator_account() -> Result<()> {
    // Issue 61: a C:\ owned by the PC's administrator refused every install.
    let drive = std::env::var("SystemDrive")?;
    assert!(system_drive_root(Path::new(&format!("{drive}\\"))));
    assert!(system_drive_root(Path::new(&format!(
        "\\\\?\\{}\\",
        drive.to_lowercase()
    ))));
    assert!(!system_drive_root(Path::new(&format!("{drive}\\Games"))));
    assert!(!system_drive_root(Path::new(&drive)));
    let other = if drive.eq_ignore_ascii_case("C:") {
        "D:\\"
    } else {
        "C:\\"
    };
    assert!(!system_drive_root(Path::new(other)));
    assert!(!system_drive_root(Path::new("\\\\server\\share\\")));

    let (_, identity) = run(
        &system32("whoami.exe"),
        &["/user", "/fo", "csv", "/nh"],
        Duration::from_secs(10),
    )?;
    let mut fields = identity.trim().split(',').map(|f| f.trim_matches('"'));
    let name = fields
        .next()
        .context("whoami omitted the name")?
        .to_string();
    let sid = fields.next().context("whoami omitted SID")?.to_string();
    if sid == "S-1-5-18" {
        return Ok(());
    }
    // Direct members, as Windows lists them, independently of netapi32.
    let (code, listed) = run(
        &system32("WindowsPowerShell\\v1.0\\powershell.exe"),
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "if ((Get-LocalGroupMember -SID S-1-5-32-544).SID.Value -contains '{sid}') {{ 'listed' }} else {{ 'not listed' }}"
            ),
        ],
        Duration::from_secs(60),
    )?;
    // Get-LocalGroupMember throws on a member it cannot resolve (a
    // deleted domain account); compare only when it answered.
    let listed = match (code, listed.trim()) {
        (0, "listed") => Some(true),
        (0, "not listed") => Some(false),
        _ => None,
    };
    // SAFETY: the SIDs are valid for the whole test.
    unsafe {
        if let Some(listed) = listed {
            assert_eq!(administrator_account(sid_of(&sid)?)?, listed);
        }
        assert!(!administrator_account(sid_of("S-1-5-32-545")?)?);
    }

    let root = tempfile::tempdir()?;
    let folder = root.path().join("drive");
    std::fs::create_dir(&folder)?;
    set_owner(&folder, &sid)?;
    if let Some(listed) = listed {
        assert_eq!(trusted_owner(&folder, true).is_ok(), listed);
    }
    let error = trusted_owner(&folder, false).unwrap_err().to_string();
    assert!(
        error.to_lowercase().ends_with(&format!(
            "must be owned by administrators, system or trustedinstaller, not {}",
            name.to_lowercase()
        )),
        "{error}"
    );
    // Below the root, nothing changes: the folder itself and D:\Games.
    assert!(trusted_folder(&folder).is_err());
    assert!(trusted_parents(&folder.join("Rubylight")).is_err());
    if elevated() {
        set_owner(&folder, "S-1-5-32-545")?;
        let error = trusted_owner(&folder, true).unwrap_err().to_string();
        assert!(
            error.contains("or an administrator's account, not "),
            "{error}"
        );
    }
    Ok(())
}
#[test]
fn setup_loads_no_dll_planted_beside_it() -> Result<()> {
    // setup.exe runs elevated from Downloads: a DLL it imports that is not
    // a KnownDLL would be loaded from beside it first. These are the
    // DLLs behind setup's own Windows calls (userenv.dll is left out:
    // only the MSVC test build's std imports it).
    let folder = tempfile::tempdir()?;
    let copy = folder.path().join("setup-copy.exe");
    std::fs::copy(std::env::current_exe()?, &copy)?;
    for dll in [
        "netapi32.dll",
        "version.dll",
        "comctl32.dll",
        "wtsapi32.dll",
        "iphlpapi.dll",
        "msi.dll",
        "dbghelp.dll",
    ] {
        std::fs::write(folder.path().join(dll), b"not a DLL")?;
    }
    let (code, output) = run(
        &copy.display().to_string(),
        &["--list"],
        Duration::from_secs(60),
    )?;
    assert_eq!(code, 0, "{output}");
    Ok(())
}
#[test]
fn locked_folders_replace_user_ownership_and_explicit_child_grants() -> Result<()> {
    if !elevated() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let folder = root.path().join("locked");
    std::fs::create_dir(&folder)?;
    let child = folder.join("host.exe");
    std::fs::write(&child, b"fixture")?;
    let (_, identity) = run(
        &system32("whoami.exe"),
        &["/user", "/fo", "csv", "/nh"],
        Duration::from_secs(10),
    )?;
    let sid = identity
        .trim()
        .split(',')
        .nth(1)
        .context("whoami omitted SID")?
        .trim_matches('"');
    for path in [&folder, &child] {
        let (code, output) = run(
            &system32("icacls.exe"),
            &[
                &path.display().to_string(),
                "/grant:r",
                "*S-1-1-0:(F)",
                "/Q",
            ],
            Duration::from_secs(10),
        )?;
        assert_eq!(code, 0, "{output}");
        let (code, output) = run(
            &system32("icacls.exe"),
            &[
                &path.display().to_string(),
                "/setowner",
                &format!("*{sid}"),
                "/Q",
            ],
            Duration::from_secs(10),
        )?;
        assert_eq!(code, 0, "{output}");
    }
    assert!(trusted_folder(&folder).is_err());
    restrict(&folder, false)?;
    inherit_contents(&folder, &[])?;
    trusted_folder(&folder)?;
    trusted_folder(&child)?;
    for path in [&folder, &child] {
        let (_, output) = run(
            &system32("icacls.exe"),
            &[&path.display().to_string(), "/findsid", "*S-1-1-0", "/Q"],
            Duration::from_secs(10),
        )?;
        assert!(!output.contains(&path.display().to_string()), "{output}");
    }
    Ok(())
}
#[test]
fn cmd_lines_keep_quoted_paths_with_spaces() -> Result<()> {
    // The shape of an NSIS QuietUninstallString under Program Files, and
    // the uninstaller's own delayed self-deletion.
    let root = tempfile::tempdir()?;
    let folder = root.path().join("Sunshine Test");
    std::fs::create_dir(&folder)?;
    let uninstaller = folder.join("uninstall.cmd");
    let marker = folder.join("ran.txt");
    std::fs::write(&uninstaller, "@echo %1> \"%~dp0ran.txt\"\r\n")?;
    let (code, _) = run_cmd(
        &format!("\"{}\" /S", uninstaller.display()),
        Duration::from_secs(30),
    )?;
    assert_eq!(code, 0);
    assert_eq!(std::fs::read_to_string(&marker)?.trim(), "/S");
    let (code, _) = run_cmd(
        &format!(
            "del /f /q \"{}\" & del /f /q \"{}\"",
            marker.display(),
            uninstaller.display()
        ),
        Duration::from_secs(30),
    )?;
    assert_eq!(code, 0);
    assert!(!marker.exists() && !uninstaller.exists());
    Ok(())
}

#[test]
fn a_wake_plan_in_setups_environment_never_reaches_the_host() -> Result<()> {
    // SAFETY: Windows' environment block is locked per call; no test in
    // this crate reads this variable.
    unsafe {
        std::env::set_var(
            "BUTTERPOLLO_WAKE_FIX",
            r#"{"adapters":[],"fast_startup_off":true}"#,
        )
    };
    let (code, output) = run_cmd(
        "if defined BUTTERPOLLO_WAKE_FIX (exit 7) else (exit 0)",
        Duration::from_secs(30),
    )?;
    // SAFETY: as above.
    unsafe { std::env::remove_var("BUTTERPOLLO_WAKE_FIX") };
    assert_eq!(code, 0, "{output}");
    Ok(())
}
#[test]
fn driver_task_scripts_preserve_unicode_paths_and_literal_arguments() -> Result<()> {
    let root = tempfile::tempdir()?;
    // A user folder with a space, an apostrophe and brackets, as a
    // Windows account name can have them (#80): the script reports the
    // folder it was started from, so the path is proven both ways.
    let expected = "Çağrı Müller's (x86) & literal%TEMP%";
    let folder = root.path().join(expected);
    std::fs::create_dir(&folder)?;
    let driver = folder.join("fake.ps1");
    std::fs::write(
        &driver,
        "param([string]$Text)\r\nWrite-Output $Text\r\nWrite-Output \"ROOT=$PSScriptRoot=\"\r\nexit 3010\r\n",
    )?;
    let log = folder.join("result.log");
    let script = root.path().join("task.cmd");
    std::fs::write(
        &script,
        task_script(
            &system32("WindowsPowerShell\\v1.0\\powershell.exe"),
            &[
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                &driver.display().to_string(),
                expected,
            ],
            &log,
        ),
    )?;
    run(
        &system32("cmd.exe"),
        &["/D", "/C", &script.display().to_string()],
        Duration::from_secs(30),
    )?;
    let output = std::fs::read_to_string(log)?;
    assert!(output.lines().any(|line| line == expected), "{output}");
    // Only the folder's own name is compared: the temp folder above it
    // can be an 8.3 short path here (`RUNNER~1` on a CI runner) while
    // PowerShell reports the long one.
    let reported = format!("\\{expected}=");
    assert!(
        output
            .lines()
            .any(|line| line.starts_with("ROOT=") && line.ends_with(&reported)),
        "{output}"
    );
    assert!(output.contains("BUTTERPOLLO-EXIT=3010"), "{output}");
    Ok(())
}
#[test]
fn setup_tasks_have_no_timer_run_at_normal_priority_and_xml_paths_round_trip() -> Result<()> {
    let root = tempfile::tempdir()?;
    let script = root.path().join("Çağrı & Müller/task.cmd");
    let definition = root.path().join("task.xml");
    std::fs::write(&definition, task_definition(&script))?;
    let check = root.path().join("check.ps1");
    std::fs::write(
        &check,
        r#"param([string]$Definition)
$task = [xml]::new()
$task.Load($Definition)
if ($task.Task.Triggers.HasChildNodes) { exit 1 }
if ($task.Task.Principals.Principal.UserId -ne 'S-1-5-18') { exit 2 }
if ($task.Task.Settings.Priority -ne '4') { exit 3 }
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
Write-Output $task.Task.Actions.Exec.Arguments
"#,
    )?;
    let (code, output) = run(
        &system32("WindowsPowerShell\\v1.0\\powershell.exe"),
        &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            &check.display().to_string(),
            &definition.display().to_string(),
        ],
        Duration::from_secs(30),
    )?;
    assert_eq!(code, 0, "{output}");
    assert!(
        output.contains(&format!("/D /S /C \"\"{}\"\"", script.display())),
        "{output}"
    );
    Ok(())
}
#[test]
fn system_tasks_run_from_folders_with_spaces_and_unicode() -> Result<()> {
    // Creating a SYSTEM task needs an administrator, as setup and CI are.
    if !elevated() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let work = root.path().join("Çağrı & Müller setup");
    let (code, output) = run_as_system_in(
        &work,
        &system32("whoami.exe"),
        &["/user"],
        Duration::from_secs(60),
    )?;
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("S-1-5-18"), "{output}");
    assert_eq!(std::fs::read_dir(&work)?.count(), 0);
    // Below normal, a busy PC starved the driver scripts past setup's limit.
    let (code, output) = run_as_system_in(
        &work,
        &system32("WindowsPowerShell\\v1.0\\powershell.exe"),
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "(Get-Process -Id $PID).PriorityClass",
        ],
        Duration::from_secs(60),
    )?;
    assert_eq!(code, 0, "{output}");
    assert_eq!(output.trim(), "Normal");
    Ok(())
}

#[test]
fn windows_consumers_receive_drive_and_unc_paths_without_verbatim_prefixes() -> Result<()> {
    for (input, expected) in [
        (
            r"\\?\C:\Program Files\Butterpollo\butterpollo.exe",
            r"C:\Program Files\Butterpollo\butterpollo.exe",
        ),
        (
            r"\\?\UNC\server\share\Butterpollo ü\butterpollo.exe",
            r"\\server\share\Butterpollo ü\butterpollo.exe",
        ),
        (
            r"C:\Program Files\Butterpollo",
            r"C:\Program Files\Butterpollo",
        ),
        (r"\\server\share\Butterpollo", r"\\server\share\Butterpollo"),
    ] {
        assert_eq!(win32_path(Path::new(input))?, Path::new(expected));
    }
    for invalid in [
        r"relative\host.exe",
        r"C:host.exe",
        r"\\.\PhysicalDrive0",
        r"\\?\GLOBALROOT\Device\host.exe",
        r"\\?\C:\Folder\..\host.exe",
        r"\\?\C:\Folder\.\host.exe",
        r"\\?\C:\Folder.\host.exe",
        r"\\?\C:\Folder \host.exe",
    ] {
        assert!(win32_path(Path::new(invalid)).is_err(), "{invalid}");
    }
    let root = tempfile::tempdir()?;
    let file = root.path().join("Butterpollo ü.exe");
    std::fs::write(&file, b"fixture")?;
    let canonical = std::fs::canonicalize(&file)?;
    assert_eq!(std::fs::canonicalize(win32_path(&canonical)?)?, canonical);
    Ok(())
}

#[test]
fn firewall_updates_or_creates_without_deleting_existing_rules_on_failure() -> Result<()> {
    for (results, expected_operations, succeeds) in [
        (vec![0], vec!["set"], true),
        (vec![1, 0], vec!["set", "add"], true),
        (vec![1, 1], vec!["set", "add"], false),
    ] {
        let mut results = results.into_iter();
        let mut operations = Vec::new();
        let result = firewall_allow_with(
            "Rubylight",
            Path::new(r"\\?\C:\Program Files\Butterpollo\butterpollo.exe"),
            |args| {
                assert!(args.contains(&r"program=C:\Program Files\Butterpollo\butterpollo.exe"));
                assert!(!args.contains(&"delete"));
                operations.push(args[2].to_owned());
                Ok(results.next().expect("unexpected firewall operation"))
            },
        );
        assert_eq!(result.is_ok(), succeeds);
        assert_eq!(operations, expected_operations);
    }
    firewall_allow_with("Butterpollo", Path::new(r"\\.\PhysicalDrive0"), |_| {
        panic!("invalid paths must fail before touching firewall rules")
    })
    .unwrap_err();
    Ok(())
}

#[test]
fn startup_plan_keeps_what_the_user_set() {
    let keep = SERVICE_START_TYPE(SERVICE_NO_CHANGE);
    assert_eq!(
        startup_plan(None),
        StartupPlan {
            start_type: SERVICE_AUTO_START,
            start: true
        }
    );
    for existing in [SERVICE_AUTO_START, SERVICE_DEMAND_START] {
        assert_eq!(
            startup_plan(Some(existing)),
            StartupPlan {
                start_type: keep,
                start: true
            }
        );
    }
    assert_eq!(
        startup_plan(Some(SERVICE_DISABLED)),
        StartupPlan {
            start_type: keep,
            start: false
        }
    );
}
/// Deletes the throwaway service when the test ends, even if it fails.
struct Throwaway(String);
impl Drop for Throwaway {
    fn drop(&mut self) {
        let _ = delete_service(&self.0);
    }
}
fn set_start_type(name: &str, start_type: SERVICE_START_TYPE) -> Result<()> {
    let service = open_service(name).context("throwaway service is missing")?;
    // SAFETY: `service` is a live handle; null strings and NO_CHANGE leave everything else alone.
    unsafe {
        ChangeServiceConfigW(
            service.0,
            ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
            start_type,
            SERVICE_ERROR(SERVICE_NO_CHANGE),
            PCWSTR::null(),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
        )?;
    }
    Ok(())
}
fn start_type_of(name: &str) -> Result<SERVICE_START_TYPE> {
    service_start_type(&open_service(name).context("throwaway service is missing")?)
}
/// Reconfiguring an installed service (every update) must not turn a
/// Manual or Disabled service back into Automatic.
#[test]
fn reconfiguring_a_service_keeps_its_startup_type() -> Result<()> {
    if !elevated() {
        return Ok(());
    }
    // Only ever a service this test created, under a name of its own.
    let name = format!("RubylightSetupTest{}", std::process::id());
    let program = std::env::current_exe()?;
    let _cleanup = Throwaway(name.clone());
    install_service(&name, "Rubylight setup test", "Throwaway", &program)?;
    assert_eq!(start_type_of(&name)?, SERVICE_AUTO_START);
    for kept in [SERVICE_DEMAND_START, SERVICE_DISABLED] {
        set_start_type(&name, kept)?;
        install_service(&name, "Rubylight setup test", "Throwaway", &program)?;
        assert_eq!(start_type_of(&name)?, kept);
    }
    // A disabled service is left alone instead of failing the start.
    assert!(!start_service_unless_disabled(&name)?);
    assert!(!service_running(&name));
    Ok(())
}
