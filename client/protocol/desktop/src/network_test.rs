//! The network tests a PC's settings run: Connection test, as Android's `PyroWaveBandwidthTest`
//! (the delay first, then the speed where the PC has a speed test), and PyroWave's frame-pace
//! calibration. Both run on their own thread, never during a stream.

use std::sync::{Arc, mpsc::{channel, Receiver}, atomic::{AtomicBool, Ordering}};
use std::time::Duration;
use nightjar_core::http::{pace, ConnectionTestStats, Family, Feature, HostClient, ServerInfo};
use nightjar_policy::connection_test::{self as plan, Speed, SpeedStep};
use crate::{config::PaceCalibration, network::Network, strings::{en, tr, trf}};

pub enum Outcome {
    Connection(Connection),
    Pace(PaceCalibration, pace::Measurement),
}

pub enum Event { Progress(String), Done(Result<Outcome, String>) }

/// What Connection test found: the delay, and the speed or why it was not measured (`None` for a
/// PC without a speed test, which gets the delay alone).
#[derive(Clone, Debug, PartialEq)]
pub struct Connection {
    pub latency_ms: Option<f64>,
    pub jitter_ms: Option<f64>,
    pub loss_percent: Option<f64>,
    pub speed: Option<Speed>,
}

impl Connection {
    pub fn new(stats: &ConnectionTestStats, speed: Option<Speed>) -> Self {
        Self { latency_ms: stats.latency_ms(), jitter_ms: stats.jitter_ms(), loss_percent: stats.loss_percent(), speed }
    }

    /// "Ping 63 ms · Jitter 41 ms · Loss 0.0 %", Android's `connection_test_stats`.
    pub fn delay_line(&self) -> String {
        let ms = |value| plan::milliseconds(value).map_or_else(|| plan::UNAVAILABLE.to_owned(), |n| trf("{} ms", &[&local(n)]));
        let loss = plan::percent(self.loss_percent).map_or_else(|| plan::UNAVAILABLE.to_owned(), |n| trf("{} %", &[&local(n)]));
        trf("Ping {} · Jitter {} · Loss {}", &[&ms(self.latency_ms), &ms(self.jitter_ms), &loss])
    }

    /// The second line: the speed and the PC's link, or why the speed was not measured.
    pub fn speed_line(&self) -> Option<String> {
        Some(match self.speed? {
            Speed::Measured { mbps, link_mbps } => {
                let speed = trf("Speed {} Mbps", &[&local(plan::megabits(mbps))]);
                if link_mbps > 0 { format!("{speed} · {}", trf("PC link {} Mbps", &[&link_mbps])) } else { speed }
            }
            Speed::NeedsPairing => tr("Speed not measured: pair this PC first").to_owned(),
            Speed::NoAnswer => tr("Speed not measured: the PC didn’t answer").to_owned(),
            Speed::Unsupported => tr("Speed not measured: this PC has no speed test").to_owned(),
            Speed::Failed => tr("Speed not measured: the test failed").to_owned(),
        })
    }
}

/// A number with the language's decimal comma.
fn local(number: String) -> String {
    if crate::strings::current().decimal_comma() { number.replace('.', ",") } else { number }
}

pub struct Test {
    pub host: u64,
    /// PyroWave's frame-pace calibration rather than Connection test.
    pub calibrate: bool,
    pub events: Receiver<Event>,
    client: Arc<HostClient>,
    cancelled: Arc<AtomicBool>,
}

impl Test {
    /// Starts a test on `client`'s PC. `measures_speed` is whether Connection test also measures
    /// the speed there (`Host::bandwidth_test`); calibration ignores it.
    #[allow(clippy::too_many_arguments)]
    pub fn start(host: u64, client: HostClient, network: Network, address: String, mode: (u32, u32), calibrate: bool, measures_speed: bool, waker: crate::hosts::Waker) -> Self {
        let client = Arc::new(client);
        let worker = client.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = cancelled.clone();
        let (tx, events) = channel();
        std::thread::spawn(move || {
            let result = (|| -> Result<Outcome, String> {
                if !calibrate {
                    let progress = |stage: &str| { let _ = tx.send(Event::Progress(stage.to_owned())); waker(); };
                    return Ok(Outcome::Connection(connection(&worker, measures_speed, || stop.load(Ordering::Acquire), progress)));
                }
                // Calibrate for the interface that reaches this PC, which is what the stream will use.
                let network = network.toward(crate::network::toward(&address));
                let info = worker.server_info_https().map_err(|e| e.to_string())?;
                if !info.paired || !info.capabilities.has(Feature::PyroWave) || !info.pyrowave_burst_probe {
                    return Err("This paired PC does not support PyroWave pace calibration.".into());
                }
                let link = calibration_link(network.link_mbps, info.pyrowave_host_link_mbps);
                if link == 0 { return Err("Neither endpoint reports a link speed. No calibration was saved.".into()); }
                let (bitrate, fps) = mode;
                let kbps = pace::probe_kbps(bitrate, fps, pace::PACKET_SIZE, info.pyrowave_critical_fec_percentage, info.pyrowave_min_parity_shards);
                let floor = kbps / 4 * 5;
                if floor >= link * 1000 { return Err("The stream's wire bitrate is too high for this link. Lower its bitrate before calibrating.".into()); }
                let outcome = pace::search(link * 1000, floor, |pace_kbps| {
                    let _ = tx.send(Event::Progress(format!("Testing {} Mbps frame pace", pace_kbps / 1000)));
                    waker();
                    worker.probe_pace(kbps, pace::PACKET_SIZE, fps.clamp(10, 500), pace_kbps.max(kbps))
                }, || stop.load(Ordering::Acquire)).map_err(|e| e.to_string())?
                    .ok_or("No usable pace was measured. The PC will keep its default.")?;
                Ok(Outcome::Pace(PaceCalibration { pace_mbps: outcome.pace_kbps / 1000, client_link_mbps: network.link_mbps,
                    host_link_mbps: info.pyrowave_host_link_mbps, wifi: network.wifi, ssid: network.ssid,
                    qualified: outcome.qualified }, outcome.measurement))
            })();
            if !stop.load(Ordering::Acquire) { let _ = tx.send(Event::Done(result)); waker(); }
        });
        Self { host, calibrate, events, client, cancelled }
    }
}

/// Connection test, as Android's `PyroWaveBandwidthTest`: [`plan::PROBES`] delay probes over
/// HTTP, then the PC's speed test where it has one. `progress` gets each stage in English (the
/// dialog translates it). A cancel ends it early; what it returns then is never shown.
fn connection(client: &HostClient, measures_speed: bool, cancelled: impl Fn() -> bool, progress: impl Fn(&str)) -> Connection {
    progress(en("Measuring ping, jitter and loss"));
    let probe = || client.probe_server_latency(Duration::from_millis(plan::PROBE_TIMEOUT_MS)).ok();
    let stats = delay(probe, || std::thread::sleep(Duration::from_millis(plan::PROBE_INTERVAL_MS)), &cancelled);
    let step = plan::speed_step(measures_speed, client.server_certificate_pem().is_some(), stats.latency_ms());
    let speed = match step {
        SpeedStep::Measure if !cancelled() => {
            progress(en("Receiving 32 MiB from the PC"));
            Some(speed(client))
        }
        SpeedStep::Measure => None,
        step => Speed::of_step(step),
    };
    Connection::new(&stats, speed)
}

/// The delay probes: `probe` answers one round trip in milliseconds, `None` for a lost probe;
/// `pause` waits between two.
fn delay(mut probe: impl FnMut() -> Option<f64>, mut pause: impl FnMut(), cancelled: impl Fn() -> bool) -> ConnectionTestStats {
    let mut stats = ConnectionTestStats::default();
    for n in 0..plan::PROBES {
        if cancelled() { break; }
        if n > 0 { pause(); }
        match probe() {
            Some(ms) => stats.add(ms),
            None => stats.add_failure(),
        }
    }
    stats
}

/// The PC's speed test, or why it gave no speed.
fn speed(client: &HostClient) -> Speed {
    match client.server_info_https() {
        Ok(info) => bandwidth(client, &info),
        Err(e) => {
            eprintln!("nightjar: connection test: serverinfo: {e}");
            Speed::Failed
        }
    }
}

/// Rubylight's PyroWave bandwidth test, else Foundation Sunshine's (`/api/network/probe`, as
/// Moonlight V+ Android runs it).
fn bandwidth(client: &HostClient, info: &ServerInfo) -> Speed {
    let measured = if info.capabilities.has(Feature::PyroWaveBandwidthProbe) {
        client.probe_bandwidth(info).map(|mbps| Speed::Measured { mbps, link_mbps: info.pyrowave_host_link_mbps })
    } else if has_foundation_probe(info) {
        client.foundation_network_probe().map(|result| Speed::Measured { mbps: result.bandwidth_mbps, link_mbps: 0 })
    } else {
        return Speed::Unsupported;
    };
    match measured {
        Ok(speed) => speed,
        Err(nightjar_core::http::Error::NotFound(_)) => Speed::Unsupported,
        Err(e) => {
            eprintln!("nightjar: connection test: speed: {e}");
            Speed::Failed
        }
    }
}

/// A paired Sunshine-family PC may be Foundation Sunshine, which has its own bandwidth test; the
/// PC is asked once whether it does (`Host::bandwidth_test`).
pub fn has_foundation_probe(info: &ServerInfo) -> bool {
    info.paired && info.capabilities.family() == Family::Sunshine
}

/// The slower of the two ends' reported rates bounds the search; an end that reports none is ignored.
fn calibration_link(client_mbps: u32, host_mbps: u32) -> u32 {
    [client_mbps, host_mbps].into_iter().filter(|&n| n > 0).min().unwrap_or(0).min(9999)
}

impl Drop for Test {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.client.cancel_pending_requests();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Foundation Sunshine's test is offered to paired Sunshine PCs; Rubylight keeps its own.
    #[test]
    fn the_foundation_bandwidth_test_is_offered_to_paired_sunshine_pcs() {
        let info = |extra: &str, paired: u8| ServerInfo::parse(&format!("<root status_code=\"200\"><uniqueid>pc</uniqueid>\
            <state>SUNSHINE_SERVER_FREE</state><PairStatus>{paired}</PairStatus>{extra}</root>"), 47989).unwrap();
        assert!(has_foundation_probe(&info("<appversion>7.1.431.-1</appversion><SunshineVersion>2025.1</SunshineVersion><VddCapabilityVersion>1</VddCapabilityVersion>", 1)));
        assert!(!has_foundation_probe(&info("<appversion>7.1.431.-1</appversion>", 0)), "unpaired");
        assert!(!has_foundation_probe(&info("<RustHostVersion>3.0.0</RustHostVersion>", 1)), "Rubylight");
    }

    #[test]
    fn calibration_link_is_capped_by_the_wifi_rate_the_route_keeps() {
        let wifi = Network { wifi: true, ssid: Some("Home".into()), link_mbps: 866, ..Default::default() };
        let route = wifi.toward(Ok(crate::network::Toward { wireless: true, link_mbps: 0 }));
        assert_eq!(calibration_link(route.link_mbps, 2500), 866);
        // A host that reports nothing does not stop a Wi-Fi calibration.
        assert_eq!(calibration_link(route.link_mbps, 0), 866);
        assert_eq!(calibration_link(0, 0), 0);
        assert_eq!(calibration_link(40_000, 20_000), 9999);
    }

    fn result(latency: f64, jitter: f64, loss: f64, speed: Option<Speed>) -> Connection {
        let known = |v: f64| (!v.is_nan()).then_some(v);
        Connection { latency_ms: known(latency), jitter_ms: known(jitter), loss_percent: known(loss), speed }
    }

    // The lines of Android's ConnectionTestResultTest, in desktop's "{} %".
    #[test]
    fn the_result_reads_as_androids() {
        assert_eq!(result(63.0, 41.0, 0.0, None).delay_line(), "Ping 63 ms · Jitter 41 ms · Loss 0.0 %");
        assert_eq!(result(2.5, 1.0, 25.0, None).delay_line(), "Ping 2.5 ms · Jitter 1.0 ms · Loss 25.0 %");
        assert_eq!(result(f64::NAN, f64::NAN, 100.0, None).delay_line(), "Ping – · Jitter – · Loss 100.0 %");
        assert_eq!(result(63.0, 41.0, 0.0, None).speed_line(), None, "a PC without a speed test: the delay alone");
        let speed = |speed| result(5.0, 1.0, 0.0, Some(speed)).speed_line().unwrap();
        assert_eq!(speed(Speed::Measured { mbps: 312.4, link_mbps: 1000 }), "Speed 312.4 Mbps · PC link 1000 Mbps");
        assert_eq!(speed(Speed::Measured { mbps: 87.0, link_mbps: 0 }), "Speed 87.0 Mbps");
        assert_eq!(speed(Speed::NeedsPairing), "Speed not measured: pair this PC first");
        assert_eq!(speed(Speed::NoAnswer), "Speed not measured: the PC didn’t answer");
        assert_eq!(speed(Speed::Unsupported), "Speed not measured: this PC has no speed test");
        assert_eq!(speed(Speed::Failed), "Speed not measured: the test failed");
    }

    #[test]
    fn the_result_reads_in_german_with_decimal_commas() {
        crate::strings::tests::with_lang(crate::strings::Lang::De, || {
            assert_eq!(result(2.5, 1.0, 25.0, None).delay_line(), "Ping 2,5 ms · Jitter 1,0 ms · Verlust 25,0 %");
            let line = result(5.0, 1.0, 0.0, Some(Speed::Measured { mbps: 312.4, link_mbps: 1000 })).speed_line().unwrap();
            assert_eq!(line, "Tempo 312,4 Mbit/s · PC-LAN 1000 Mbit/s");
        });
    }

    // ConnectionTestStatsTest's sequence, through the probe loop.
    #[test]
    fn ten_probes_with_a_pause_between_two_and_lost_ones_counted() {
        let mut answers = [Some(10.0), None, Some(100.0), Some(110.0)].into_iter().chain(std::iter::repeat(Some(110.0)));
        let (mut probes, mut pauses) = (0, 0);
        let stats = delay(|| { probes += 1; answers.next().unwrap() }, || pauses += 1, || false);
        assert_eq!((probes, pauses), (10, 9));
        assert_eq!(stats.attempts(), 10);
        assert_eq!(stats.loss_percent(), Some(10.0));
        assert_eq!(stats.jitter_ms(), Some(10.0 / 7.0), "the lost probe breaks the sequence: 10 then six zeros over 7");
        let cancelled = std::cell::Cell::new(false);
        let stats = delay(|| { cancelled.set(true); Some(5.0) }, || {}, || cancelled.get());
        assert_eq!(stats.attempts(), 1, "a cancel stops the probes");
    }

    /// A PC's HTTP port on this machine: each of `connections` gets serverinfo when `answers`,
    /// else is closed unanswered.
    fn http_pc(answers: bool, connections: usize) -> (u16, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let served = std::thread::spawn(move || {
            let body = "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><hostname>PC</hostname>\
                <uniqueid>uid</uniqueid><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort>\
                <PairStatus>0</PairStatus><currentgame>0</currentgame><state>SUNSHINE_SERVER_FREE</state></root>";
            for _ in 0..connections {
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = [0u8; 4096];
                let _ = socket.read(&mut request);
                assert!(request.starts_with(b"GET /serverinfo?"), "{}", String::from_utf8_lossy(&request));
                if answers {
                    let reply = format!("HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    socket.write_all(reply.as_bytes()).unwrap();
                }
            }
        });
        (port, served)
    }

    fn client(port: u16, paired: bool) -> HostClient {
        let identity = Arc::new(nightjar_core::http::ClientIdentity::generate().expect("an identity"));
        let client = HostClient::new("127.0.0.1", port, identity.clone()).unwrap();
        if paired { client.with_server_certificate_pem(identity.certificate_pem()).unwrap() } else { client }
    }

    #[test]
    fn a_pc_that_answers_has_its_delay_measured_over_plain_http() {
        let (port, served) = http_pc(true, plan::PROBES as usize);
        let stages = std::cell::RefCell::new(Vec::new());
        let found = connection(&client(port, false), false, || false, |stage| stages.borrow_mut().push(stage.to_owned()));
        served.join().unwrap();
        assert_eq!(found.loss_percent, Some(0.0));
        assert!(found.latency_ms.is_some_and(|ms| (0.0..2000.0).contains(&ms)), "{found:?}");
        assert!(found.jitter_ms.is_some());
        assert_eq!(found.speed, None, "no speed test, no second line");
        assert_eq!(*stages.borrow(), ["Measuring ping, jitter and loss"]);
    }

    #[test]
    fn a_pc_that_never_answers_is_all_loss_and_says_why_speed_is_missing() {
        for (paired, why) in [(false, Speed::NeedsPairing), (true, Speed::NoAnswer)] {
            let (port, served) = http_pc(false, plan::PROBES as usize);
            let found = connection(&client(port, paired), true, || false, |_| {});
            served.join().unwrap();
            assert_eq!(found, Connection { latency_ms: None, jitter_ms: None, loss_percent: Some(100.0), speed: Some(why) });
            assert_eq!(found.delay_line(), "Ping – · Jitter – · Loss 100.0 %");
        }
    }
}
