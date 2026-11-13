use super::*;

#[test]
fn a_wall_clock_time_moves_by_this_pcs_offset() {
    let wall = 1_788_025_796;
    let utc = local_to_utc(wall).unwrap();
    // Whatever this PC's zone, the shift is whole quarter hours within a day.
    assert!((wall - utc).abs() <= 14 * 3600);
    assert_eq!((wall - utc) % 900, 0);
}
use std::os::windows::io::FromRawHandle;
use windows::{
    Win32::{
        Storage::FileSystem::PIPE_ACCESS_DUPLEX,
        System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, PIPE_NOWAIT,
            PIPE_TYPE_BYTE,
        },
    },
    core::PCWSTR,
};

fn server(name: &str) -> File {
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: `name` is NUL-terminated and outlives the call; null attributes mean default
    // security.
    let pipe = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_NOWAIT,
            1,
            4096,
            4096,
            0,
            None,
        )
    };
    assert!(!pipe.is_invalid());
    // SAFETY: `pipe` was checked valid and nothing else owns it, so the File takes sole
    // ownership.
    let file = unsafe { File::from_raw_handle(pipe.0) };
    // SAFETY: `file` owns the pipe handle, and a null OVERLAPPED is allowed for this
    // synchronous call on a PIPE_NOWAIT pipe.
    unsafe {
        let _ = ConnectNamedPipe(handle(&file), None);
    }
    file
}
fn accept(file: &File) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut pid = 0;
        // SAFETY: `file` keeps the pipe open and `pid` outlives the call.
        if unsafe { GetNamedPipeClientProcessId(handle(file), &mut pid) }.is_ok() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "fake pipe client did not connect"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn read_line(file: &mut File) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut line = Vec::new();
    loop {
        if available(file).unwrap() > 0 {
            let mut byte = [0];
            file.read_exact(&mut byte).unwrap();
            if byte[0] == b'\n' {
                return String::from_utf8(line).unwrap();
            }
            line.push(byte[0]);
        } else {
            assert!(Instant::now() < deadline, "fake pipe received no line");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn pipe_handshake_and_fragmented_unicode_status_match_the_packaged_plugin() {
    let temp = tempfile::tempdir().unwrap();
    let name = format!(
        "BP-{}-{}",
        std::process::id(),
        temp.path().file_name().unwrap().to_string_lossy()
    );
    let control_name = format!(r"\\.\pipe\{name}-control");
    let mut control = server(&control_name);
    let mut data = server(&format!(r"\\.\pipe\{name}"));
    let thread = std::thread::spawn(move || {
        accept(&control);
        let mut handshake = [0u8; 80];
        for (i, unit) in name.encode_utf16().enumerate() {
            handshake[i * 2..i * 2 + 2].copy_from_slice(&unit.to_le_bytes());
        }
        control.write_all(&handshake).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while available(&control).unwrap() == 0 {
            assert!(Instant::now() < deadline, "fake pipe received no ACK");
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut ack = [0];
        control.read_exact(&mut ack).unwrap();
        assert_eq!(ack, [2]);
        accept(&data);
        let hello: serde_json::Value = serde_json::from_str(&read_line(&mut data)).unwrap();
        assert_eq!(hello["role"], "launcher");
        let line = "\u{feff}{\"type\":\"status\",\"status\":{\"name\":\"gameStarted\",\"id\":\"game\",\"installDir\":\"C:/Çağrı/Oyun\"}}\r\n";
        data.write_all(&line.as_bytes()[..2]).unwrap();
        std::thread::sleep(Duration::from_millis(60));
        data.write_all(&line.as_bytes()[2..]).unwrap();
        assert_eq!(read_line(&mut data), "{\"done\":true}");
    });
    let pipe = Pipe::connect_to(
        &control_name,
        &serde_json::json!({"type":"hello","role":"launcher","pid":std::process::id()}),
    )
    .unwrap();
    let message = pipe.lines.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(
        matches!(butterpollo_core::playnite::parse(&message), butterpollo_core::playnite::Message::Status { install_dir, .. } if install_dir == "C:/Çağrı/Oyun")
    );
    pipe.send(&serde_json::json!({"done":true})).unwrap();
    thread.join().unwrap();
}

#[test]
fn pipe_server_cannot_impersonate_the_host() {
    use std::os::windows::io::OwnedHandle;
    use windows::Win32::{
        Security::{
            GetTokenInformation, RevertToSelf, SECURITY_IMPERSONATION_LEVEL,
            SecurityIdentification, TOKEN_QUERY, TokenImpersonationLevel,
        },
        System::{
            Pipes::ImpersonateNamedPipeClient,
            Threading::{GetCurrentThread, OpenThreadToken},
        },
    };

    let temp = tempfile::tempdir().unwrap();
    let name = format!(
        r"\\.\pipe\BP-{}-{}",
        std::process::id(),
        temp.path().file_name().unwrap().to_string_lossy()
    );
    let mut server = server(&name);
    let mut client = open(&name, Duration::from_secs(1)).unwrap();
    check_session(&client, session(std::process::id()).unwrap()).unwrap();
    client.write_all(&[1]).unwrap();
    server.read_exact(&mut [0]).unwrap();
    let mut level = SECURITY_IMPERSONATION_LEVEL::default();
    // SAFETY: `server` is a connected pipe that has read client data, as impersonation
    // requires; `token` is owned once opened and `level` outlives GetTokenInformation.
    unsafe {
        ImpersonateNamedPipeClient(handle(&server)).unwrap();
        let mut token = HANDLE::default();
        let opened = OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut token);
        RevertToSelf().unwrap();
        opened.unwrap();
        let token = OwnedHandle::from_raw_handle(token.0);
        GetTokenInformation(
            HANDLE(token.as_raw_handle()),
            TokenImpersonationLevel,
            Some((&mut level as *mut SECURITY_IMPERSONATION_LEVEL).cast()),
            size_of::<SECURITY_IMPERSONATION_LEVEL>() as u32,
            &mut 0,
        )
        .unwrap();
    }
    assert_eq!(level, SecurityIdentification);
}

#[test]
fn pipe_rejects_a_server_outside_the_expected_windows_session() {
    let temp = tempfile::tempdir().unwrap();
    let name = format!(
        r"\\.\pipe\BP-{}-{}",
        std::process::id(),
        temp.path().file_name().unwrap().to_string_lossy()
    );
    let _server = server(&name);
    let client = open(&name, Duration::from_secs(1)).unwrap();
    let current = session(std::process::id()).unwrap();
    check_session(&client, current).unwrap();
    let error = check_session(&client, current + 1).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Playnite plugin pipe belongs to Windows session")
    );
}

#[test]
fn running_ignores_other_sessions_and_unreadable_processes() {
    let processes = (1..=3)
        .map(|pid| butterpollo_core::steam::Process {
            pid,
            parent: 0,
            started: 1,
            name: PROCESSES[1].into(),
        })
        .collect();
    let found = find_running(
        processes,
        2,
        |pid| Some(if pid == 1 { 1 } else { 2 }),
        |pid| (pid != 2).then(|| "C:/Çağrı/Playnite.FullscreenApp.exe".into()),
    );
    assert_eq!(
        found,
        Some((3, PathBuf::from("C:/Çağrı/Playnite.FullscreenApp.exe")))
    );
}

#[test]
fn discovery_skips_stale_paths_and_finds_either_mode_with_unicode() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("Çağrı & Oyunlar");
    std::fs::create_dir(&dir).unwrap();
    let fullscreen = dir.join(PROCESSES[1]);
    std::fs::write(&fullscreen, []).unwrap();
    assert_eq!(
        find_install([temp.path().join("missing"), dir.clone()]),
        Some(dir.clone())
    );
    assert_eq!(executable(&dir), Some(fullscreen));
    std::fs::write(dir.join(PROCESSES[0]), []).unwrap();
    assert_eq!(executable(&dir), Some(dir.join(PROCESSES[0])));
}

#[test]
fn extensions_follow_playnites_portable_detection() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    let roaming = dir.join("Çağrı");
    assert_eq!(
        extensions_at(dir, Some(&roaming)),
        Some(dir.join("Extensions"))
    );
    std::fs::write(dir.join("unins000.exe"), []).unwrap();
    assert_eq!(
        extensions_at(dir, Some(&roaming)),
        Some(roaming.join("Playnite/Extensions"))
    );
    assert_eq!(extensions_at(dir, None), None);
}

#[test]
fn missing_executable_returns_the_actual_start_failure() {
    let temp = tempfile::tempdir().unwrap();
    let exe = temp.path().join("Çağrı Playnite.exe");
    let error = launch(&exe, &[], &Default::default()).unwrap_err();
    assert!(format!("{error:#}").contains("Çağrı Playnite.exe"));
}
