//! Reaching a host from outside its network, shared by desktop and Android: the paths in order
//! (local network, IPv6, a forwarded IPv4 port, a punched path), one tunnel per host, and the
//! words for the path in use and for when none works.
//!
//! The direct paths are plain addresses, tried by the caller's own probe (a serverinfo request).
//! The punched path starts a [`ClientTunnel`] that binds the host's ports on a loopback address
//! of this device; the caller then talks to that address exactly as to the host, so streams,
//! pairing pins and Moonlight's engine stay as they are.

use crate::http::InternetPlay;
use rubylight_traverse::{
    client::{ClientConfig, ClientTunnel, Status},
    path::{self, Path},
};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicBool, Ordering},
        LazyLock, Mutex,
    },
    time::{Duration, Instant},
};

pub use rubylight_traverse::path::{failure_message, Outcome, Path as InternetPath};

/// How long the punched path may take to come up: the DHT lookups, the host's next look for
/// this device's record (every ten seconds) and the punching itself.
pub const PUNCH_WAIT: Duration = Duration::from_secs(50);

/// How long after a punched path failed it is not tried again: a PC that is off would
/// otherwise have the DHT asked on every poll.
pub const RETRY_AFTER: Duration = Duration::from_secs(60);

struct Entry {
    tunnel: ClientTunnel,
    started: Instant,
}

static TUNNELS: LazyLock<Mutex<HashMap<String, Entry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// This device's "Play over the internet" setting; off until the app says otherwise.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Takes in the setting; turning it off ends every tunnel.
pub fn set_enabled(on: bool) {
    if !ENABLED.swap(on, Ordering::AcqRel) || on {
        return;
    }
    stop_all();
}

/// Whether this device may look up and punch paths.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Acquire)
}

/// The tunnel to `host_id`, started from `info` unless one is already up or on its way.
pub fn tunnel(host_id: &str, info: &InternetPlay) -> Result<ClientTunnel, String> {
    let secret = info.secret.ok_or_else(|| {
        "connect once on the PC's network with Play over the internet on".to_owned()
    })?;
    let mut tunnels = TUNNELS.lock().unwrap();
    if let Some(entry) = tunnels.get(host_id) {
        if entry.tunnel.is_active() {
            return Ok(entry.tunnel.clone());
        }
        if entry.started.elapsed() < RETRY_AFTER {
            return Err(match entry.tunnel.status() {
                Status::Failed(reason) => reason,
                _ => "stopped".into(),
            });
        }
        entry.tunnel.stop();
    }
    let tunnel = ClientTunnel::start(ClientConfig {
        host_id: host_id.to_owned(),
        secret,
        known: Some(info.host_info()),
        stun_servers: rubylight_traverse::STUN_SERVERS
            .iter()
            .map(|server| (*server).to_owned())
            .collect(),
        bootstrap: rubylight_traverse::DHT_BOOTSTRAP
            .iter()
            .map(|node| (*node).to_owned())
            .collect(),
    });
    tunnels.insert(
        host_id.to_owned(),
        Entry {
            tunnel: tunnel.clone(),
            started: Instant::now(),
        },
    );
    Ok(tunnel)
}

/// Waits up to `wait` for `tunnel` to come through: the loopback address that reaches the host.
pub fn wait(tunnel: &ClientTunnel, wait: Duration) -> Result<IpAddr, String> {
    let deadline = Instant::now() + wait;
    loop {
        match tunnel.status() {
            Status::Connected { address, .. } => return Ok(address),
            Status::Failed(reason) => return Err(reason),
            Status::Stopped => return Err("stopped".into()),
            Status::LookingUp | Status::Punching => {}
        }
        if Instant::now() >= deadline {
            return Err("the PC did not answer in time".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The loopback address of `host_id`'s tunnel while it is through.
pub fn connected(host_id: &str) -> Option<IpAddr> {
    match TUNNELS.lock().unwrap().get(host_id)?.tunnel.status() {
        Status::Connected { address, .. } => Some(address),
        _ => None,
    }
}

/// Whether `address` is the loopback end of an internet play tunnel (127.78.x.y, or 127.0.0.1
/// where the platform binds only that).
pub fn is_tunnel(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => ip.octets()[..2] == [127, 78] || TUNNELS
            .lock()
            .unwrap()
            .values()
            .any(|entry| matches!(entry.tunnel.status(), Status::Connected { address, .. } if address == IpAddr::V4(ip))),
        IpAddr::V6(_) => false,
    }
}

/// Ends `host_id`'s tunnel.
pub fn stop(host_id: &str) {
    if let Some(entry) = TUNNELS.lock().unwrap().remove(host_id) {
        entry.tunnel.stop();
    }
}

/// Ends every tunnel (the app closes, or the user turned Play over the internet off).
pub fn stop_all() {
    for (_, entry) in TUNNELS.lock().unwrap().drain() {
        entry.tunnel.stop();
    }
}

/// Which path `address` is, for a host known at `lan` with internet play reply `info`.
pub fn path_of(address: IpAddr, lan: &[IpAddr], info: Option<&InternetPlay>) -> Path {
    if is_tunnel(address) {
        return Path::Punched;
    }
    if lan.contains(&address) {
        return Path::Lan;
    }
    match (address, info) {
        (IpAddr::V6(ip), Some(info)) if info.ipv6.contains(&ip) => Path::Ipv6,
        (IpAddr::V4(ip), Some(info)) if info.external_ipv4 == Some(ip) => Path::MappedIpv4,
        _ => Path::Lan,
    }
}

/// Tries the paths to a host in order. `probe` asks the host at an address (its HTTP port) and
/// says whether it answered; the punched path starts or reuses the tunnel and probes its
/// loopback end. Returns the path and the address that answered, or the words for every path
/// that failed.
pub fn reach(
    host_id: &str,
    lan: &[IpAddr],
    info: Option<&InternetPlay>,
    punch: bool,
    mut probe: impl FnMut(SocketAddr) -> bool,
) -> Result<(Path, SocketAddr), String> {
    let known = match info {
        Some(info) => info.known(lan, punch),
        // Without a reply only the local addresses are known; the port is the caller's.
        None => path::Known {
            lan: lan.iter().map(|&ip| SocketAddr::new(ip, 0)).collect(),
            ..Default::default()
        },
    };
    path::first_working(&known, |path, address| match (path, address) {
        (Path::Punched, _) => {
            let info = info.ok_or(Outcome::Unknown)?;
            let tunnel = tunnel(host_id, info).map_err(Outcome::Failed)?;
            let ip = wait(&tunnel, PUNCH_WAIT).map_err(Outcome::Failed)?;
            let at = SocketAddr::new(ip, info.ports.http);
            if probe(at) {
                Ok(at)
            } else {
                Err(Outcome::Failed(
                    "the PC did not answer over the direct path".into(),
                ))
            }
        }
        (_, Some(at)) => {
            if probe(at) {
                Ok(at)
            } else {
                Err(Outcome::NoAnswer)
            }
        }
        (_, None) => Err(Outcome::Unknown),
    })
}

/// The words for a path, as the host list shows it.
pub fn label(path: Path) -> &'static str {
    path.label()
}

/// Which direct paths a poll asked, for [`unreachable`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tried {
    pub lan: bool,
    pub ipv6: bool,
    pub mapped: bool,
}

/// What a device says when no path reached a PC that gave it internet play details: every path
/// in order and why it failed. `punched` is why the punched path failed, None when it was not
/// tried (and this says why not).
pub fn unreachable(reply: &InternetPlay, tried: Tried, punched: Option<String>) -> String {
    let asked = |known: bool, asked: bool| match (known, asked) {
        (true, true) => Outcome::NoAnswer,
        _ => Outcome::Unknown,
    };
    let punched = match punched {
        Some(reason) => Outcome::Failed(reason),
        None if !enabled() => {
            Outcome::Failed("Play over the internet is off in Nightjar's settings".into())
        }
        None if !reply.enabled => {
            Outcome::Failed("Play over the internet is off on the PC".into())
        }
        None => Outcome::Unknown,
    };
    let mapped = reply.mapped_ports && reply.external_ipv4.is_some();
    failure_message(&[
        (Path::Lan, asked(true, tried.lan)),
        (Path::Ipv6, asked(!reply.ipv6.is_empty(), tried.ipv6)),
        (Path::MappedIpv4, asked(mapped, tried.mapped)),
        (Path::Punched, punched),
    ])
}

/// The punched path to `host_id` as one blocking step: the tunnel, then its loopback address,
/// or the words for every path when it does not come through.
pub fn punch(host_id: &str, reply: &InternetPlay, tried: Tried) -> Result<SocketAddr, String> {
    if !enabled() || !reply.enabled || reply.secret.is_none() {
        return Err(unreachable(reply, tried, None));
    }
    tunnel(host_id, reply)
        .and_then(|tunnel| wait(&tunnel, PUNCH_WAIT))
        .map(|ip| SocketAddr::new(ip, reply.ports.http))
        .map_err(|reason| unreachable(reply, tried, Some(reason)))
}

/// One JSON request in, one JSON answer out, for Android's `com.limelight.computers.InternetPlay`
/// (desktop calls the functions above directly). The answer is `{"ok": ...}` or
/// `{"error": words}`. Requests, by `op`:
///
/// - `enable` `{on}`: the device's setting.
/// - `addresses` `{reply}`: the internet addresses to poll after the local ones, in order:
///   `{"ipv6": [..], "mapped": ip|null, "port": http}`.
/// - `connect` `{host, reply, tried: {lan, ipv6, mapped}}`: blocks up to [`PUNCH_WAIT`];
///   `{"address", "port"}` of the tunnel's loopback end, or the error for every path.
/// - `unreachable` `{reply, tried}`: the error for every path without punching.
/// - `path` `{address, lan: [..], reply|null}`: `"lan"`, `"ipv6"`, `"mapped"` or `"punched"`.
/// - `stop` `{host}`, `stopAll`.
pub mod json {
    use super::*;
    use serde_json::{json, Value};

    fn reply(request: &Value) -> Result<InternetPlay, String> {
        let text = request
            .get("reply")
            .and_then(Value::as_str)
            .ok_or("no reply")?;
        InternetPlay::parse(text).map_err(|e| e.to_string())
    }

    fn tried(request: &Value) -> Tried {
        let flag = |name: &str| request["tried"][name].as_bool().unwrap_or(false);
        Tried {
            lan: flag("lan"),
            ipv6: flag("ipv6"),
            mapped: flag("mapped"),
        }
    }

    fn answer(request: &Value) -> Result<Value, String> {
        let text = |name: &str| request.get(name).and_then(Value::as_str);
        match text("op").ok_or("no op")? {
            "enable" => {
                set_enabled(request["on"].as_bool().ok_or("no on")?);
                Ok(Value::Null)
            }
            "addresses" => {
                let reply = reply(request)?;
                let mapped = reply.external_ipv4.filter(|_| reply.mapped_ports);
                Ok(json!({
                    "ipv6": reply.ipv6.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    "mapped": mapped.map(|ip| ip.to_string()),
                    "port": reply.ports.http,
                }))
            }
            "connect" => {
                let reply = reply(request)?;
                let host = text("host").ok_or("no host")?;
                let at = punch(host, &reply, tried(request))?;
                Ok(json!({"address": at.ip().to_string(), "port": at.port()}))
            }
            "unreachable" => Ok(Value::String(unreachable(
                &reply(request)?,
                tried(request),
                None,
            ))),
            "path" => {
                let address: IpAddr = text("address")
                    .and_then(|a| a.parse().ok())
                    .ok_or("no address")?;
                let lan: Vec<IpAddr> = request["lan"]
                    .as_array()
                    .map(|list| {
                        list.iter()
                            .filter_map(|a| a.as_str()?.parse().ok())
                            .collect()
                    })
                    .unwrap_or_default();
                let reply = reply(request).ok();
                Ok(Value::String(
                    path_of(address, &lan, reply.as_ref()).key().into(),
                ))
            }
            "stop" => {
                stop(text("host").ok_or("no host")?);
                Ok(Value::Null)
            }
            "stopAll" => {
                stop_all();
                Ok(Value::Null)
            }
            other => Err(format!("unknown op {other}")),
        }
    }

    pub fn call(request: &str) -> String {
        let answer = serde_json::from_str::<Value>(request)
            .map_err(|_| "the request is not JSON".to_owned())
            .and_then(|request| answer(&request));
        match answer {
            Ok(value) => json!({ "ok": value }),
            Err(error) => json!({ "error": error }),
        }
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = r#"{"version":1,"enabled":true,"portMapping":true,
        "externalIpv4":"203.0.113.5","mappedPorts":true,"ipv6":["2001:db8::5"],
        "ports":{"http":47989,"https":47984,"rtsp":48010,"video":47998,"audio":48000,"control":47999,"mic":48002}}"#;

    #[test]
    fn the_local_network_is_tried_first_and_answers_without_any_internet_path() {
        let info = InternetPlay::parse(REPLY).unwrap();
        let lan: [IpAddr; 1] = ["192.168.1.10".parse().unwrap()];
        let mut asked = Vec::new();
        let (path, at) = reach("host", &lan, Some(&info), true, |at| {
            asked.push(at);
            true
        })
        .unwrap();
        assert_eq!(path, Path::Lan);
        assert_eq!(at.to_string(), "192.168.1.10:47989");
        assert_eq!(asked.len(), 1);
    }

    #[test]
    fn away_from_home_ipv6_comes_before_the_forwarded_port() {
        let info = InternetPlay::parse(REPLY).unwrap();
        let lan: [IpAddr; 1] = ["192.168.1.10".parse().unwrap()];
        let mut asked = Vec::new();
        let (path, at) = reach("host", &lan, Some(&info), true, |at| {
            asked.push(at);
            at.ip() == "203.0.113.5".parse::<IpAddr>().unwrap()
        })
        .unwrap();
        assert_eq!(path, Path::MappedIpv4);
        assert_eq!(at.port(), 47989);
        let order: Vec<String> = asked.iter().map(ToString::to_string).collect();
        assert_eq!(
            order,
            [
                "192.168.1.10:47989",
                "[2001:db8::5]:47989",
                "203.0.113.5:47989"
            ]
        );
        assert_eq!(path_of(at.ip(), &lan, Some(&info)), Path::MappedIpv4);
        assert_eq!(
            path_of("2001:db8::5".parse().unwrap(), &lan, Some(&info)),
            Path::Ipv6
        );
        assert_eq!(
            path_of("127.78.1.2".parse().unwrap(), &lan, Some(&info)),
            Path::Punched
        );
    }

    #[test]
    fn when_nothing_answers_the_error_names_every_path() {
        let info = InternetPlay::parse(REPLY).unwrap();
        // No secret in the reply: the punched path is not tried and says how to get one.
        let error = reach("host", &[], Some(&info), true, |_| false).unwrap_err();
        assert!(error.starts_with("The PC could not be reached."), "{error}");
        assert!(error.contains("IPv6: no answer"), "{error}");
        assert!(error.contains("a forwarded port: no answer"), "{error}");
        assert!(
            error.contains("connect once on the PC's network"),
            "{error}"
        );
    }

    #[test]
    fn the_json_calls_answer_addresses_paths_and_refusals() {
        let request = |value: serde_json::Value| -> serde_json::Value {
            serde_json::from_str(&json::call(&value.to_string())).unwrap()
        };
        let addresses = request(serde_json::json!({"op": "addresses", "reply": REPLY}));
        assert_eq!(addresses["ok"]["ipv6"][0], "2001:db8::5");
        assert_eq!(addresses["ok"]["mapped"], "203.0.113.5");
        assert_eq!(addresses["ok"]["port"], 47989);
        let path = request(serde_json::json!({
            "op": "path", "address": "2001:db8::5", "lan": ["192.168.1.10"], "reply": REPLY
        }));
        assert_eq!(path["ok"], "ipv6");
        let lan = request(serde_json::json!({
            "op": "path", "address": "192.168.1.10", "lan": ["192.168.1.10"], "reply": null
        }));
        assert_eq!(lan["ok"], "lan");
        // No secret: connect refuses at once with the words for every path, and punches nothing.
        let refused = request(serde_json::json!({
            "op": "connect", "host": "h", "reply": REPLY, "tried": {"lan": true, "ipv6": true, "mapped": true}
        }));
        let words = refused["error"].as_str().unwrap();
        assert!(words.starts_with("The PC could not be reached."), "{words}");
        assert!(words.contains("a forwarded port: no answer"), "{words}");
        assert!(request(serde_json::json!({"op": "nope"}))["error"].is_string());
        assert!(serde_json::from_str::<serde_json::Value>(&json::call("not json")).unwrap()["error"].is_string());
    }

    #[test]
    fn the_failure_names_why_the_punched_path_failed() {
        let info = InternetPlay::parse(REPLY).unwrap();
        let words = unreachable(
            &info,
            Tried { lan: true, ipv6: false, mapped: true },
            Some("both routers change ports per destination".into()),
        );
        assert!(words.contains("the local network: no answer"), "{words}");
        assert!(words.contains("IPv6: the PC has no public IPv6 address known"), "{words}");
        assert!(words.contains("a direct internet path: both routers change ports"), "{words}");
    }
}
