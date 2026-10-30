//! Foundation Sunshine's HTTPS APIs for adaptive bitrate (`/api/abr`) and the bandwidth probe
//! (`/api/network/*`), on the paired HTTPS port with this client's certificate. The host runs
//! the bitrate controller and applies its bitrate to the running stream itself; the client turns
//! it on, reports its network every few seconds and shows the bitrate the host chose.
//!
//! Request and response shapes follow Foundation's `src/nvhttp/abr_api.cpp` and
//! `network_probe.cpp`, and their clients in Moonlight V+ PC (`NvHTTP::configureAbr`,
//! `sendAbrFeedback`, `Session::startSunshineAbr`) and V+ Android (`NvHTTP.runNetworkProbe`).

use super::transport::{self, Body, Request, Timeouts};
use super::{Error, HostClient, LONG_CONNECT_TIMEOUT};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// How often a client reports its network while host ABR runs (V+ PC's `ABR_FEEDBACK_INTERVAL_MS`).
pub const ABR_FEEDBACK_INTERVAL: Duration = Duration::from_secs(3);
/// The longest bandwidth probe this client asks for (V+ Android's `MAX_NETWORK_PROBE_BYTES`).
pub const MAX_NETWORK_PROBE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ABR_JSON_BYTES: usize = 64 * 1024;
/// V+ PC's timeouts: 2 s to turn ABR on and per report, 1 s to turn it off.
const ABR_TIMEOUT: Duration = Duration::from_secs(2);
const ABR_OFF_TIMEOUT: Duration = Duration::from_secs(1);
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// `GET /api/abr/capabilities`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AbrCapabilities {
    pub supported: bool,
    pub version: u32,
    pub features: Vec<String>,
    /// The host's own bitrate cap in Kbps; 0 for none.
    pub host_max_bitrate: u32,
}

/// The host controller's aim.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum AbrMode {
    #[default]
    #[serde(rename = "balanced")]
    Balanced,
    #[serde(rename = "quality")]
    Quality,
    #[serde(rename = "lowLatency")]
    LowLatency,
}

/// `POST /api/abr`: on with a mode and a Kbps range (0 for no bound), or off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AbrRequest {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<AbrMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_bitrate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bitrate: Option<u32>,
}

impl AbrRequest {
    /// On as V+ PC turns it on: balanced, from 0 up to the chosen bitrate.
    pub fn balanced_up_to(max_kbps: u32) -> Self {
        Self { enabled: true, mode: Some(AbrMode::Balanced), min_bitrate: Some(0), max_bitrate: Some(max_kbps) }
    }

    pub fn off() -> Self {
        Self { enabled: false, mode: None, min_bitrate: None, max_bitrate: None }
    }
}

/// The answer to [`AbrRequest`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AbrConfigured {
    pub success: bool,
    pub enabled: bool,
    /// The bitrate the controller starts from, in Kbps.
    pub initial_bitrate: u32,
    pub min_bitrate: u32,
    pub max_bitrate: u32,
    pub host_max_bitrate: u32,
    pub max_bitrate_capped: bool,
    pub bitrate_applied: bool,
    pub bitrate_apply_error: Option<String>,
    pub error: Option<String>,
}

/// `POST /api/abr/feedback`: this client's network since the last report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AbrFeedback {
    /// Lost video packets in percent, 0 to 100.
    pub packet_loss: f64,
    pub rtt_ms: f64,
    pub decode_fps: f64,
    pub dropped_frames: u32,
    /// The bitrate in Kbps the host last set (or the stream started with).
    pub current_bitrate: u32,
}

impl AbrFeedback {
    /// V+ PC's loss figure: packets that never made a frame (FEC failures, out of order and
    /// invalid ones) against all video and FEC packets of the window, in percent.
    pub fn loss_percent(lost_packets: u64, packets: u64) -> f64 {
        if packets == 0 {
            0.0
        } else {
            (lost_packets as f64 * 100.0 / packets as f64).min(100.0)
        }
    }
}

/// The answer to [`AbrFeedback`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AbrAction {
    /// The bitrate in Kbps the host moved the stream to, when it moved it.
    pub new_bitrate: Option<u32>,
    pub bitrate_applied: bool,
    pub bitrate_apply_error: Option<String>,
    pub reason: String,
}

/// `GET /api/network/capabilities`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NetworkCapabilities {
    pub version: u32,
    pub features: Vec<String>,
    pub bandwidth_probe: Option<BandwidthProbe>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BandwidthProbe {
    pub version: u32,
    pub endpoint: String,
    pub min_bytes: u64,
    pub max_bytes: u64,
    pub cooldown_ms: u64,
}

impl NetworkCapabilities {
    /// The probe size to ask for, or why there is none (V+ Android's `parseNetworkProbeCapabilities`).
    pub fn probe_bytes(&self) -> Result<u64, Error> {
        let probe = self.bandwidth_probe.as_ref().filter(|p| self.version >= 1 && p.version >= 1);
        let Some(probe) = probe else {
            return Err(Error::Malformed("The PC's bandwidth test has an unknown version.".into()));
        };
        if probe.endpoint != "/api/network/probe" || probe.min_bytes == 0 || probe.max_bytes < probe.min_bytes
            || probe.min_bytes > MAX_NETWORK_PROBE_BYTES
        {
            return Err(Error::Malformed("The PC's bandwidth test is not one this device can run.".into()));
        }
        Ok(probe.max_bytes.min(MAX_NETWORK_PROBE_BYTES))
    }
}

/// What a bandwidth probe measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NetworkProbeResult {
    pub bandwidth_mbps: f64,
    /// The mean answer time of the capability requests after the first (which sets up TLS).
    pub latency_ms: f64,
    pub jitter_ms: f64,
}

/// Mean and mean successive difference of the request times after the first.
fn latency_and_jitter(samples: &[f64]) -> (f64, f64) {
    let steady = if samples.len() > 1 { &samples[1..] } else { samples };
    if steady.is_empty() {
        return (0.0, 0.0);
    }
    let mean = steady.iter().sum::<f64>() / steady.len() as f64;
    let jitter = if steady.len() < 2 {
        0.0
    } else {
        steady.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f64>() / (steady.len() - 1) as f64
    };
    (mean, jitter)
}

impl HostClient {
    fn foundation_json<T: DeserializeOwned>(
        &self,
        path: &str,
        body: Option<&[u8]>,
        read: Duration,
    ) -> Result<T, Error> {
        let target = self.url_target(path, None);
        let request = Request {
            host: &self.host,
            port: self.https_port(true)?,
            target: &target,
            tls: Some(self.tls()?),
            timeouts: Timeouts { connect: LONG_CONNECT_TIMEOUT.min(read), read: Some(read) },
            body_limit: MAX_ABR_JSON_BYTES,
        };
        let response = transport::send(&request, &self.cancel, body.map(Body::json))?;
        match response.status {
            200..=299 => serde_json::from_slice(&response.body).map_err(|e| Error::Malformed(format!("/{path}: {e}"))),
            404 => Err(Error::NotFound(format!("/{path}"))),
            code => Err(Error::Status { code: i32::from(code), message: error_text(&response.body).unwrap_or(response.reason) }),
        }
    }

    /// Whether the PC runs Foundation's bitrate controller. 404 on other hosts.
    pub fn abr_capabilities(&self) -> Result<AbrCapabilities, Error> {
        self.foundation_json("api/abr/capabilities", None, ABR_TIMEOUT)
    }

    /// Turns host ABR on or off for this device's running stream.
    pub fn configure_abr(&self, request: &AbrRequest) -> Result<AbrConfigured, Error> {
        let body = serde_json::to_vec(request).map_err(|e| Error::InvalidArgument(e.to_string()))?;
        let read = if request.enabled { ABR_TIMEOUT } else { ABR_OFF_TIMEOUT };
        let answer: AbrConfigured = self.foundation_json("api/abr", Some(&body), read)?;
        if !answer.success {
            return Err(Error::Status { code: 200, message: answer.error.unwrap_or_else(|| "ABR refused".into()) });
        }
        Ok(answer)
    }

    /// Reports this device's network; the answer carries the bitrate the host moved to.
    pub fn abr_feedback(&self, feedback: &AbrFeedback) -> Result<AbrAction, Error> {
        let body = serde_json::to_vec(feedback).map_err(|e| Error::InvalidArgument(e.to_string()))?;
        self.foundation_json("api/abr/feedback", Some(&body), ABR_TIMEOUT)
    }

    /// Foundation's bandwidth test, between streams only (the PC answers 409 during one): four
    /// capability requests for the answer time, then one download of up to 4 MiB.
    pub fn foundation_network_probe(&self) -> Result<NetworkProbeResult, Error> {
        let mut samples = Vec::with_capacity(4);
        let mut capabilities: Option<NetworkCapabilities> = None;
        for _ in 0..4 {
            let started = Instant::now();
            let answer: NetworkCapabilities = self.foundation_json("api/network/capabilities", None, ABR_TIMEOUT)?;
            samples.push(started.elapsed().as_secs_f64() * 1000.0);
            capabilities.get_or_insert(answer);
        }
        let bytes = capabilities.unwrap_or_default().probe_bytes()?;
        let nonce = transport::random_uuid();
        let path = "api/network/probe";
        let target = self.url_target(path, Some(&format!("bytes={bytes}&nonce={nonce}")));
        let mut body_started = None;
        let response = transport::probe(
            &Request {
                host: &self.host,
                port: self.https_port(true)?,
                target: &target,
                tls: Some(self.tls()?),
                timeouts: Timeouts { connect: LONG_CONNECT_TIMEOUT, read: Some(Duration::from_secs(5)) },
                body_limit: bytes as usize,
            },
            &self.cancel,
            PROBE_TIMEOUT,
            &mut |status, headers, _| {
                check_probe_head(status, headers, &nonce)?;
                body_started = Some(Instant::now());
                Ok(())
            },
        )?;
        if response.body.len() as u64 != bytes {
            return Err(Error::Malformed("The bandwidth test ended before its size.".into()));
        }
        let seconds = body_started.map_or(0.0, |t| t.elapsed().as_secs_f64());
        if seconds <= 0.0 {
            return Err(Error::Malformed("The bandwidth test took no time.".into()));
        }
        let (latency_ms, jitter_ms) = latency_and_jitter(&samples);
        Ok(NetworkProbeResult { bandwidth_mbps: bytes as f64 * 8.0 / seconds / 1_000_000.0, latency_ms, jitter_ms })
    }
}

/// The probe answer must be version 1 and echo this request's nonce.
fn check_probe_head(status: u16, headers: &[(String, String)], nonce: &str) -> Result<(), Error> {
    if !(200..=299).contains(&status) {
        let message = match status {
            409 => "The PC is streaming; test again after the stream.".to_owned(),
            429 => "The PC was tested a moment ago; try again shortly.".to_owned(),
            _ => format!("The PC refused the bandwidth test ({status})."),
        };
        return Err(Error::Status { code: i32::from(status), message });
    }
    let get = |name: &str| headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str());
    if get("X-Bandwidth-Probe-Version") != Some("1") || get("X-Bandwidth-Probe-Nonce") != Some(nonce) {
        return Err(Error::Malformed("The bandwidth test answer is not this request's.".into()));
    }
    Ok(())
}

/// The `error` text of a Foundation error body.
fn error_text(body: &[u8]) -> Option<String> {
    #[derive(Deserialize)]
    struct ErrorBody {
        error: String,
    }
    serde_json::from_slice::<ErrorBody>(body).ok().map(|b| b.error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn abr_requests_are_v_plus_bodies() {
        assert_eq!(
            serde_json::to_value(AbrRequest::balanced_up_to(40000)).unwrap(),
            json!({"enabled": true, "mode": "balanced", "minBitrate": 0, "maxBitrate": 40000})
        );
        assert_eq!(serde_json::to_value(AbrRequest::off()).unwrap(), json!({"enabled": false}));
        let feedback = AbrFeedback { packet_loss: 1.5, rtt_ms: 12.0, decode_fps: 120.0, dropped_frames: 3, current_bitrate: 30000 };
        assert_eq!(
            serde_json::to_value(feedback).unwrap(),
            json!({"packetLoss": 1.5, "rttMs": 12.0, "decodeFps": 120.0, "droppedFrames": 3, "currentBitrate": 30000})
        );
        assert_eq!(AbrFeedback::loss_percent(0, 0), 0.0);
        assert_eq!(AbrFeedback::loss_percent(5, 200), 2.5);
        assert_eq!(AbrFeedback::loss_percent(300, 200), 100.0);
    }

    // Foundation's abr_api.cpp answers.
    #[test]
    fn abr_answers_read_as_foundation_writes_them() {
        let caps: AbrCapabilities = serde_json::from_str(
            r#"{"supported":true,"version":1,"features":["llm_ai","game_aware","fallback_threshold","bitrate_cap"],"llmEnabled":false,"hostMaxBitrate":80000}"#,
        )
        .unwrap();
        assert!(caps.supported);
        assert_eq!(caps.host_max_bitrate, 80000);
        let on: AbrConfigured = serde_json::from_str(
            r#"{"success":true,"enabled":true,"mode":"balanced","minBitrate":0,"maxBitrate":40000,"initialBitrate":35000,
                "requestedMaxBitrate":50000,"hostMaxBitrate":40000,"maxBitrateCapped":true,"maxBitrateInheritedFromHost":false,"bitrateApplied":true}"#,
        )
        .unwrap();
        assert_eq!((on.success, on.initial_bitrate, on.max_bitrate, on.max_bitrate_capped), (true, 35000, 40000, true));
        let off: AbrConfigured = serde_json::from_str(r#"{"success":true,"enabled":false}"#).unwrap();
        assert!(off.success && !off.enabled);
        let moved: AbrAction = serde_json::from_str(r#"{"newBitrate":28000,"bitrateApplied":true,"reason":"loss"}"#).unwrap();
        assert_eq!(moved.new_bitrate, Some(28000));
        let kept: AbrAction = serde_json::from_str(r#"{"bitrateApplied":true,"reason":"stable"}"#).unwrap();
        assert_eq!(kept.new_bitrate, None);
        assert_eq!(error_text(br#"{"success":false,"error":"No active streaming session for this client"}"#).as_deref(),
            Some("No active streaming session for this client"));
    }

    #[test]
    fn the_probe_size_follows_the_pcs_capabilities() {
        let caps = |body: &str| serde_json::from_str::<NetworkCapabilities>(body).unwrap();
        let ok = caps(r#"{"version":1,"features":["bandwidth-probe-v1"],"bandwidthProbe":{"version":1,"endpoint":"/api/network/probe","minBytes":262144,"maxBytes":8388608,"cooldownMs":10000}}"#);
        assert_eq!(ok.probe_bytes().unwrap(), MAX_NETWORK_PROBE_BYTES, "capped at 4 MiB");
        let small = caps(r#"{"version":1,"bandwidthProbe":{"version":1,"endpoint":"/api/network/probe","minBytes":1000,"maxBytes":2000}}"#);
        assert_eq!(small.probe_bytes().unwrap(), 2000);
        for bad in [
            r#"{"version":1}"#,
            r#"{"version":0,"bandwidthProbe":{"version":1,"endpoint":"/api/network/probe","minBytes":1,"maxBytes":2}}"#,
            r#"{"version":1,"bandwidthProbe":{"version":1,"endpoint":"/elsewhere","minBytes":1,"maxBytes":2}}"#,
            r#"{"version":1,"bandwidthProbe":{"version":1,"endpoint":"/api/network/probe","minBytes":3,"maxBytes":2}}"#,
            r#"{"version":1,"bandwidthProbe":{"version":1,"endpoint":"/api/network/probe","minBytes":8388608,"maxBytes":9000000}}"#,
        ] {
            assert!(caps(bad).probe_bytes().is_err(), "{bad}");
        }
    }

    #[test]
    fn the_probe_answer_must_echo_the_nonce() {
        let headers = |version: &str, nonce: &str| {
            vec![("x-bandwidth-probe-version".to_owned(), version.to_owned()), ("X-Bandwidth-Probe-Nonce".to_owned(), nonce.to_owned())]
        };
        assert!(check_probe_head(200, &headers("1", "n1"), "n1").is_ok());
        assert!(check_probe_head(200, &headers("1", "n2"), "n1").is_err());
        assert!(check_probe_head(200, &headers("2", "n1"), "n1").is_err());
        assert!(matches!(check_probe_head(409, &[], "n1"), Err(Error::Status { code: 409, .. })));
        assert_eq!(latency_and_jitter(&[90.0, 10.0, 14.0, 12.0]), (12.0, 3.0));
    }
}
