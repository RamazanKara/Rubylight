use std::sync::{Arc, mpsc::{channel, Receiver}, atomic::{AtomicBool, Ordering}};
use nightjar_core::http::{pace, Family, Feature, HostClient, ServerInfo};
use crate::{config::PaceCalibration, network::Network};

pub enum Outcome {
    Bandwidth(f64),
    Pace(PaceCalibration, pace::Measurement),
}

pub enum Event { Progress(String), Done(Result<Outcome, String>) }

pub struct Test {
    pub host: u64,
    pub events: Receiver<Event>,
    client: Arc<HostClient>,
    cancelled: Arc<AtomicBool>,
}

impl Test {
    pub fn start(host: u64, client: HostClient, network: Network, address: String, mode: (u32, u32), calibrate: bool, waker: crate::hosts::Waker) -> Self {
        let client = Arc::new(client);
        let worker = client.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = cancelled.clone();
        let (tx, events) = channel();
        std::thread::spawn(move || {
            let result = (|| -> Result<Outcome, String> {
                // Calibrate for the interface that reaches this PC, which is what the stream will use.
                let network = network.toward(crate::network::toward(&address));
                let info = worker.server_info_https().map_err(|e| e.to_string())?;
                if !calibrate { return bandwidth(&worker, &info); }
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
        Self { host, events, client, cancelled }
    }
}

/// Rubylight's PyroWave bandwidth test, else Foundation Sunshine's (`/api/network/probe`, as
/// Moonlight V+ Android runs it).
fn bandwidth(client: &HostClient, info: &ServerInfo) -> Result<Outcome, String> {
    if info.capabilities.has(Feature::PyroWaveBandwidthProbe) || !has_foundation_probe(info) {
        return client.probe_bandwidth(info).map(Outcome::Bandwidth).map_err(|e| e.to_string());
    }
    match client.foundation_network_probe() {
        Ok(result) => Ok(Outcome::Bandwidth(result.bandwidth_mbps)),
        Err(nightjar_core::http::Error::NotFound(_)) => Err("This PC has no bandwidth test.".into()),
        Err(e) => Err(e.to_string()),
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
}
