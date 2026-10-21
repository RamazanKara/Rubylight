use std::sync::{Arc, mpsc::{channel, Receiver}, atomic::{AtomicBool, Ordering}};
use nightjar_core::http::{pace, HostClient, Feature};
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
    pub fn start(host: u64, client: HostClient, network: Network, mode: (u32, u32), calibrate: bool, waker: crate::hosts::Waker) -> Self {
        let client = Arc::new(client);
        let worker = client.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = cancelled.clone();
        let (tx, events) = channel();
        std::thread::spawn(move || {
            let result = (|| -> Result<Outcome, String> {
                let info = worker.server_info_https().map_err(|e| e.to_string())?;
                if !calibrate { return worker.probe_bandwidth(&info).map(Outcome::Bandwidth).map_err(|e| e.to_string()); }
                if !info.paired || !info.capabilities.has(Feature::PyroWave) || !info.pyrowave_burst_probe {
                    return Err("This paired PC does not support PyroWave pace calibration.".into());
                }
                let link = [network.link_mbps, info.pyrowave_host_link_mbps].into_iter().filter(|&n| n > 0).min().unwrap_or(0).min(9999);
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

impl Drop for Test {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.client.cancel_pending_requests();
    }
}
