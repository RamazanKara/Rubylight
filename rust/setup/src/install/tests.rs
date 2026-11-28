//! Tests for installation planning, side-by-side layout and host probing.
mod drivers;
mod profile;
mod staging;

/// 2.4.2 trusted the root only on a fresh install; an update, the way
/// every existing host got 2.4.2, left the console untrusted.
#[test]
fn every_start_of_the_new_host_trusts_the_console_root() {
    let ready = "Some(env!(\"CARGO_PKG_VERSION\")))?;";
    for (name, source) in [
        ("install.rs", include_str!("../install.rs")),
        ("update.rs", include_str!("../update.rs")),
    ] {
        let code = source.split("#[cfg(test)]").next().unwrap();
        let starts: Vec<_> = code.match_indices(ready).collect();
        assert!(!starts.is_empty(), "{name}");
        for (at, _) in starts {
            let after: String = code[at..].lines().skip(1).take(4).collect();
            assert!(after.contains("trust_console"), "{name}: {after}");
        }
    }
}
#[test]
fn setup_removes_the_root_by_the_name_the_host_gives_it() {
    assert_eq!(
        super::CONSOLE_ROOT_NAME,
        butterpollo_crypto::crypto::CONSOLE_ROOT_NAME
    );
}
use super::*;
#[test]
fn vibepollos_nvidia_undo_runs_only_with_its_journal_and_program() {
    let base = std::env::temp_dir().join(format!("nvprefs-undo-{}", std::process::id()));
    let (data, previous) = (base.join("data"), base.join("previous"));
    std::fs::create_dir_all(data.join("Sunshine")).unwrap();
    std::fs::create_dir_all(&previous).unwrap();
    assert!(!nvprefs_undo_pending(&data, &previous));
    std::fs::write(data.join("Sunshine\\nvprefs_undo.json"), "{}").unwrap();
    assert!(!nvprefs_undo_pending(&data, &previous));
    std::fs::write(previous.join("sunshine.exe"), "").unwrap();
    assert!(nvprefs_undo_pending(&data, &previous));
    std::fs::remove_file(data.join("Sunshine\\nvprefs_undo.json")).unwrap();
    assert!(!nvprefs_undo_pending(&data, &previous));
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn nsis_uninstallers_run_in_place_so_setup_waits_for_them() -> Result<()> {
    let product = |uninstall: Option<&str>, quiet: Option<&str>| detect::Product {
        key: "Apollo".into(),
        name: "Apollo".into(),
        version: "0.4.6".into(),
        location: None,
        uninstall: uninstall.map(Into::into),
        quiet_uninstall: quiet.map(Into::into),
        msi: false,
        root: HKEY_LOCAL_MACHINE,
    };
    let apollo = product(Some(r#""C:\Program Files\Apollo\Uninstall.exe""#), None);
    assert_eq!(
        legacy_uninstall(&apollo, |_| true),
        Some((
            r#""C:\Program Files\Apollo\Uninstall.exe" /S _?=C:\Program Files\Apollo"#.into(),
            Some(PathBuf::from(r"C:\Program Files\Apollo\Uninstall.exe"))
        ))
    );
    // Other uninstallers, and a command that already runs in place, are
    // run as registered.
    assert_eq!(
        legacy_uninstall(&apollo, |_| false),
        Some((r#""C:\Program Files\Apollo\Uninstall.exe" /S"#.into(), None))
    );
    let quiet = product(None, Some(r"C:\Apollo\Uninstall.exe /S _?=C:\Apollo"));
    assert_eq!(
        legacy_uninstall(&quiet, |_| true),
        Some((r"C:\Apollo\Uninstall.exe /S _?=C:\Apollo".into(), None))
    );
    assert_eq!(legacy_uninstall(&product(None, None), |_| true), None);

    let folder = tempfile::tempdir()?;
    let nsis = folder.path().join("Uninstall.exe");
    std::fs::write(
        &nsis,
        [&b"MZ"[..], &[0; 4096], b"\xef\xbe\xad\xdeNullsoftInst"].concat(),
    )?;
    let other = folder.path().join("unins000.exe");
    std::fs::write(&other, [&b"MZ"[..], &[0; 4096], b"Inno Setup"].concat())?;
    assert!(is_nsis(&nsis));
    assert!(!is_nsis(&other));
    assert!(!is_nsis(&folder.path().join("missing.exe")));
    Ok(())
}

#[test]
fn replacing_stays_as_it_was_and_side_by_side_touches_only_rubylight() {
    let replacing = takeover(false);
    assert_eq!(replacing.stop, OLD_SERVICES);
    assert_eq!(replacing.end, HOST_PROCESSES);
    assert!(replacing.remove_hosts);
    assert_eq!(
        replacing.firewall_rules,
        ["Butterpollo", "Vibepollo", "Vibepollo Service", "Apollo"]
    );
    let keeping = takeover(true);
    assert_eq!(keeping.stop, [host_service::SIDE_BY_SIDE]);
    assert_eq!(keeping.end, OWN_PROCESSES);
    assert!(!keeping.remove_hosts);
    assert_eq!(keeping.firewall_rules, ["Butterpollo"]);
    for service in OLD_SERVICES {
        assert!(!keeping.stop.contains(&service), "{service}");
    }
    assert_eq!(HOST_PROCESSES[..4], OWN_PROCESSES);
    for other in &HOST_PROCESSES[4..] {
        assert!(!keeping.end.contains(other), "{other}");
    }
}
/// Every removal of a host goes through the plan: its steps stay gated by
/// `remove_hosts`, the import by the side-by-side branch.
#[test]
fn a_full_installation_gates_each_removal_on_the_plan() {
    // A Windows checkout may have CRLF line ends.
    let source = include_str!("../install.rs").replace("\r\n", "\n");
    let code = source
        .split("fn install_with(")
        .nth(1)
        .and_then(|rest| rest.split("/// Uninstall the hosts `found` lists.").next())
        .unwrap();
    for gated in [
        "if plan.remove_hosts {\n        restart_needed |= remove_hosts(",
        "if plan.remove_hosts {\n        // Remaining old services",
        "if plan.remove_hosts {\n        let _ = std::fs::remove_dir_all(",
        "if plan.remove_hosts {\n        crate::uninstall::remove_stale_layers();",
        "for rule in plan.firewall_rules {",
        "system::kill(plan.end);",
        "        plan.stop,\n",
    ] {
        assert!(code.contains(gated), "{gated}");
    }
    for ungated in [
        "        &OLD_SERVICES,\n",
        "system::kill(&HOST_PROCESSES)",
        "for rule in [",
    ] {
        assert!(!code.contains(ungated), "{ungated}");
    }
    assert!(code.contains(
        "let previous = if side_by_side {\n        None\n    } else {\n        migration_source("
    ));
    // Recorded before the service in both: an update heals a lost record,
    // and a new installation's driver steps read it.
    assert_eq!(
        code.matches("detect::record_service(side_by_side)?;")
            .count(),
        2
    );
    // Before anything is stopped or written, on every path but an update
    // in place: a move returns early, and took the folder unchecked.
    let at = |text: &str| code.find(text).unwrap_or_else(|| panic!("{text}"));
    let guard = at("outside_other_hosts(&found, &install)?;");
    assert!(guard < at("if let Target::Move(old) = &target {"));
    assert!(guard > at("let updating = target == Target::InPlace;"));
    assert!(at("refuse_foreign_service(") < at("prepare_profile(&profile"));
}
#[test]
fn side_by_side_stays_out_of_a_folder_a_host_knows_only_by_its_service() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let host = dir.path().join("host");
    std::fs::create_dir(&host)?;
    let found = detect::Found {
        other_service_folders: vec![("ApolloService", host.clone())],
        ..Default::default()
    };
    for inside in [host.clone(), host.join("Rubylight")] {
        let refused = format!("{:#}", outside_other_hosts(&found, &inside).unwrap_err());
        assert!(refused.contains("ApolloService"), "{refused}");
        assert!(refused.contains("Nothing has been changed"), "{refused}");
    }
    outside_other_hosts(&found, &dir.path().join("Rubylight"))?;
    // A service whose folder is gone holds nothing to overwrite.
    let gone = detect::Found {
        other_service_folders: vec![("ApolloService", dir.path().join("gone"))],
        ..Default::default()
    };
    outside_other_hosts(&gone, &dir.path().join("gone"))?;
    Ok(())
}
#[test]
fn another_programs_rubylight_service_is_refused() {
    refuse_foreign_service(None).unwrap();
    refuse_foreign_service(Some(Path::new(
        r"C:\Program Files\Rubylight\butterpollo-service.exe",
    )))
    .unwrap();
    let refused = format!(
        "{:#}",
        refuse_foreign_service(Some(Path::new(r"C:\Other\other.exe"))).unwrap_err()
    );
    assert!(refused.contains("RubylightService"), "{refused}");
    assert!(refused.contains(r"C:\Other\other.exe"), "{refused}");
    assert!(refused.contains("Nothing has been changed"), "{refused}");
}
/// Setup's rollback must not start or stop the other host: it only ever
/// acts on Rubylight's own service, whatever else runs.
#[test]
fn a_failed_side_by_side_setup_leaves_the_other_hosts_services_alone() -> Result<()> {
    let mut stopped = Vec::new();
    let mut started = Vec::new();
    let restart = stop_running(
        takeover(true).stop,
        |_| true,
        |service| {
            stopped.push(service.to_owned());
            Ok(())
        },
        |service: &str| {
            started.push(service.to_owned());
            Ok(())
        },
    )?;
    drop(restart);
    assert_eq!(stopped, ["RubylightService"]);
    assert_eq!(started, ["RubylightService"]);
    Ok(())
}
#[test]
fn the_range_checked_covers_every_port_the_host_derives() {
    use butterpollo_config::config::{Config, Ports};
    for base in SIDE_BY_SIDE_PORTS {
        let range = ports_of(base);
        let ports = Ports::from_base(base);
        for port in [
            ports.http,
            ports.https,
            ports.web,
            ports.guest,
            ports.video,
            ports.control,
            ports.audio,
            ports.mic,
            ports.rtsp,
        ] {
            assert!(range.contains(&port), "{port} for {base}");
        }
        let config = Config::parse(&side_by_side_conf(base, None)).unwrap();
        assert_eq!(config.port().unwrap(), base);
        assert!(range.contains(&config.webrtc_port()));
        // The guests' media ports follow the media port (MAX_GUESTS, 4).
        assert!(range.contains(&(config.webrtc_media_port() + 4)));
        // Clear of the default range other hosts use, and of the ephemeral one.
        assert!(*range.start() > 48010 + 5, "{base}");
        assert!(*range.end() < 49152, "{base}");
    }
    for pair in SIDE_BY_SIDE_PORTS.windows(2) {
        assert!(ports_of(pair[0]).end() < ports_of(pair[1]).start());
    }
}
#[test]
fn the_base_port_needs_its_whole_range_free_with_a_fallback_then_an_error() {
    assert_eq!(pick_base_port(|_| true).unwrap(), SIDE_BY_SIDE_PORTS[0]);
    // Any one port of a range rules out the base.
    for taken in [48584, 48589, 48590, 48610, 48619, 48624] {
        assert_eq!(
            pick_base_port(|port| port != taken).unwrap(),
            SIDE_BY_SIDE_PORTS[1],
            "{taken}"
        );
    }
    let error = format!("{:#}", pick_base_port(|_| false).unwrap_err());
    assert!(error.contains("no free ports"), "{error}");
    assert!(error.contains("Nothing has been changed"), "{error}");
}
#[test]
fn a_port_a_program_holds_is_not_free() -> Result<()> {
    let tcp = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let tcp_port = tcp.local_addr()?.port();
    assert!(side_by_side::table_holds_port(tcp_port));
    assert!(!port_free(tcp_port));
    let udp = std::net::UdpSocket::bind(("127.0.0.1", 0))?;
    let udp_port = udp.local_addr()?.port();
    assert!(side_by_side::table_holds_port(udp_port));
    assert!(!port_free(udp_port));
    if let Ok(tcp6) = std::net::TcpListener::bind(("::1", 0)) {
        let port = tcp6.local_addr()?.port();
        assert!(side_by_side::table_holds_port(port));
        assert!(!port_free(port));
    }
    if let Ok(udp6) = std::net::UdpSocket::bind(("::1", 0)) {
        let port = udp6.local_addr()?.port();
        assert!(side_by_side::table_holds_port(port));
        assert!(!port_free(port));
    }
    // A port just let go can be taken at once by another test or program (CI flaked on it),
    // so a free one is looked for among a few released ones.
    let mut free = false;
    for _ in 0..20 {
        let port = {
            let probe = std::net::TcpListener::bind(("127.0.0.1", 0))?;
            probe.local_addr()?.port()
        };
        if port_free(port) {
            free = true;
            break;
        }
    }
    assert!(free, "no released port was free");
    Ok(())
}
#[test]
fn mib_table_port_matcher_handles_network_order_upper_bits_and_ipv4_ipv6() {
    use windows::Win32::NetworkManagement::IpHelper::{
        MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_UDPROW, MIB_UDPTABLE,
    };
    let raw_48589 = u32::from(48589u16.to_be()) | 0xDEAD_0000;
    assert_eq!(side_by_side::mib_port(raw_48589), 48589);
    assert_eq!(side_by_side::mib_port(u32::from(80u16.to_be())), 80);

    // IPv4 UDP table: 2 entries, second holds 48589 with dirty upper bits.
    let v4_rows = [
        MIB_UDPROW {
            dwLocalAddr: 0,
            dwLocalPort: u32::from(47989u16.to_be()),
        },
        MIB_UDPROW {
            dwLocalAddr: 0x0100_007F,
            dwLocalPort: raw_48589,
        },
    ];
    let mut v4_buf = vec![2u32];
    for row in v4_rows {
        v4_buf.push(row.dwLocalAddr);
        v4_buf.push(row.dwLocalPort);
    }
    assert!(side_by_side::table_bytes_hold_port::<
        MIB_UDPTABLE,
        MIB_UDPROW,
    >(
        &v4_buf,
        std::mem::offset_of!(MIB_UDPTABLE, table),
        |r| r.dwLocalPort,
        48589,
    ));
    assert!(!side_by_side::table_bytes_hold_port::<
        MIB_UDPTABLE,
        MIB_UDPROW,
    >(
        &v4_buf,
        std::mem::offset_of!(MIB_UDPTABLE, table),
        |r| r.dwLocalPort,
        48590,
    ));
    // Truncated buffer claiming more rows than bytes must fail closed.
    let truncated = vec![5u32, 0, raw_48589];
    assert!(!side_by_side::table_bytes_hold_port::<
        MIB_UDPTABLE,
        MIB_UDPROW,
    >(
        &truncated,
        std::mem::offset_of!(MIB_UDPTABLE, table),
        |r| r.dwLocalPort,
        48589,
    ));

    // IPv6 TCP table: 1 entry holding 48619 with dirty upper bits.
    let v6_row = MIB_TCP6ROW_OWNER_PID {
        dwLocalPort: u32::from(48619u16.to_be()) | 0xBEEF_0000,
        ..Default::default()
    };
    let row_words = std::mem::size_of::<MIB_TCP6ROW_OWNER_PID>() / 4;
    let mut v6_buf = vec![0u32; 1 + row_words];
    v6_buf[0] = 1;
    // SAFETY: `v6_row` is `Copy` and `v6_buf[1..]` has exact size `size_of::<MIB_TCP6ROW_OWNER_PID>()`.
    unsafe {
        std::ptr::copy_nonoverlapping(
            (&raw const v6_row).cast::<u8>(),
            v6_buf.as_mut_ptr().add(1).cast::<u8>(),
            std::mem::size_of::<MIB_TCP6ROW_OWNER_PID>(),
        );
    }
    assert!(side_by_side::table_bytes_hold_port::<
        MIB_TCP6TABLE_OWNER_PID,
        MIB_TCP6ROW_OWNER_PID,
    >(
        &v6_buf,
        std::mem::offset_of!(MIB_TCP6TABLE_OWNER_PID, table),
        |r| r.dwLocalPort,
        48619,
    ));
    assert!(!side_by_side::table_bytes_hold_port::<
        MIB_TCP6TABLE_OWNER_PID,
        MIB_TCP6ROW_OWNER_PID,
    >(
        &v6_buf,
        std::mem::offset_of!(MIB_TCP6TABLE_OWNER_PID, table),
        |r| r.dwLocalPort,
        48620,
    ));
}
/// Two hosts with one PC's name would collide on mDNS and look the same
/// in a client's list.
#[test]
fn the_new_profile_has_its_own_ports_and_name() {
    use butterpollo_config::config::Config;
    let conf = side_by_side_conf(48589, Some("GAMING-PC"));
    let config = Config::parse(&conf).unwrap();
    assert_eq!(config.get("sunshine_name", ""), "GAMING-PC (Rubylight)");
    assert_eq!(config.webrtc_port(), 48619);
    assert_eq!(config.webrtc_media_port(), 48620);
    let unnamed = Config::parse(&side_by_side_conf(48589, Some(" "))).unwrap();
    assert_eq!(unnamed.get("sunshine_name", ""), "Rubylight");
}
#[test]
fn a_side_by_side_profile_is_new_and_never_another_hosts() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let profile = temp.path().join("config");
    let mut moved = None;
    let free = |_: u16| true;
    // Nothing there: a new profile on the first free base port.
    assert!(prepare_side_by_side_profile(
        &profile,
        false,
        Some("PC"),
        free,
        &mut moved
    )?);
    assert_eq!(web_port(&profile), 48590);
    assert_eq!(probe(&profile).port(), 48589);
    assert!(moved.is_none());
    // Left by an earlier side-by-side installation: kept, settings and all.
    std::fs::write(profile.join("sunshine_state.json"), b"paired")?;
    assert!(!prepare_side_by_side_profile(
        &profile,
        false,
        Some("PC"),
        free,
        &mut moved
    )?);
    assert_eq!(
        std::fs::read(profile.join("sunshine_state.json"))?,
        b"paired"
    );
    // Its port is taken now: said, not changed.
    let error = prepare_side_by_side_profile(
        &profile,
        false,
        Some("PC"),
        |port| port != 48589,
        &mut moved,
    )
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("base port 48589"),
        "{error:#}"
    );
    // Left by a normal installation, whose identity may be imported from
    // another host: moved aside unchanged, and the new one starts fresh.
    std::fs::remove_file(profile.join(SIDE_BY_SIDE_MARK))?;
    std::fs::write(profile.join("sunshine.conf"), "port = 47989\n")?;
    assert!(prepare_side_by_side_profile(
        &profile,
        false,
        Some("PC"),
        free,
        &mut moved
    )?);
    assert_eq!(web_port(&profile), 48590);
    assert!(!profile.join("sunshine_state.json").exists());
    let aside = moved.take().context("the earlier profile moved aside")?;
    assert!(aside.parent() == profile.parent());
    assert_eq!(std::fs::read(aside.join("sunshine_state.json"))?, b"paired");
    assert_eq!(
        std::fs::read_to_string(aside.join("sunshine.conf"))?,
        "port = 47989\n"
    );
    assert!(earlier_settings_moved(&aside).contains(&aside.display().to_string()));
    // No free ports: refused before the earlier settings moved.
    std::fs::remove_file(profile.join(SIDE_BY_SIDE_MARK))?;
    assert!(prepare_side_by_side_profile(&profile, false, None, |_| false, &mut moved).is_err());
    assert!(moved.is_none());
    assert!(profile.join("sunshine.conf").is_file());
    // An installed side-by-side Rubylight keeps its profile as it is.
    std::fs::write(profile.join("sunshine.conf"), "port = 50000\n")?;
    assert!(!prepare_side_by_side_profile(
        &profile,
        true,
        None,
        |_| false,
        &mut moved
    )?);
    assert_eq!(web_port(&profile), 50001);
    Ok(())
}
/// A failed first side-by-side setup leaves no profile to take for
/// Rubylight's own on the next try.
#[test]
fn a_failed_first_install_removes_the_profile_it_made() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let profile = temp.path().join("config");
    let new = prepare_side_by_side_profile(&profile, false, None, |_| true, &mut None)?;
    assert!(new);
    let failed = undo_import_on_error(&profile, new, || bail!("no space on the disk"));
    assert!(failed.is_err());
    assert!(!profile.exists());
    Ok(())
}
#[test]
fn the_result_gives_the_address_with_its_port() {
    let text = side_by_side_result(48590, "192.168.4.12", &["Vibepollo".to_owned()]);
    assert!(text.contains("beside Vibepollo"), "{text}");
    assert!(text.contains("RubylightService"), "{text}");
    assert!(text.contains("add this PC as 192.168.4.12:48589"), "{text}");
    let alone = side_by_side_result(48590, "PC", &[]);
    assert!(alone.contains("Rubylight runs as the"), "{alone}");
}

#[test]
fn setup_refuses_streams_pending_connections_and_apps() {
    for counts in [[0, 0, 0], [1, 0, 0], [0, 1, 0], [0, 0, 1]] {
        let response = format!(
            "<RustHostSessionCount>{}</RustHostSessionCount><RustHostPendingSessionCount>{}</RustHostPendingSessionCount><RustHostApplicationActive>{}</RustHostApplicationActive>",
            counts[0], counts[1], counts[2]
        );
        assert_eq!(check_idle(&response).is_ok(), counts == [0, 0, 0]);
    }
    // Older hosts, and a Rust host asked on its LAN bind_address, cannot
    // say; they are stopped as before rather than blocking setup forever.
    assert!(check_idle("<state>SUNSHINE_SERVER_FREE</state>").is_ok());
    assert!(check_idle("<RustHostVersion>2.0.0-rc.21</RustHostVersion><RustHostSessionCount></RustHostSessionCount><RustHostPendingSessionCount></RustHostPendingSessionCount><RustHostApplicationActive></RustHostApplicationActive>").is_ok());
    assert!(check_idle("").is_ok());
}
#[test]
fn a_stopped_or_unresponsive_host_does_not_block_setup() -> Result<()> {
    // Windows refuses a closed loopback port only after about two seconds.
    let closed = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?;
    ensure_idle(closed, false)?;
    let silent = std::net::TcpListener::bind("127.0.0.1:0")?;
    ensure_idle(silent.local_addr()?, false)?;
    // A busy host is still refused over a real connection, unless the
    // user chose to end its stream.
    let busy = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = busy.local_addr()?;
    let server = std::thread::spawn(move || -> std::io::Result<()> {
        use std::io::{Read, Write};
        for _ in 0..2 {
            let (mut socket, _) = busy.accept()?;
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte)?;
                request.push(byte[0]);
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\n\r\n<root><RustHostSessionCount>1</RustHostSessionCount></root>")?;
        }
        Ok(())
    });
    assert!(ensure_idle(address, false).is_err());
    ensure_idle(address, true)?;
    server.join().unwrap()?;
    Ok(())
}

#[test]
fn the_health_check_asks_the_configured_bind_address() {
    let local = IpAddr::from([127, 0, 0, 1]);
    for (conf, expected) in [
        ("", local),
        ("port = 48000\n", local),
        ("bind_address =\n", local),
        ("bind_address = 0.0.0.0\n", local),
        ("bind_address = ::\n", local),
        ("bind_address = Ethernet\n", local),
        ("# bind_address = 192.168.1.50\n", local),
        (
            "bind_address = 192.168.1.50\n",
            IpAddr::from([192, 168, 1, 50]),
        ),
        (
            "bind_address = \"192.168.1.50\" # LAN only\r\n",
            IpAddr::from([192, 168, 1, 50]),
        ),
        (
            "bind_address =\nbind_address = 10.0.0.2\nbind_address = 10.0.0.3\n",
            IpAddr::from([10, 0, 0, 2]),
        ),
        ("bind_address = fd00::5\n", "fd00::5".parse().unwrap()),
    ] {
        assert_eq!(probe_address(conf), expected, "{conf:?}");
    }
    let profile = tempfile::tempdir().unwrap();
    let mut utf16 = vec![0xff, 0xfe];
    utf16.extend(
        "port = 48000\r\nbind_address = 192.168.1.50\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes),
    );
    std::fs::write(profile.path().join("sunshine.conf"), utf16).unwrap();
    assert_eq!(
        probe(profile.path()),
        SocketAddr::from(([192, 168, 1, 50], 48000))
    );
    std::fs::write(
        profile.path().join("sunshine.conf"),
        b"\xef\xbb\xbfport = 48000\n",
    )
    .unwrap();
    assert_eq!(
        probe(profile.path()),
        SocketAddr::from(([127, 0, 0, 1], 48000))
    );
    // The probe reaches a host listening only on that address.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let host = std::thread::spawn(move || {
        use std::io::{Read, Write};
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0; 512];
        let length = socket.read(&mut request).unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\n\r\n<RustHostVersion>9.9</RustHostVersion>")
            .unwrap();
        String::from_utf8_lossy(&request[..length]).into_owned()
    });
    wait_ready(address, Some("9.9")).unwrap();
    assert!(host.join().unwrap().contains("Host: 127.0.0.1\r\n"));
}

#[test]
fn hkcu_uninstall_commands_are_never_run_elevated() {
    use windows::Win32::System::Registry::HKEY_CURRENT_USER;
    let hkcu = detect::Product {
        key: "Sunshine".into(),
        name: "Sunshine".into(),
        version: "0.23.0".into(),
        location: None,
        uninstall: Some(r"C:\Users\alice\payload.exe".into()),
        quiet_uninstall: Some(r"C:\Users\alice\payload.exe /S".into()),
        msi: false,
        root: HKEY_CURRENT_USER,
    };
    assert_eq!(legacy_uninstall(&hkcu, |_| true), None);
    let error = remove_legacy(&hkcu).unwrap_err();
    assert!(error.to_string().contains("elevated"), "{error:#}");
}
#[test]
fn side_by_side_profile_prep_runs_after_idle_check_and_restores_earlier_profile_on_error()
-> Result<()> {
    let temp = tempfile::tempdir()?;
    let profile = temp.path().join("config");
    std::fs::create_dir_all(&profile)?;
    std::fs::write(profile.join("sunshine.conf"), b"port = 47989\n")?;
    let mut earlier = None;
    let err = undo_profile_on_error(&profile, false, &mut earlier, |created, earlier| {
        *created |= prepare_side_by_side_profile(&profile, false, Some("HOST"), |_| true, earlier)?;
        assert!(earlier.is_some());
        assert!(profile.join(SIDE_BY_SIDE_MARK).is_file());
        anyhow::bail!("simulated copy_package failure")
    })
    .unwrap_err();
    assert!(err.to_string().contains("simulated copy_package failure"));
    assert!(
        earlier.is_none(),
        "restored earlier profile must clear earlier note"
    );
    assert_eq!(
        std::fs::read(profile.join("sunshine.conf"))?,
        b"port = 47989\n"
    );
    assert!(!profile.join(SIDE_BY_SIDE_MARK).exists());

    // Source order check: side-by-side profile mutation happens only after idle/stop/recover
    // inside undo_profile_on_error, and stale PendingFileRenameOperations are pruned before writes.
    let source = include_str!("../install.rs").replace("\r\n", "\n");
    let code = source
        .split("fn install_with(")
        .nth(1)
        .and_then(|rest| rest.split("/// Uninstall the hosts `found` lists.").next())
        .unwrap();
    let fresh = code
        .split("if let Target::Move(old) = &target")
        .nth(1)
        .unwrap();
    let at = |needle: &str| fresh.find(needle).unwrap_or_else(|| panic!("{needle}"));
    assert!(at("ensure_idle(") < at("prepare_side_by_side_profile("));
    assert!(at("stop_running(") < at("prepare_side_by_side_profile("));
    assert!(at("system::kill(plan.end);") < at("prepare_side_by_side_profile("));
    assert!(at("crate::update::recover(&profile)?;") < at("prepare_side_by_side_profile("));
    assert!(at("undo_profile_on_error(") < at("prepare_side_by_side_profile("));
    assert!(at("system::prune_pending_renames(&install);") < at("copy_package("));
    Ok(())
}
