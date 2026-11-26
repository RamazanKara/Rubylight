//! Tests for usbipd-win forwarding against a fake usbipd and a loopback (`127.0.0.1:0`) USB/IP
//! server: the import's checks, and that only what Nightjar shared is ever unshared, journaled
//! before the change and recovered after a crash.

use std::net::TcpListener;
use std::sync::Mutex;

use super::*;

const DUALSENSE: &str = r"USB\VID_054C&PID_0CE6\5&1A2B3C4D&0&3";
const STICK: &str = r"USB\VID_0F0D&PID_0092\6&2&0&1";
const GUID: &str = "2f1e6a64-9b3c-4f0e-8d7a-0123456789ab";
const OTHER_GUID: &str = "11111111-2222-3333-4444-555555555555";

fn listed(instance_id: &str, busid: Option<&str>, guid: Option<&str>) -> StateDevice {
    StateDevice { bus_id: busid.map(str::to_owned), instance_id: instance_id.to_owned(), persisted_guid: guid.map(str::to_owned), ..Default::default() }
}

/// usbipd-win as the tests want it: its list, what ran, and binds that fail when asked to.
#[derive(Default)]
struct Fake {
    state: Mutex<Vec<StateDevice>>,
    calls: Mutex<Vec<String>>,
    fail_bind: bool,
    fail_unbind: bool,
    /// The journal, to check it lists the device before the bind runs.
    journal: Option<Journal>,
}

impl Fake {
    fn with(state: Vec<StateDevice>) -> Self {
        Self { state: Mutex::new(state), ..Default::default() }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl Usbipd for Fake {
    fn state(&self) -> Result<Vec<StateDevice>, String> {
        Ok(self.state.lock().unwrap().clone())
    }

    fn bind(&self, busid: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("bind {busid}"));
        if let Some(journal) = &self.journal {
            let entries = journal.entries().unwrap();
            assert!(entries.iter().any(|e| e.busid == busid && e.guid.is_none()), "journaled before the bind: {entries:?}");
        }
        if self.fail_bind { return Err("cancelled".into()); }
        for device in self.state.lock().unwrap().iter_mut().filter(|d| d.bus_id.as_deref() == Some(busid)) {
            device.persisted_guid = Some(GUID.into());
        }
        Ok(())
    }

    fn unbind(&self, guid: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("unbind {guid}"));
        if self.fail_unbind { return Err("cancelled".into()); }
        for device in self.state.lock().unwrap().iter_mut().filter(|d| d.persisted_guid.as_deref() == Some(guid)) {
            device.persisted_guid = None;
        }
        Ok(())
    }
}

/// A journal in a fresh temporary directory, removed with it.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("nightjar-usbipd-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn journal(&self) -> Journal {
        Journal::at(self.0.join("usbipd-journal.json"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn usbipd_state_reads_shared_attached_and_unplugged_devices() {
    let json = r#"{
  "Devices": [
    { "BusId": "1-3", "ClientIPAddress": null, "Description": "DualSense", "InstanceId": "USB\\VID_054C&PID_0CE6\\5&1A2B3C4D&0&3", "IsForced": false, "PersistedGuid": null, "StubInstanceId": null },
    { "BusId": "2-1", "ClientIPAddress": "172.29.0.2", "Description": "Stick", "InstanceId": "USB\\VID_0F0D&PID_0092\\6&2&0&1", "IsForced": false, "PersistedGuid": "2f1e6a64-9b3c-4f0e-8d7a-0123456789ab", "StubInstanceId": "USB\\Vid_80EE&Pid_CAFE\\x" },
    { "BusId": null, "ClientIPAddress": null, "Description": "Unplugged", "InstanceId": "USB\\VID_1234&PID_5678\\9", "IsForced": false, "PersistedGuid": "11111111-2222-3333-4444-555555555555", "StubInstanceId": null },
    { "BusId": "IncompatibleHub", "InstanceId": "USB\\VID_AAAA&PID_BBBB\\1" }
  ]
}"#;
    let state = parse_state(json).unwrap();
    assert_eq!(state.len(), 4);
    let dualsense = find(&state, &DUALSENSE.to_ascii_lowercase()).expect("instance ids match in any case");
    assert_eq!((dualsense.busid(), dualsense.shared(), dualsense.attached()), (Some("1-3"), false, false));
    let stick = find(&state, STICK).unwrap();
    assert_eq!((stick.busid(), stick.shared(), stick.attached()), (Some("2-1"), true, true));
    assert_eq!(state[2].busid(), None, "unplugged: nothing to import");
    assert!(state[2].shared());
    assert_eq!(state[3].busid(), None, "a hub usbipd-win can't use");
    assert!(parse_state("not json").unwrap_err().contains("usbipd-win"));
}

#[test]
fn only_usbipd_win_busids_and_guids_reach_its_command_line() {
    for good in ["1-2", "12-99", "3-10"] { assert!(valid_busid(good), "{good}"); }
    for bad in ["", "1", "1-", "-2", "0-1", "1-02", "1-2.3", "1-2 --force", "1-2&x", "IncompatibleHub"] { assert!(!valid_busid(bad), "{bad}"); }
    assert!(valid_guid(GUID));
    assert!(valid_guid(&format!("{{{GUID}}}")));
    for bad in ["", "not-a-guid", "2f1e6a64-9b3c-4f0e-8d7a-0123456789a", "2f1e6a64-9b3c-4f0e-8d7a-0123456789ab --all"] { assert!(!valid_guid(bad), "{bad}"); }
}

#[test]
fn the_journal_is_empty_without_a_file_and_never_guessed_when_unreadable() {
    let scratch = Scratch::new("journal");
    let journal = scratch.journal();
    assert_eq!(journal.entries().unwrap(), vec![]);
    let entry = Entry { instance_id: DUALSENSE.into(), busid: "1-3".into(), guid: None };
    journal.record(entry.clone()).unwrap();
    journal.record(Entry { guid: Some(GUID.into()), ..entry.clone() }).unwrap();
    assert_eq!(journal.entries().unwrap(), vec![Entry { guid: Some(GUID.into()), ..entry.clone() }], "one entry per device");
    journal.forget(&DUALSENSE.to_ascii_lowercase()).unwrap();
    assert!(!journal.path().exists(), "nothing left: no journal");
    std::fs::write(journal.path(), "{ broken").unwrap();
    assert!(journal.entries().is_err(), "an unreadable journal is an error, not an empty one");
    assert!(journal.record(entry).is_err(), "and nothing is recorded over it");
}

#[test]
fn sharing_journals_before_the_bind_and_leaves_the_users_own_shares_alone() {
    let scratch = Scratch::new("share");
    let journal = scratch.journal();
    let fake = Fake { journal: Some(journal.clone()), ..Fake::with(vec![listed(DUALSENSE, Some("1-3"), None), listed(STICK, Some("2-1"), Some(OTHER_GUID))]) };
    let state = fake.state().unwrap();

    let ours = share(&fake, &journal, find(&state, DUALSENSE).unwrap()).unwrap();
    assert_eq!(ours, Share::Ours { instance_id: DUALSENSE.into(), guid: GUID.into() });
    assert_eq!(journal.entries().unwrap(), vec![Entry { instance_id: DUALSENSE.into(), busid: "1-3".into(), guid: Some(GUID.into()) }]);

    assert_eq!(share(&fake, &journal, find(&state, STICK).unwrap()).unwrap(), Share::Theirs);
    assert_eq!(fake.calls(), vec!["bind 1-3"], "the user's share is used as it is");
    assert_eq!(journal.entries().unwrap().len(), 1, "and never journaled");

    let mut attached = listed(STICK, Some("2-1"), Some(OTHER_GUID));
    attached.client_ip_address = Some(serde_json::json!("172.29.0.2"));
    assert!(share(&fake, &journal, &attached).unwrap_err().contains("Another computer"));
    assert!(share(&fake, &journal, &listed(STICK, None, None)).is_err(), "unplugged: nothing to share");
}

#[test]
fn a_failed_or_cancelled_bind_leaves_nothing_journaled() {
    let scratch = Scratch::new("cancelled");
    let journal = scratch.journal();
    let fake = Fake { fail_bind: true, journal: Some(journal.clone()), ..Fake::with(vec![listed(DUALSENSE, Some("1-3"), None)]) };
    let state = fake.state().unwrap();
    assert_eq!(share(&fake, &journal, &state[0]).unwrap_err(), "cancelled");
    assert_eq!(journal.entries().unwrap(), vec![], "not shared: nothing to undo");
    assert_eq!(fake.calls(), vec!["bind 1-3"]);
}

#[test]
fn unsharing_undoes_only_nightjars_share_by_its_guid() {
    let scratch = Scratch::new("unshare");
    let journal = scratch.journal();
    let fake = Fake::with(vec![listed(DUALSENSE, Some("1-3"), Some(GUID)), listed(STICK, Some("2-1"), Some(OTHER_GUID))]);
    journal.record(Entry { instance_id: DUALSENSE.into(), busid: "1-3".into(), guid: Some(GUID.into()) }).unwrap();
    unshare(&fake, &journal, DUALSENSE, GUID).unwrap();
    assert_eq!(fake.calls(), vec![format!("unbind {GUID}")]);
    assert_eq!(journal.entries().unwrap(), vec![]);
    assert_eq!(fake.state().unwrap()[1].persisted_guid.as_deref(), Some(OTHER_GUID), "the user's share stays");

    // The user unshared it meanwhile and shared it again: that new share is theirs.
    let fake = Fake::with(vec![listed(DUALSENSE, Some("1-3"), Some(OTHER_GUID))]);
    journal.record(Entry { instance_id: DUALSENSE.into(), busid: "1-3".into(), guid: Some(GUID.into()) }).unwrap();
    unshare(&fake, &journal, DUALSENSE, GUID).unwrap();
    assert_eq!(fake.calls(), Vec::<String>::new());
    assert_eq!(journal.entries().unwrap(), vec![]);
}

#[test]
fn the_next_start_unshares_what_a_crash_left_and_nothing_else() {
    let scratch = Scratch::new("recover");
    let journal = scratch.journal();
    let pending = r"USB\VID_1209&PID_0001\7";
    let gone = r"USB\VID_1209&PID_0002\8";
    let fake = Fake::with(vec![
        listed(DUALSENSE, Some("1-3"), Some(GUID)),
        listed(STICK, Some("2-1"), Some(OTHER_GUID)),
        listed(pending, None, Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")),
        listed(gone, Some("2-4"), None),
    ]);
    journal.record(Entry { instance_id: DUALSENSE.into(), busid: "1-3".into(), guid: Some(GUID.into()) }).unwrap();
    // Crashed while Windows' prompt for the bind was up: no GUID journaled yet.
    journal.record(Entry { instance_id: pending.into(), busid: "1-7".into(), guid: None }).unwrap();
    // Unshared by the user meanwhile.
    journal.record(Entry { instance_id: gone.into(), busid: "2-4".into(), guid: Some("99999999-8888-7777-6666-555555555555".into()) }).unwrap();
    assert_eq!(recover(&fake, &journal).unwrap(), 2);
    assert_eq!(fake.calls(), vec![format!("unbind {GUID}"), "unbind aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_owned()]);
    assert_eq!(journal.entries().unwrap(), vec![]);
    assert_eq!(find(&fake.state().unwrap(), STICK).unwrap().persisted_guid.as_deref(), Some(OTHER_GUID), "never journaled, never unshared");
    assert_eq!(recover(&fake, &journal).unwrap(), 0, "nothing left");

    // A cancelled prompt keeps the entry for the next start.
    let fake = Fake { fail_unbind: true, ..Fake::with(vec![listed(DUALSENSE, Some("1-3"), Some(GUID))]) };
    journal.record(Entry { instance_id: DUALSENSE.into(), busid: "1-3".into(), guid: Some(GUID.into()) }).unwrap();
    assert_eq!(recover(&fake, &journal).unwrap(), 0);
    assert_eq!(journal.entries().unwrap().len(), 1);
}

/// What the loopback server answers an import with.
enum Answer {
    Device(u16, u16, &'static str),
    Status(u32),
}

/// A USB/IP server on `127.0.0.1:0` that answers one import, then echoes what follows; the
/// busid it was asked for comes back from the thread.
fn usbip_server(answer: Answer) -> (SocketAddr, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let addr = listener.local_addr().unwrap();
    let thread = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; wire::IMPORT_REQUEST_BYTES];
        stream.read_exact(&mut request).unwrap();
        let asked = wire::decode_import_request(&request).unwrap();
        match answer {
            Answer::Device(vendor, product, busid) => {
                stream.write_all(&wire::import_reply(0)).unwrap();
                let device = wire::Device { path: "/usbipd/1-3".into(), busid: busid.into(), busnum: 1, devnum: 3, speed: 3, vendor, product, ..Default::default() };
                stream.write_all(&device.encode().unwrap()).unwrap();
                let mut byte = [0; 1];
                while stream.read_exact(&mut byte).is_ok() {
                    if stream.write_all(&byte).is_err() { break; }
                }
            }
            Answer::Status(status) => stream.write_all(&wire::import_reply(status)).unwrap(),
        }
        asked
    });
    (addr, thread)
}

#[test]
fn the_import_checks_the_device_and_goes_to_this_computer_only() {
    let (addr, server) = usbip_server(Answer::Device(0x054c, 0x0ce6, "1-3"));
    let (_stream, device) = import(addr, "1-3", 0x054c, 0x0ce6).unwrap();
    assert_eq!((device.busnum, device.devnum, device.busid.as_str()), (1, 3, "1-3"));
    drop(_stream);
    assert_eq!(server.join().unwrap(), "1-3");

    let (addr, server) = usbip_server(Answer::Device(0x045e, 0x0b12, "1-3"));
    assert!(import(addr, "1-3", 0x054c, 0x0ce6).unwrap_err().contains("another device"), "vendor and product must match");
    server.join().unwrap();
    let (addr, server) = usbip_server(Answer::Device(0x054c, 0x0ce6, "1-4"));
    assert!(import(addr, "1-3", 0x054c, 0x0ce6).unwrap_err().contains("another device"), "busid must match");
    server.join().unwrap();
    let (addr, server) = usbip_server(Answer::Status(1));
    assert!(import(addr, "1-3", 0x054c, 0x0ce6).unwrap_err().contains("status 1"));
    server.join().unwrap();

    // TEST-NET-1: refused before anything connects.
    let remote: SocketAddr = "192.0.2.1:3240".parse().unwrap();
    assert!(import(remote, "1-3", 0x054c, 0x0ce6).unwrap_err().contains("not on this computer"));
    let unused = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap();
    assert!(import(unused, "1-3", 0x054c, 0x0ce6).unwrap_err().contains("isn’t running"), "nothing listens");
}

fn picked() -> Picked {
    Picked { busid: "1-2.4".into(), vid: 0x054c, pid: 0x0ce6, instance_id: DUALSENSE.into(), class: 3 }
}

#[test]
fn a_forwarded_device_is_relayed_and_unshared_when_the_attachment_ends() {
    let scratch = Scratch::new("open");
    let journal = scratch.journal();
    let fake = Arc::new(Fake::with(vec![listed(DUALSENSE, Some("1-3"), None)]));
    let (addr, server) = usbip_server(Answer::Device(0x054c, 0x0ce6, "1-3"));
    let device = open_with(fake.clone(), journal.clone(), addr, &picked()).unwrap();
    let described = device.describe();
    assert_eq!((described.busid.as_str(), described.busnum, described.devnum), ("1-2.4", 1, 3), "Nightjar's busid, usbipd-win's numbers");
    assert_eq!(device.class(), 3);
    let tunnel = device.relay().unwrap().unwrap();
    tunnel.write_all(b"x").unwrap();
    let mut echoed = [0; 1];
    while tunnel.read(&mut echoed).unwrap() == 0 {}
    assert_eq!(&echoed, b"x", "the tunnel reaches usbipd-win's connection");
    assert!(device.relay().unwrap().is_err(), "served once");
    assert_eq!(journal.entries().unwrap().len(), 1);
    device.release(true);
    assert_eq!(fake.calls(), vec!["bind 1-3".to_owned(), format!("unbind {GUID}")]);
    assert_eq!(journal.entries().unwrap(), vec![], "unshared and forgotten");
    assert_eq!(server.join().unwrap(), "1-3");
    device.release(true);
    assert_eq!(fake.calls().len(), 2, "released once");
}

#[test]
fn a_failed_import_undoes_nightjars_share_and_a_users_share_stays() {
    let scratch = Scratch::new("failed");
    let journal = scratch.journal();
    let fake = Arc::new(Fake::with(vec![listed(DUALSENSE, Some("1-3"), None)]));
    let (addr, server) = usbip_server(Answer::Status(1));
    assert!(open_with(fake.clone(), journal.clone(), addr, &picked()).is_err());
    assert_eq!(fake.calls(), vec!["bind 1-3".to_owned(), format!("unbind {GUID}")]);
    assert_eq!(journal.entries().unwrap(), vec![]);
    server.join().unwrap();

    let fake = Arc::new(Fake::with(vec![listed(DUALSENSE, Some("1-3"), Some(OTHER_GUID))]));
    let (addr, server) = usbip_server(Answer::Device(0x054c, 0x0ce6, "1-3"));
    let device = open_with(fake.clone(), journal.clone(), addr, &picked()).unwrap();
    device.release(true);
    drop(device);
    assert_eq!(fake.calls(), Vec::<String>::new(), "the user's share is neither bound nor unbound");
    assert_eq!(fake.state().unwrap()[0].persisted_guid.as_deref(), Some(OTHER_GUID));
    server.join().unwrap();

    assert!(open_with(Arc::new(Fake::default()), journal, LOOPBACK, &picked()).unwrap_err().contains("doesn’t list"));
}
