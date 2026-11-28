//! Unit tests for desktop away-wake requester path selection, progress/outcome mapping,
//! cancel propagation, `wake-relay-keys.json` persistence, and secret redaction.

use super::*;
use nightjar_core::http::native_api::wake_relay::{RelayCredential, WakeRelayBundle};
use rubylight_traverse::wake_relay::{Status, WakeStatus};

fn sample_bundle(relays: usize) -> WakeRelayBundle {
    let mut list = Vec::new();
    for idx in 0..relays {
        let ch = char::from(b'a' + (idx as u8 % 26));
        list.push(RelayCredential {
            id: format!("relay-{idx}"),
            name: if idx == 0 {
                "Living Room NUC".into()
            } else {
                format!("Relay {idx}")
            },
            secret: ch.to_string().repeat(64),
        });
    }
    WakeRelayBundle {
        target_id: "PC-UID".into(),
        epoch: 4,
        device: "f".repeat(64),
        relays: list,
    }
}

#[test]
fn choose_wake_path_distinguishes_lan_relay_and_no_relay() {
    let bundle = sample_bundle(1);
    let kept = KeptEntry::Relays(bundle.clone());
    assert_eq!(choose_wake_path(false, false, Some(&kept)), WakePath::Lan);
    assert_eq!(choose_wake_path(true, true, Some(&kept)), WakePath::Lan);
    assert_eq!(choose_wake_path(true, false, None), WakePath::Lan);
    assert_eq!(
        choose_wake_path(true, false, Some(&kept)),
        WakePath::Relay(Box::new(bundle))
    );
    assert_eq!(
        choose_wake_path(true, false, Some(&KeptEntry::NoRelay)),
        WakePath::NoRelay
    );
    let empty_relays = KeptEntry::Relays(sample_bundle(0));
    assert_eq!(
        choose_wake_path(true, false, Some(&empty_relays)),
        WakePath::NoRelay
    );
}

#[test]
fn subnet_matching_uses_addresses_and_valid_broadcast_masks() {
    let own = vec![Subnet {
        address: u32::from(Ipv4Addr::new(192, 168, 4, 15)),
        prefix: 24,
    }];
    assert!(on_pc_network_in(&own, &["192.168.4.42".into()], &[]));
    assert!(!on_pc_network_in(&own, &["10.0.0.42".into()], &[]));
    assert!(on_pc_network_in(&own, &[], &["192.168.4.255".into()]));
    assert!(!on_pc_network_in(&own, &[], &["10.0.0.255".into()]));

    // /31 or /32 does not claim a subnet broadcast match; prefix out of 1..=32 never matches.
    let p32 = vec![Subnet {
        address: u32::from(Ipv4Addr::new(192, 168, 4, 15)),
        prefix: 32,
    }];
    assert!(!on_pc_network_in(&p32, &[], &["192.168.4.15".into()]));
    let bad = vec![Subnet {
        address: u32::from(Ipv4Addr::new(192, 168, 4, 15)),
        prefix: 0,
    }];
    assert!(!on_pc_network_in(&bad, &["192.168.4.15".into()], &[]));
    assert_eq!(
        prefix_from_netmask(Ipv4Addr::new(255, 255, 255, 0)),
        Some(24)
    );
    assert_eq!(prefix_from_netmask(Ipv4Addr::new(255, 0, 255, 0)), None);
}

#[test]
fn progress_and_outcome_messages_and_retry_flags_match_design() {
    assert_eq!(
        progress_message("NUC", "Gaming PC", &Progress::LookingUp).as_deref(),
        Some("Contacting your wake relay (NUC)…")
    );
    assert_eq!(
        progress_message("NUC", "Gaming PC", &Progress::Punching).as_deref(),
        Some("Contacting your wake relay (NUC)…")
    );
    assert_eq!(
        progress_message("NUC", "Gaming PC", &Progress::Requested).as_deref(),
        Some("Asking your wake relay (NUC) to wake Gaming PC…")
    );
    assert_eq!(
        progress_message(
            "NUC",
            "Gaming PC",
            &Progress::Status(WakeStatus::new(Status::PacketSent, [0; 16]))
        )
        .as_deref(),
        Some("Wake signal sent. Waiting for Gaming PC to start…")
    );
    assert_eq!(
        progress_message(
            "NUC",
            "Gaming PC",
            &Progress::Status(WakeStatus::new(Status::HostAwake, [0; 16]))
        ),
        None
    );

    assert!(Outcome::SentNoAnswer.offers_retry());
    assert!(Outcome::TimedOut.offers_retry());
    assert!(Outcome::RelayOffline.offers_retry());
    assert!(Outcome::RelayUnreachable.offers_retry());
    assert!(Outcome::Network("err".into()).offers_retry());
    assert!(!Outcome::Awake.offers_retry());
    assert!(!Outcome::Refused.offers_retry());
    assert!(!Outcome::RateLimited(12).offers_retry());
    assert!(!Outcome::NoRelay.offers_retry());
    assert!(!Outcome::Cancelled.offers_retry());

    assert_eq!(outcome_message(&Outcome::Cancelled, "Gaming PC", &[]), None);
    let limited = outcome_message(&Outcome::RateLimited(0), "Gaming PC", &[]).unwrap();
    assert!(limited.contains("1 s"), "{limited}");
}

#[test]
fn request_through_relays_picks_most_telling_miss_and_respects_cancel() {
    let bundle = sample_bundle(3);
    let cancel = AtomicBool::new(false);
    let mut progress = Vec::new();
    let mut seen = Vec::new();
    let (outcome, relays) = request_through_relays(
        &bundle,
        "Gaming PC",
        BIND,
        &[],
        &[],
        &cancel,
        &mut |line| progress.push(line),
        &mut |cfg, _, on_step| {
            seen.push(cfg.secret);
            on_step(Progress::LookingUp);
            match seen.len() {
                1 => Err(Failure::RelayOffline),
                2 => Ok(WakeStatus::new(Status::TargetNotOnLan, [0; 16])),
                _ => Err(Failure::RelayUnreachable),
            }
        },
    );
    assert_eq!(outcome, Outcome::NotOnPcNetwork);
    assert_eq!(relays, vec!["Relay 1"]);
    assert_eq!(progress.len(), 3);

    // Pre-cancelled request stops before dialing any relay.
    cancel.store(true, Ordering::Release);
    let mut dialed = false;
    let (cancelled, names) = request_through_relays(
        &bundle,
        "Gaming PC",
        BIND,
        &[],
        &[],
        &cancel,
        &mut |_| {},
        &mut |_, _, _| {
            dialed = true;
            Err(Failure::RelayOffline)
        },
    );
    assert_eq!(cancelled, Outcome::Cancelled);
    assert!(names.is_empty());
    assert!(!dialed);
}

#[test]
fn keys_store_persists_atomically_redacts_secrets_and_protects_corrupt_files() {
    let dir = std::env::temp_dir().join(format!(
        "nightjar-wake-relay-keys-test-{}-{}",
        std::process::id(),
        nightjar_core::wake_relay::driver::unix_now()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let bundle = sample_bundle(1);
    let device_secret = bundle.device.clone();
    let relay_secret = bundle.relays[0].secret.clone();

    let mut store = KeysStore::open(&dir);
    assert_eq!(store.get("PC-UID"), None);
    store.apply("PC-UID", WakeRelaySync::Offered(None));
    assert!(matches!(store.get("PC-UID"), Some(KeptEntry::NoRelay)));

    store.apply("PC-UID", WakeRelaySync::Offered(Some(bundle.clone())));
    assert!(matches!(store.get("PC-UID"), Some(KeptEntry::Relays(_))));

    // Secrets registered in redactor never appear verbatim in scrubbed logs.
    let scrubbed =
        nightjar_core::report::redact::scrub(&format!("dev={device_secret} key={relay_secret}"));
    assert!(!scrubbed.contains(&device_secret), "{scrubbed}");
    assert!(!scrubbed.contains(&relay_secret), "{scrubbed}");

    // OS intent: POSIX owner-only 0600 check; Windows uses user-profile ACLs.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join(KEYS_FILE_NAME))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // Reopening from disk reloads the bundle and keeps it until forgotten.
    let mut reopened = KeysStore::open(&dir);
    assert_eq!(reopened.get("PC-UID"), Some(KeptEntry::Relays(bundle)));
    reopened.apply("PC-UID", WakeRelaySync::NotOffered);
    assert_eq!(reopened.get("PC-UID"), None);

    // A corrupt file on disk is left untouched rather than overwritten.
    let path = dir.join(KEYS_FILE_NAME);
    std::fs::write(&path, b"{not valid json").unwrap();
    let mut broken = KeysStore::open(&dir);
    broken.apply("PC-UID", WakeRelaySync::Offered(None));
    assert_eq!(std::fs::read(&path).unwrap(), b"{not valid json");

    let _ = std::fs::remove_dir_all(&dir);
}
